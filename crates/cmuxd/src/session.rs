//! Per-PTY session state owned by the daemon.
//!
//! Each `Session` owns:
//! - a `portable_pty::MasterPty` + child process running an arbitrary command
//! - an `alacritty_terminal::Term` parsed in lockstep by a reader thread
//! - a 1 MiB raw-byte ring for replay
//! - a `broadcast::Sender<Vec<u8>>` that fans PTY bytes out to attached
//!   clients as `FrameDelta` events
//! - a `watch::Sender<SessionInfo>` that publishes status changes
//! - an optional [`StatusProbe`], the only part that knows what is running
//!
//! ## Lock order
//!
//! When more than one lock must be held simultaneously, acquire in this order
//! to keep the daemon deadlock-free:
//!
//!   `term`  >  `byte_ring`  >  `size`  >  `info_tx` (`watch::Sender`)  >  `master`
//!
//! Broadcast and atomic primitives (`bytes_tx`, `alive`, `last_active_ms`,
//! `dirty`) are non-blocking and may be touched at any point. The reader
//! thread is the only writer for `term` + `byte_ring`; everything else only
//! reads from them, which keeps contention bounded to short critical sections.
//!
//! See `DAEMON_PLAN.md` §7.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use cmux_proto::{ProbeKind, SessionInfo, SessionStatus};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, watch};

use crate::probe::{ProbeCtx, StatusProbe};

