//! Daemon connection plumbing for `cmux --connect`.
//!
//! Owns one client connection. Exposes:
//! - `req_tx`: channel for the App to ship Requests to the daemon
//! - `register_slot`: hook a daemon-backed Session up to the events stream
//! - `pending_spawns`: small queue so the App can await SessionSpawned in a
//!   synchronous manner without losing FrameDelta traffic
//!
//! Lives behind an `App.daemon_conn: Option<Arc<DaemonHandle>>`. Local-PTY
//! mode keeps `daemon_conn = None`.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar};

use alacritty_terminal::grid::Dimensions;
use anyhow::{Context, Result};
use cmux_proto::{Event, Request, SessionInfo};

use crate::client::Client;
use crate::session::{DaemonSlot, PendingStatus};
use crate::util::now_ms;

const RING_CAP: usize = 1_048_576;

/// Spawn `cmuxd` in the background and wait up to ~2 s for its socket to
/// appear. Tries the binary next to the current `cmux` first, then `cmuxd`
/// on `$PATH`.
fn try_spawn_daemon(http: Option<&str>) -> Result<()> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.join("cmuxd"));
    }
    candidates.push(std::path::PathBuf::from("cmuxd"));

    let mut launched = false;
    for cand in candidates {
        let mut command = Command::new(&cand);
        if let Some(addr) = http {
            command.arg("--http").arg(addr);
        }
        match command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_child) => {
                // child is intentionally let-dropped: it keeps running in
                // background. Subsequent runs use the existing socket.
                launched = true;
                break;
            }
            Err(_) => continue,
        }
    }
    if !launched {
        anyhow::bail!("could not spawn cmuxd: not in $PATH or next to cmux");
    }

    // Wait for the ready-stamp file (or socket) to appear.
    let socket = crate::client::socket_path().ok_or_else(|| anyhow::anyhow!("no socket path"))?;
    let ready = socket.with_file_name("cmuxd.ready");
    let deadline = Instant::now() + Duration::from_millis(2_000);
    while Instant::now() < deadline {
        if ready.exists() && socket.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    anyhow::bail!("cmuxd did not become ready within 2s")
}

/// Pending spawn waiter. App pushes a `(client_id, mailbox)` here before
/// sending `SpawnSession`, then blocks on `mailbox.wait_for_info()`. The
/// events reader thread moves matching `SessionSpawned` events into the
/// mailbox.
pub struct SpawnMailbox {
    inner: Mutex<Option<SessionInfo>>,
    cv: Condvar,
}

impl SpawnMailbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(None),
            cv: Condvar::new(),
        })
    }
    pub fn fulfill(&self, info: SessionInfo) {
        let mut g = self.inner.lock().unwrap();
        *g = Some(info);
        self.cv.notify_all();
    }
    pub fn wait(&self, timeout_ms: u64) -> Option<SessionInfo> {
        let mut g = self.inner.lock().unwrap();
        if g.is_some() {
            return g.take();
        }
        let (mut gg, _) = self
            .cv
            .wait_timeout(g, std::time::Duration::from_millis(timeout_ms))
            .unwrap();
        gg.take()
    }
}

pub struct DaemonHandle {
    pub req_tx: mpsc::Sender<Request>,
    pub slots: Arc<Mutex<HashMap<u64, Arc<DaemonSlot>>>>,
    pub pending_spawns: Arc<Mutex<VecDeque<Arc<SpawnMailbox>>>>,
    pub alive: Arc<AtomicBool>,
}

impl DaemonHandle {
    pub fn register_slot(&self, remote_id: u64, slot: DaemonSlot) {
        self.slots.lock().unwrap().insert(remote_id, Arc::new(slot));
    }

    pub fn request(&self, req: Request) -> Result<()> {
        self.req_tx
            .send(req)
            .map_err(|_| anyhow::anyhow!("daemon channel closed"))
    }
}

