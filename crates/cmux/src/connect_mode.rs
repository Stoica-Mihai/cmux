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
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Condvar};

use anyhow::{Context, Result};
use cmux_proto::{Event, Request, SessionInfo};

use crate::client::Client;
use crate::session::DaemonSlot;
use crate::util::now_ms;

const RING_CAP: usize = 1_048_576;

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
pub fn connect(path: &Path) -> Result<(Arc<DaemonHandle>, Vec<SessionInfo>)> {
    let mut client = Client::connect(path).context("connect cmuxd")?;
    client.send(&Request::ListSessions)?;
    let infos = match client.recv()? {
        Event::SessionList { sessions } => sessions,
        other => anyhow::bail!("expected SessionList, got {other:?}"),
    };

    let (mut creader, mut cwriter) = client.split().context("split client")?;
    let (req_tx, req_rx) = mpsc::channel::<Request>();
    let slots: Arc<Mutex<HashMap<u64, Arc<DaemonSlot>>>> = Default::default();
    let pending_spawns: Arc<Mutex<VecDeque<Arc<SpawnMailbox>>>> = Default::default();

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
    std::thread::Builder::new()
        .name("cmuxd-reader".into())
        .spawn(move || {
            while let Ok(ev) = creader.recv() {
                match ev {
                    Event::FrameDelta { id, bytes } => {
                        let slot_opt = slots_for_reader.lock().ok().and_then(|m| m.get(&id).cloned());
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
                    Event::Goodbye { .. } => break,
                    _ => {}
                }
            }
            // Reader done: mark all slots dead so the App reaps them.
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
        }),
        infos,
    ))
}