const SCROLLBACK_LINES: usize = 4096;
const RING_BYTES_CAP: usize = 1_048_576;
const BROADCAST_QUEUE: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub struct TermSize {
    pub lines: usize,
    pub cols: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct TerminalState {
    pub term: Term<VoidListener>,
    pub proc: Processor,
}

impl TerminalState {
    fn new(rows: u16, cols: u16) -> Self {
        let config = TermConfig {
            scrolling_history: SCROLLBACK_LINES,
            ..Default::default()
        };
        let size = TermSize {
            lines: rows.max(1) as usize,
            cols: cols.max(1) as usize,
        };
        Self {
            term: Term::new(config, &size, VoidListener),
            proc: Processor::new(),
        }
    }

    fn process(&mut self, bytes: &[u8]) {
        self.proc.advance(&mut self.term, bytes);
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let size = TermSize {
            lines: rows.max(1) as usize,
            cols: cols.max(1) as usize,
        };
        self.term.resize(size);
    }
}

pub struct Session {
    pub id: u64,
    pub label: Mutex<String>,
    pub cwd: PathBuf,
    /// argv this session was exec'd with, `cmd[0]` being the program.
    pub cmd: Vec<String>,
    pub probe_kind: ProbeKind,
    pub term_state: Arc<Mutex<TerminalState>>,
    pub byte_ring: Arc<Mutex<VecDeque<u8>>>,
    pub size: Mutex<(u16, u16)>,
    pub spawned_at_ms: u64,
    pub last_active_ms: Arc<AtomicU64>,
    pub alive: Arc<AtomicBool>,
    pub dirty: Arc<AtomicBool>,
    pub pid: Option<u32>,
    pub bytes_tx: broadcast::Sender<Vec<u8>>,
    pub info_tx: watch::Sender<SessionInfo>,
    pub info_rx: watch::Receiver<SessionInfo>,
    probe: Option<Box<dyn StatusProbe>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn std::io::Write + Send>>,
    #[allow(dead_code)]
    child: Mutex<Box<dyn Child + Send + Sync>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[allow(dead_code)]
    reader_thread: JoinHandle<()>,
}

impl Session {
    pub fn spawn(
        id: u64,
        label: String,
        cwd: PathBuf,
        cmd: Vec<String>,
        probe_kind: ProbeKind,
        rows: u16,
        cols: u16,
    ) -> Result<Arc<Self>> {
        let Some((program, args)) = cmd.split_first() else {
            anyhow::bail!("SpawnSession: empty command");
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let mut builder = CommandBuilder::new(program);
        for arg in args {
            builder.arg(arg);
        }
        builder.cwd(&cwd);
        for (k, v) in cmux_proto::terminal_spawn_env() {
            builder.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .with_context(|| format!("spawn {program}"))?;
        let killer = child.clone_killer();
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().context("clone reader")?;
        let writer = pair.master.take_writer().context("take writer")?;

        let term_state = Arc::new(Mutex::new(TerminalState::new(rows, cols)));
        let byte_ring: Arc<Mutex<VecDeque<u8>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(RING_BYTES_CAP)));
        let alive = Arc::new(AtomicBool::new(true));
        let dirty = Arc::new(AtomicBool::new(true));
        let now = now_ms();
        let last_active_ms = Arc::new(AtomicU64::new(now));
        let (bytes_tx, _bytes_rx) = broadcast::channel::<Vec<u8>>(BROADCAST_QUEUE);
        let pid = child.process_id();

        let info = SessionInfo {
            id,
            label: label.clone(),
            cwd: cwd.clone(),
            cmd: cmd.clone(),
            probe: probe_kind.clone(),
            rows,
            cols,
            spawned_at_ms: now,
            last_active_ms: now,
            status: SessionStatus::Unknown,
            attention: false,
        };
        let (info_tx, info_rx) = watch::channel(info);

        let reader_thread = {
            let term_state = term_state.clone();
            let alive_t = alive.clone();
            let dirty_t = dirty.clone();
            let last_active_t = last_active_ms.clone();
            let ring_t = byte_ring.clone();
            let bytes_tx_t = bytes_tx.clone();
            let mut reader = reader;
            std::thread::Builder::new()
                .name(format!("pty-reader-{}", id))
                .spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        match std::io::Read::read(&mut reader, &mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let chunk = buf[..n].to_vec();
                                if let Ok(mut t) = term_state.lock() {
                                    t.process(&chunk);
                                }
                                if let Ok(mut r) = ring_t.lock() {
                                    r.extend(chunk.iter().copied());
                                    let over = r.len().saturating_sub(RING_BYTES_CAP);
                                    if over > 0 {
                                        r.drain(..over);
                                    }
                                }
                                last_active_t.store(now_ms(), Ordering::SeqCst);
                                dirty_t.store(true, Ordering::Relaxed);
                                // best-effort fan-out — drop on full
                                let _ = bytes_tx_t.send(chunk);
                            }
                            Err(_) => break,
                        }
                    }
                    alive_t.store(false, Ordering::SeqCst);
                })?
        };

        Ok(Arc::new(Self {
            id,
            label: Mutex::new(label),
            cwd,
            cmd,
            probe: crate::probe::build(&probe_kind),
            probe_kind,
            term_state,
            byte_ring,
            size: Mutex::new((rows, cols)),
            spawned_at_ms: now,
            last_active_ms,
            alive,
            dirty,
            pid,
            bytes_tx,
            info_tx,
            info_rx,
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            killer: Mutex::new(killer),
            reader_thread,
        }))
    }

    /// Whether this session has anything to poll. Sessions without a probe get
    /// no status ticker at all.
    pub fn has_probe(&self) -> bool {
        self.probe.is_some()
    }

    pub fn write_input(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("writer poisoned"))?;
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        {
            let mut size = self
                .size
                .lock()
                .map_err(|_| anyhow::anyhow!("size poisoned"))?;
            if *size == (rows, cols) {
                return Ok(());
            }
            *size = (rows, cols);
        }
        {
            let m = self
                .master
                .lock()
                .map_err(|_| anyhow::anyhow!("master poisoned"))?;
            m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize pty")?;
        }
        if let Ok(mut t) = self.term_state.lock() {
            t.resize(rows, cols);
        }
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn ring_snapshot(&self) -> Vec<u8> {
        self.byte_ring
            .lock()
            .map(|r| r.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn info(&self) -> SessionInfo {
        let label = self.label.lock().map(|s| s.clone()).unwrap_or_default();
        let (rows, cols) = self.size.lock().map(|s| *s).unwrap_or((24, 80));
        // status/attention live in the watch channel, written by the probe.
        let observed = self.info_tx.borrow().clone();
        SessionInfo {
            id: self.id,
            label,
            cwd: self.cwd.clone(),
            cmd: self.cmd.clone(),
            probe: self.probe_kind.clone(),
            rows,
            cols,
            spawned_at_ms: self.spawned_at_ms,
            last_active_ms: self.last_active_ms.load(Ordering::SeqCst),
            status: observed.status,
            attention: observed.attention,
        }
    }

    pub fn rename(&self, new_label: String) {
        if let Ok(mut l) = self.label.lock() {
            *l = new_label;
        }
        let info = self.info();
        let _ = self.info_tx.send(info);
    }

    /// One pass of the status probe. No-op when the session has none.
    pub fn poll_status_once(&self) {
        let Some(probe) = self.probe.as_ref() else {
            return;
        };
        let outcome = {
            let Ok(t) = self.term_state.lock() else {
                return;
            };
            probe.poll(&ProbeCtx {
                pid: self.pid,
                term: &t.term,
            })
        };

        let mut next = self.info_tx.borrow().clone();
        next.last_active_ms = self.last_active_ms.load(Ordering::SeqCst);
        if let Some(s) = outcome.status {
            next.status = s;
        }
        if let Some(l) = outcome.label {
            next.label = l;
        }
        if let Some(a) = outcome.attention {
            next.attention = a;
        }
        // emit only when something actually changed
        if *self.info_tx.borrow() != next {
            let _ = self.info_tx.send(next);
        }
    }

    pub fn kill(&self) {
        if let Ok(mut k) = self.killer.lock() {
            let _ = k.kill();
        }
        self.alive.store(false, Ordering::SeqCst);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_rejects_an_empty_command() {
        let result = Session::spawn(
            1,
            "x".into(),
            PathBuf::from("/tmp"),
            Vec::<String>::new(),
            ProbeKind::None,
            24,
            80,
        );
        let err = match result {
            Ok(_) => panic!("empty argv must not spawn a session"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("empty command"), "got: {err}");
    }

    #[test]
    fn spawn_runs_an_arbitrary_program_and_reports_it() {
        let sess = Session::spawn(
            7,
            "echo-test".into(),
            PathBuf::from("/tmp"),
            vec!["/bin/echo".into(), "hello".into()],
            ProbeKind::None,
            30,
            100,
        )
        .expect("spawn /bin/echo");

        let info = sess.info();
        assert_eq!(info.cmd, vec!["/bin/echo", "hello"]);
        assert_eq!(info.probe, ProbeKind::None);
        assert_eq!((info.rows, info.cols), (30, 100));
        assert!(!sess.has_probe());
    }

    #[test]
    fn a_claude_session_gets_a_probe() {
        let sess = Session::spawn(
            8,
            "claude-test".into(),
            PathBuf::from("/tmp"),
            vec!["/bin/true".into()],
            ProbeKind::Claude {
                dangerous: true,
                resume_id: None,
            },
            24,
            80,
        )
        .expect("spawn");
        assert!(sess.has_probe());
        assert!(sess.info().probe.dangerous());
    }
}