/// Connect, run the Hello/Welcome handshake, list existing sessions, and
/// return `(DaemonHandle, initial_session_infos)`. The handle's reader thread
/// is already running; the writer thread drains `req_tx` into the socket.
pub fn connect(path: &Path, http: Option<&str>) -> Result<(Arc<DaemonHandle>, Vec<SessionInfo>)> {
    // `http` only reaches a daemon this call starts. One already running keeps
    // whatever it was launched with.
    let mut client = match Client::connect(path) {
        Ok(c) => c,
        Err(_) => {
            try_spawn_daemon(http)?;
            Client::connect(path).context("connect cmuxd after auto-spawn")?
        }
    };
    client.send(&Request::ListSessions)?;
    let infos = match client.recv()? {
        Event::SessionList { sessions } => sessions,
        other => anyhow::bail!("expected SessionList, got {other:?}"),
    };

    let (mut creader, mut cwriter) = client.split().context("split client")?;
    let (req_tx, req_rx) = mpsc::channel::<Request>();
    let slots: Arc<Mutex<HashMap<u64, Arc<DaemonSlot>>>> = Default::default();
    let pending_spawns: Arc<Mutex<VecDeque<Arc<SpawnMailbox>>>> = Default::default();
    let alive = Arc::new(AtomicBool::new(true));

    // Writer thread: drain req_rx into the daemon.
    std::thread::Builder::new()
        .name("cmuxd-writer".into())
        .spawn(move || {
            while let Ok(req) = req_rx.recv() {
                if cwriter.send(&req).is_err() {
                    break;
                }
            }
        })?;

    // Reader thread: distribute events into per-session slots + spawn mailboxes.
    let slots_for_reader = slots.clone();
    let pending_for_reader = pending_spawns.clone();
    let alive_for_reader = alive.clone();
    let req_tx_for_reader = req_tx.clone();
    std::thread::Builder::new()
        .name("cmuxd-reader".into())
        .spawn(move || {
            while let Ok(ev) = creader.recv() {
                match ev {
                    Event::FrameDelta { id, bytes } => {
                        let slot_opt = slots_for_reader
                            .lock()
                            .ok()
                            .and_then(|m| m.get(&id).cloned());
                        if let Some(slot) = slot_opt {
                            if let Ok(mut p) = slot.parser.lock() {
                                p.process(&bytes);
                            }
                            if let Ok(mut r) = slot.byte_ring.lock() {
                                r.extend(bytes.iter().copied());
                                let over = r.len().saturating_sub(RING_CAP);
                                if over > 0 {
                                    r.drain(..over);
                                }
                            }
                            slot.last_active_ms.store(now_ms(), Ordering::SeqCst);
                            slot.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                    Event::SessionExited { id, .. } => {
                        if let Ok(m) = slots_for_reader.lock()
                            && let Some(slot) = m.get(&id)
                        {
                            slot.alive.store(false, Ordering::SeqCst);
                            slot.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                    Event::SessionSpawned { id: _, info } => {
                        if let Ok(mut q) = pending_for_reader.lock()
                            && let Some(mb) = q.pop_front()
                        {
                            mb.fulfill(info);
                        }
                    }
                    Event::Resync { id } => {
                        // Client lagged the broadcast queue. Reset the local
                        // grid + ring and ask the daemon to replay history.
                        if let Ok(m) = slots_for_reader.lock()
                            && let Some(slot) = m.get(&id)
                        {
                            if let Ok(mut p) = slot.parser.lock() {
                                let (rows, cols) = (
                                    p.term.grid().screen_lines() as u16,
                                    p.term.grid().columns() as u16,
                                );
                                *p = crate::session::TerminalState::fresh(rows, cols);
                            }
                            if let Ok(mut r) = slot.byte_ring.lock() {
                                r.clear();
                            }
                            slot.dirty.store(true, Ordering::Relaxed);
                        }
                        let _ = req_tx_for_reader.send(Request::Attach {
                            session_id: id,
                            want_history: true,
                        });
                    }
                    Event::StatusUpdate {
                        id,
                        status,
                        label,
                        attention,
                    } => {
                        if let Ok(m) = slots_for_reader.lock()
                            && let Some(slot) = m.get(&id)
                            && let Ok(mut ps) = slot.pending_status.lock()
                        {
                            *ps = Some(PendingStatus {
                                status,
                                label,
                                attention,
                            });
                            slot.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                    Event::Goodbye { .. } => break,
                    _ => {}
                }
            }
            // Reader done: mark daemon dead + cascade to all slots.
            alive_for_reader.store(false, Ordering::SeqCst);
            if let Ok(m) = slots_for_reader.lock() {
                for slot in m.values() {
                    slot.alive.store(false, Ordering::SeqCst);
                    slot.dirty.store(true, Ordering::Relaxed);
                }
            }
        })?;

    Ok((
        Arc::new(DaemonHandle {
            req_tx,
            slots,
            pending_spawns,
            alive,
        }),
        infos,
    ))
}
