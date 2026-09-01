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

use std::collections::{HashMap, VecDeque};
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
    /// Effective grid size, i.e. what the PTY is actually running at.
    pub size: Mutex<(u16, u16)>,
    /// Size each attached client asked for, keyed by client id. The PTY runs
    /// at the per-axis minimum so every client can draw the grid without
    /// clipping — a phone attached beside a wide terminal would otherwise be
    /// fighting it, last writer winning.
    client_sizes: Mutex<HashMap<u64, (u16, u16)>>,
    /// Size to use while nobody is attached.
    baseline_size: Mutex<(u16, u16)>,
    pub spawned_at_ms: u64,
    pub last_active_ms: Arc<AtomicU64>,
    pub alive: Arc<AtomicBool>,
    pub dirty: Arc<AtomicBool>,
    /// Set once someone renames the session, after which the probe stops
    /// overwriting the label with whatever the child calls itself.
    manually_renamed: AtomicBool,
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
            client_sizes: Mutex::new(HashMap::new()),
            baseline_size: Mutex::new((rows, cols)),
            spawned_at_ms: now,
            last_active_ms,
            alive,
            dirty,
            manually_renamed: AtomicBool::new(false),
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

    /// How many clients currently have a size registered.
    pub fn attached_clients(&self) -> usize {
        self.client_sizes.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Register or update one client's size, then re-apply the minimum.
    pub fn set_client_size(&self, client: u64, rows: u16, cols: u16) -> Result<()> {
        {
            let mut m = self
                .client_sizes
                .lock()
                .map_err(|_| anyhow::anyhow!("client_sizes poisoned"))?;
            m.insert(client, (rows.max(1), cols.max(1)));
        }
        self.apply_effective_size()
    }

    /// Forget a client that has gone away, and grow back if it was the
    /// smallest one holding the grid down.
    pub fn drop_client(&self, client: u64) {
        let removed = self
            .client_sizes
            .lock()
            .map(|mut m| m.remove(&client).is_some())
            .unwrap_or(false);
        if removed {
            let _ = self.apply_effective_size();
        }
    }

    /// Size to use while nothing is attached. Ignored whenever a client is,
    /// since the minimum over attached clients governs then.
    pub fn set_baseline_size(&self, rows: u16, cols: u16) -> Result<()> {
        {
            let mut b = self
                .baseline_size
                .lock()
                .map_err(|_| anyhow::anyhow!("baseline poisoned"))?;
            *b = (rows.max(1), cols.max(1));
        }
        self.apply_effective_size()
    }

    fn effective_size(&self) -> (u16, u16) {
        let clients = self.client_sizes.lock();
        if let Ok(m) = clients
            && !m.is_empty()
        {
            let rows = m.values().map(|(r, _)| *r).min().unwrap_or(24);
            let cols = m.values().map(|(_, c)| *c).min().unwrap_or(80);
            return (rows, cols);
        }
        self.baseline_size.lock().map(|b| *b).unwrap_or((24, 80))
    }

    fn apply_effective_size(&self) -> Result<()> {
        let (rows, cols) = self.effective_size();
        let clients = self.attached_clients();
        tracing::debug!(
            session = self.id,
            rows,
            cols,
            clients,
            "pty size (minimum across attached clients)"
        );
        let changed = self.resize(rows, cols);
        // Tell attached clients what the pty actually runs at. Without this a
        // client keeps rendering at the size it asked for, and the rows past
        // the effective height keep whatever was drawn there before.
        if changed.is_ok() {
            let info = self.info();
            let _ = self.info_tx.send(info);
        }
        changed
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
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

    /// What a newly attached client needs in order to arrive at the current
    /// picture. A full-screen program gets a rendered snapshot, because
    /// replaying its raw output re-executes stale drawing commands and lands
    /// as garbage. Anything else gets the ring, so shell history survives,
    /// followed by a repaint so the visible screen is authoritative.
    pub fn attach_payload(&self) -> Vec<u8> {
        let Ok(t) = self.term_state.lock() else {
            return self.ring_snapshot();
        };
        if crate::snapshot::is_alt_screen(&t.term) {
            return crate::snapshot::render(&t.term);
        }
        let mut out = self.ring_snapshot();
        out.extend_from_slice(&crate::snapshot::render(&t.term));
        out
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

    /// Adopt a name the probe read off the child, unless someone has renamed
    /// the session. Updates the label `info()` reports: writing only the watch
    /// channel left ListSessions and the HTTP API showing the spawn-time name
    /// while attached clients showed the probe's, so the terminal and the
    /// browser named the same session differently. Returns whether it applied.
    fn take_probe_label(&self, label: &str) -> bool {
        if self.manually_renamed.load(Ordering::SeqCst) {
            return false;
        }
        match self.label.lock() {
            Ok(mut current) => {
                *current = label.to_string();
                true
            }
            Err(_) => false,
        }
    }

    pub fn rename(&self, new_label: String) {
        self.manually_renamed.store(true, Ordering::SeqCst);
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
            next.label = l.clone();
            self.take_probe_label(&l);
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

    /// Both directions: a probe name must reach `info()` — the browser reads
    /// that — but must not clobber a name the user chose.
    #[test]
    fn the_probe_names_a_session_until_someone_renames_it() {
        let sess = Session::spawn(
            9,
            "spawned".into(),
            PathBuf::from("/tmp"),
            vec!["/bin/sleep".into(), "30".into()],
            ProbeKind::None,
            24,
            80,
        )
        .expect("spawn");
        assert_eq!(sess.info().label, "spawned");

        assert!(sess.take_probe_label("probe-named"));
        assert_eq!(
            sess.info().label,
            "probe-named",
            "the API the browser reads must see the probe's name"
        );

        sess.rename("mine".into());
        assert_eq!(sess.info().label, "mine");
        assert!(
            !sess.take_probe_label("probe-again"),
            "a manual rename must survive the next probe tick"
        );
        assert_eq!(sess.info().label, "mine");
        sess.kill();
    }

    /// Attach, replay what the daemon hands a new client into a fresh
    /// terminal, and require the same grid *and* the same screen buffer.
    ///
    /// The fixture deliberately overflows the 1 MiB ring. A complete ring
    /// replays correctly, so the bug only shows once the front has been
    /// dropped and the replay starts mid-stream, missing the alt-screen
    /// switch that framed everything after it. That is the state any
    /// long-running full-screen program reaches.
    fn assert_attach_reproduces_the_session(script: &str, expect_alt: bool) {
        let (rows, cols) = (10u16, 40u16);
        let sess = Session::spawn(
            1,
            "t".into(),
            PathBuf::from("/tmp"),
            vec!["/bin/sh".into(), "-c".into(), script.into()],
            ProbeKind::None,
            rows,
            cols,
        )
        .expect("spawn");
        std::thread::sleep(std::time::Duration::from_millis(1200));

        let payload = sess.attach_payload();
        let live = sess.term_state.lock().expect("term");
        assert_eq!(
            crate::snapshot::is_alt_screen(&live.term),
            expect_alt,
            "the fixture did not put the session where the test expects"
        );

        let size = TermSize {
            lines: rows as usize,
            cols: cols as usize,
        };
        let mut fresh = Term::new(TermConfig::default(), &size, VoidListener);
        let mut proc: Processor = Processor::new();
        proc.advance(&mut fresh, &payload);

        assert_eq!(
            crate::snapshot::is_alt_screen(&fresh),
            crate::snapshot::is_alt_screen(&live.term),
            "the attaching client ended up in the other screen buffer"
        );
        for row in 0..rows as usize {
            let line = alacritty_terminal::index::Line(row as i32);
            for col in 0..cols as usize {
                let a = &live.term.grid()[line][alacritty_terminal::index::Column(col)];
                let b = &fresh.grid()[line][alacritty_terminal::index::Column(col)];
                assert_eq!(
                    (a.c, a.fg, a.bg, a.flags),
                    (b.c, b.fg, b.bg, b.flags),
                    "cell ({row},{col}) differs; an attaching client sees something else"
                );
            }
        }
        drop(live);
        sess.kill();
    }

    #[test]
    fn attaching_reproduces_a_long_running_full_screen_program() {
        assert_attach_reproduces_the_session(
            "printf '\\033[?1049h\\033[2J\\033[H'; \
             head -c 1400000 /dev/zero | tr '\\0' 'x'; \
             printf '\\033[2J\\033[HFINAL\\r\\n\\033[7mROW2\\033[0m'; sleep 5",
            true,
        );
    }

    #[test]
    fn attaching_reproduces_a_program_that_repaints_in_place() {
        assert_attach_reproduces_the_session(
            "printf 'old frame\\r\\n'; \
             printf '\\033[2J\\033[H\\033[32mnew frame\\033[0m\\r\\n'; sleep 5",
            false,
        );
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
