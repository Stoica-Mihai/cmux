//! Per-PTY session state owned by the daemon.
//!
//! Each `Session` owns:
//! - a `portable_pty::MasterPty` + child process running `claude`
//! - an `alacritty_terminal::Term` parsed in lockstep by a reader thread
//! - a 1 MiB raw-byte ring for replay
//! - a `broadcast::Sender<Vec<u8>>` that fans PTY bytes out to attached
//!   clients as `FrameDelta` events
//! - a `watch::Sender<SessionInfo>` that publishes status changes
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
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use cmux_proto::{ClaudeStatus, SessionInfo};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, watch};

const PROMPT_NEEDLES: &[&str] = &[
    "do you want to proceed",
    "allow this",
    "apply this edit",
    "requires approval",
    "don't ask again",
    "esc to cancel",
];

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
    pub dangerous: bool,
    pub resume_id: Option<String>,
    pub term_state: Arc<Mutex<TerminalState>>,
    pub byte_ring: Arc<Mutex<VecDeque<u8>>>,
    pub size: Mutex<(u16, u16)>,
    pub spawned_at_ms: u64,
    pub last_active_ms: Arc<AtomicU64>,
    pub alive: Arc<AtomicBool>,
    pub dirty: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub pid: Option<u32>,
    pub bytes_tx: broadcast::Sender<Vec<u8>>,
    pub info_tx: watch::Sender<SessionInfo>,
    #[allow(dead_code)]
    pub info_rx: watch::Receiver<SessionInfo>,
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
        dangerous: bool,
        resume: Option<String>,
        rows: u16,
        cols: u16,
    ) -> Result<Arc<Self>> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new("claude");
        if dangerous {
            cmd.arg("--dangerously-skip-permissions");
        }
        if let Some(ref rid) = resume {
            cmd.arg("--resume");
            cmd.arg(rid);
        }
        cmd.cwd(&cwd);
        const STRIP: &[&str] = &[
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "COLORTERM",
            "TMUX",
            "TMUX_PANE",
            "WT_SESSION",
            "WT_PROFILE_ID",
            "KITTY_WINDOW_ID",
            "KITTY_INSTALLATION_DIR",
            "KITTY_PID",
            "KITTY_PUBLIC_KEY",
            "ITERM_SESSION_ID",
            "ITERM_PROFILE",
            "LC_TERMINAL",
            "LC_TERMINAL_VERSION",
            "ZELLIJ",
            "ZELLIJ_SESSION_NAME",
            "ZELLIJ_PANE_ID",
            "VTE_VERSION",
            "ALACRITTY_LOG",
            "ALACRITTY_SOCKET",
            "ALACRITTY_WINDOW_ID",
            "GHOSTTY_RESOURCES_DIR",
            "WEZTERM_PANE",
            "WEZTERM_UNIX_SOCKET",
            "WEZTERM_EXECUTABLE",
        ];
        for (k, v) in std::env::vars() {
            if STRIP.iter().any(|name| *name == k.as_str()) {
                continue;
            }
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(cmd).context("spawn claude")?;
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
            dangerous,
            resume_id: resume.clone(),
            rows,
            cols,
            spawned_at_ms: now,
            last_active_ms: now,
            status: ClaudeStatus::Unknown,
            permission_pending: false,
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
            dangerous,
            resume_id: resume,
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

    pub fn write_input(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().map_err(|_| anyhow::anyhow!("writer poisoned"))?;
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        {
            let mut size = self.size.lock().map_err(|_| anyhow::anyhow!("size poisoned"))?;
            if *size == (rows, cols) {
                return Ok(());
            }
            *size = (rows, cols);
        }
        {
            let m = self.master.lock().map_err(|_| anyhow::anyhow!("master poisoned"))?;
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
        let label = self
            .label
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let (rows, cols) = self
            .size
            .lock()
            .map(|s| *s)
            .unwrap_or((24, 80));
        SessionInfo {
            id: self.id,
            label,
            cwd: self.cwd.clone(),
            dangerous: self.dangerous,
            resume_id: self.resume_id.clone(),
            rows,
            cols,
            spawned_at_ms: self.spawned_at_ms,
            last_active_ms: self.last_active_ms.load(Ordering::SeqCst),
            status: ClaudeStatus::Unknown,
            permission_pending: false,
        }
    }

    pub fn rename(&self, new_label: String) {
        if let Ok(mut l) = self.label.lock() {
            *l = new_label;
        }
        let info = self.info();
        let _ = self.info_tx.send(info);
    }

    /// One pass of the status-polling logic. Reads
    /// `~/.claude/sessions/<pid>.json`, scans the live grid for the
    /// permission-prompt heuristics, and updates `info_tx` if any field
    /// changed.
    pub fn poll_status_once(&self) {
        let mut next = self.info_tx.borrow().clone();
        next.last_active_ms = self.last_active_ms.load(Ordering::SeqCst);
        if let Some(pid) = self.pid {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            if let Some(home) = home {
                let path = home
                    .join(".claude")
                    .join("sessions")
                    .join(format!("{}.json", pid));
                if let Ok(bytes) = std::fs::read(&path)
                    && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
                {
                    if let Some(s) = v.get("status").and_then(|x| x.as_str()) {
                        next.status = match s {
                            "busy" => ClaudeStatus::Busy,
                            "idle" => ClaudeStatus::Idle,
                            _ => ClaudeStatus::Unknown,
                        };
                    }
                    if let Some(n) = v.get("name").and_then(|x| x.as_str())
                        && !n.is_empty()
                    {
                        next.label = n.to_string();
                    }
                }
            }
        }
        // permission-prompt heuristic over the visible grid
        if let Ok(t) = self.term_state.lock() {
            next.permission_pending = scan_permission_prompt(&t.term);
        }
        // emit only when something actually changed
        let cur = self.info_tx.borrow().clone();
        if cur.status != next.status
            || cur.label != next.label
            || cur.permission_pending != next.permission_pending
            || cur.last_active_ms != next.last_active_ms
        {
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

fn scan_permission_prompt(term: &Term<VoidListener>) -> bool {
    let mut text = String::new();
    let mut last_line: Option<i32> = None;
    for indexed in term.grid().display_iter() {
        let line = indexed.point.line.0;
        if Some(line) != last_line {
            if last_line.is_some() {
                text.push('\n');
            }
            last_line = Some(line);
        }
        if indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER)
            || indexed.cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let c = indexed.cell.c;
        text.push(if c == '\0' { ' ' } else { c });
    }
    let lower = text.to_lowercase();
    if PROMPT_NEEDLES.iter().any(|n| lower.contains(n)) {
        return true;
    }
    let has_yes = lower.contains("1. yes");
    has_yes && (lower.contains("2. no") || lower.contains("3. no"))
}
