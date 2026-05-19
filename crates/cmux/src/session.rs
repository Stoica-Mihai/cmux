use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use cmux_proto::Request as ProtoRequest;
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::debug_log;
use crate::term_render::{self, TermSize, TileSelection};
use crate::util::{claude_sessions_dir, now_ms};

const SCROLLBACK_LINES: usize = 4096;
const RING_BYTES_CAP: usize = 1_048_576; // 1 MiB raw PTY history per session
const REPLAY_HISTORY_LINES: usize = 16_384;

const PROMPT_NEEDLES: &[&str] = &[
    "do you want to proceed",
    "allow this",
    "apply this edit",
    "requires approval",
    "don't ask again",
    "esc to cancel",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClaudeStatus {
    #[default]
    Unknown,
    Busy,
    Idle,
}

impl ClaudeStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "busy" => Self::Busy,
            "idle" => Self::Idle,
            _ => Self::Unknown,
        }
    }
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub struct TerminalState {
    pub term: Term<VoidListener>,
    pub proc: Processor,
}

/// Strip xterm alt-screen mode set/reset sequences so replay through a fresh
/// Term keeps writes in the primary buffer (which has full scrollback).
/// Targets CSI ?1049h, ?1049l, ?47h, ?47l, ?1047h, ?1047l.
pub fn strip_alt_screen(input: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == 0x1b && i + 1 < input.len() && input[i + 1] == b'[' {
            // peek forward for ? ... h/l
            let mut j = i + 2;
            while j < input.len() {
                let b = input[j];
                if b == b'h' || b == b'l' || b == b'~' || (b as char).is_alphabetic() {
                    break;
                }
                j += 1;
            }
            if j < input.len() && (input[j] == b'h' || input[j] == b'l') {
                let params = &input[i + 2..j];
                let trailer = input[j];
                if matches!(params, b"?1049" | b"?47" | b"?1047")
                    && (trailer == b'h' || trailer == b'l')
                {
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

pub fn build_scrollback(rows: u16, cols: u16, ring_bytes: &[u8]) -> TerminalState {
    let config = TermConfig {
        scrolling_history: REPLAY_HISTORY_LINES,
        ..Default::default()
    };
    let size = TermSize {
        lines: rows.max(1) as usize,
        cols: cols.max(1) as usize,
    };
    let mut term = Term::new(config, &size, VoidListener);
    let mut proc: Processor = Processor::new();
    let cleaned = strip_alt_screen(ring_bytes);
    proc.advance(&mut term, &cleaned);
    // Start at bottom (display_offset=0) so user sees the current frame on
    // entry; scrolling up reveals older history.
    term.scroll_display(Scroll::Bottom);
    TerminalState { term, proc }
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
        let term = Term::new(config, &size, VoidListener);
        Self {
            term,
            proc: Processor::new(),
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.proc.advance(&mut self.term, bytes);
    }

    /// Construct a fresh `TerminalState` at the given grid size. Used by
    /// `Event::Resync` handling on the daemon-backed path.
    pub fn fresh(rows: u16, cols: u16) -> Self {
        Self::new(rows, cols)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = TermSize {
            lines: rows.max(1) as usize,
            cols: cols.max(1) as usize,
        };
        self.term.resize(size);
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.term.scroll_display(scroll);
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }
}

/// Per-session daemon hooks, shared with the daemon-events reader thread.
pub struct DaemonSlot {
    pub parser: Arc<Mutex<TerminalState>>,
    pub byte_ring: Arc<Mutex<VecDeque<u8>>>,
    pub dirty: Arc<AtomicBool>,
    pub alive: Arc<AtomicBool>,
    pub last_active_ms: Arc<AtomicU64>,
    pub pending_status: Arc<Mutex<Option<PendingStatus>>>,
}

/// Latest `Event::StatusUpdate` payload that hasn't yet been merged into the
/// owning `Session` struct. Drained on the next `Session::poll_status` tick.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub status: cmux_proto::ClaudeStatus,
    pub label: Option<String>,
    pub permission_pending: bool,
}

enum Backend {
    Local {
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
        killer: Box<dyn ChildKiller + Send + Sync>,
        _reader_thread: JoinHandle<()>,
    },
    Daemon {
        remote_id: u64,
        req_tx: mpsc::Sender<ProtoRequest>,
    },
}

pub struct Session {
    pub id: u64,
    pub label: String,
    pub cwd: PathBuf,
    pub dangerous: bool,
    pub resume_id: Option<String>,
    pub parser: Arc<Mutex<TerminalState>>,
    pub size: (u16, u16),
    pub alive: Arc<AtomicBool>,
    pub last_active_ms: Arc<AtomicU64>,
    pub dirty: Arc<AtomicBool>,
    pub byte_ring: Arc<Mutex<VecDeque<u8>>>,
    pub scrollback: Option<TerminalState>,
    pub pid: Option<u32>,
    pub claude_status: ClaudeStatus,
    pub claude_name: Option<String>,
    pub permission_pending: bool,
    pub manually_renamed: bool,
    pub selection: Option<TileSelection>,
    pub mouse_down_at: Option<(u16, u16)>,
    last_status_check_ms: u64,
    last_perm_check_active_ms: u64,
    daemon_pending_status: Option<Arc<Mutex<Option<PendingStatus>>>>,
    backend: Backend,
}

impl Session {
    pub fn spawn(
        id: u64,
        label: String,
        cwd: PathBuf,
        dangerous: bool,
        rows: u16,
        cols: u16,
        resume: Option<String>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size(rows, cols))
            .context("openpty")?;

        let mut cmd = CommandBuilder::new("claude");
        if dangerous {
            cmd.arg("--dangerously-skip-permissions");
        }
        if let Some(ref id) = resume {
            cmd.arg("--resume");
            cmd.arg(id);
        }
        cmd.cwd(&cwd);
        for (k, v) in cmux_proto::claude_spawn_env() {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).context("spawn claude")?;
        let killer = child.clone_killer();
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().context("clone reader")?;
        let writer = pair.master.take_writer().context("take writer")?;
        let parser = Arc::new(Mutex::new(TerminalState::new(rows, cols)));
        let alive = Arc::new(AtomicBool::new(true));
        let dirty = Arc::new(AtomicBool::new(true));
        let byte_ring: Arc<Mutex<VecDeque<u8>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(RING_BYTES_CAP)));
        let last_active_ms = Arc::new(AtomicU64::new(now_ms()));

        let reader_thread = {
            let parser = parser.clone();
            let alive = alive.clone();
            let dirty_t = dirty.clone();
            let ring_t = byte_ring.clone();
            let last_active = last_active_ms.clone();
            let mut reader = reader;
            thread::Builder::new()
                .name(format!("pty-reader-{}", id))
                .spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        match std::io::Read::read(&mut reader, &mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Ok(mut p) = parser.lock() {
                                    p.process(&buf[..n]);
                                }
                                if let Ok(mut r) = ring_t.lock() {
                                    r.extend(buf[..n].iter().copied());
                                    let over = r.len().saturating_sub(RING_BYTES_CAP);
                                    if over > 0 {
                                        r.drain(..over);
                                    }
                                }
                                last_active.store(now_ms(), Ordering::SeqCst);
                                dirty_t.store(true, Ordering::Relaxed);
                            }
                            Err(_) => break,
                        }
                    }
                    alive.store(false, Ordering::SeqCst);
                })?
        };

        let pid = child.process_id();
        Ok(Self {
            id,
            label,
            cwd,
            dangerous,
            resume_id: resume.clone(),
            parser,
            size: (rows, cols),
            alive,
            last_active_ms,
            dirty,
            byte_ring,
            scrollback: None,
            pid,
            claude_status: ClaudeStatus::Unknown,
            claude_name: None,
            permission_pending: false,
            manually_renamed: false,
            selection: None,
            mouse_down_at: None,
            last_status_check_ms: 0,
            last_perm_check_active_ms: 0,
            daemon_pending_status: None,
            backend: Backend::Local {
                master: pair.master,
                writer,
                child,
                killer,
                _reader_thread: reader_thread,
            },
        })
    }

    /// Build a daemon-backed Session. The caller is responsible for arranging
    /// a reader thread that feeds FrameDelta bytes from the daemon into the
    /// returned `parser`/`byte_ring` via the returned `DaemonSlot`, and for
    /// servicing `req_tx` to ship Requests to the daemon.
    #[allow(clippy::too_many_arguments)]
    pub fn new_daemon(
        id: u64,
        label: String,
        cwd: PathBuf,
        dangerous: bool,
        resume_id: Option<String>,
        rows: u16,
        cols: u16,
        pid: Option<u32>,
        remote_id: u64,
        req_tx: mpsc::Sender<ProtoRequest>,
    ) -> (Self, DaemonSlot) {
        let parser = Arc::new(Mutex::new(TerminalState::new(rows, cols)));
        let alive = Arc::new(AtomicBool::new(true));
        let dirty = Arc::new(AtomicBool::new(true));
        let byte_ring: Arc<Mutex<VecDeque<u8>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(RING_BYTES_CAP)));
        let last_active_ms = Arc::new(AtomicU64::new(now_ms()));
        let pending_status: Arc<Mutex<Option<PendingStatus>>> = Arc::new(Mutex::new(None));
        let slot = DaemonSlot {
            parser: parser.clone(),
            byte_ring: byte_ring.clone(),
            dirty: dirty.clone(),
            alive: alive.clone(),
            last_active_ms: last_active_ms.clone(),
            pending_status: pending_status.clone(),
        };
        let s = Self {
            id,
            label,
            cwd,
            dangerous,
            resume_id,
            parser,
            size: (rows, cols),
            alive,
            last_active_ms,
            dirty,
            byte_ring,
            scrollback: None,
            pid,
            claude_status: ClaudeStatus::Unknown,
            claude_name: None,
            permission_pending: false,
            manually_renamed: false,
            selection: None,
            mouse_down_at: None,
            last_status_check_ms: 0,
            last_perm_check_active_ms: 0,
            daemon_pending_status: Some(pending_status),
            backend: Backend::Daemon { remote_id, req_tx },
        };
        (s, slot)
    }

    #[allow(dead_code)]
    pub fn is_daemon_backed(&self) -> bool {
        matches!(self.backend, Backend::Daemon { .. })
    }

    pub fn poll_status(&mut self) {
        let now = now_ms();
        if now.saturating_sub(self.last_status_check_ms) < 500 {
            return;
        }
        self.last_status_check_ms = now;

        // Daemon mode: drain pending StatusUpdate(s) emitted by the events
        // reader thread. Skip the local PID-file lookup since the claude
        // process isn't ours.
        if let Some(slot) = self.daemon_pending_status.as_ref() {
            if let Ok(mut ps) = slot.lock()
                && let Some(p) = ps.take()
            {
                let mapped = match p.status {
                    cmux_proto::ClaudeStatus::Busy => ClaudeStatus::Busy,
                    cmux_proto::ClaudeStatus::Idle => ClaudeStatus::Idle,
                    cmux_proto::ClaudeStatus::Unknown => ClaudeStatus::Unknown,
                };
                if self.claude_status != mapped {
                    self.claude_status = mapped;
                }
                if let Some(n) = p.label
                    && !n.is_empty()
                    && self.claude_name.as_deref() != Some(n.as_str())
                {
                    if !self.manually_renamed {
                        self.label = n.clone();
                    }
                    self.claude_name = Some(n);
                }
                self.permission_pending = p.permission_pending;
            }
            return;
        }

        if let Some(pid) = self.pid {
            let path = claude_sessions_dir().map(|d| d.join(format!("{}.json", pid)));
            if let Some(path) = path
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
            {
                if let Some(s) = v.get("status").and_then(|x| x.as_str()) {
                    let next = ClaudeStatus::from_str(s);
                    if self.claude_status != next {
                        self.claude_status = next;
                    }
                }
                if let Some(n) = v.get("name").and_then(|x| x.as_str())
                    && !n.is_empty()
                    && self.claude_name.as_deref() != Some(n)
                {
                    if !self.manually_renamed {
                        self.label = n.to_string();
                    }
                    self.claude_name = Some(n.to_string());
                }
            }
        }

        let last_active = self.last_active_ms.load(Ordering::SeqCst);
        if last_active != self.last_perm_check_active_ms {
            self.permission_pending = self.detect_permission_prompt();
            self.last_perm_check_active_ms = last_active;
        }
    }

    fn detect_permission_prompt(&self) -> bool {
        let Ok(p) = self.parser.lock() else {
            return false;
        };
        let text = term_render::visible_text(&p.term);
        debug_log!(
            &format!("/tmp/cmux-screen-{}.txt", self.id),
            "\n========= scan at id={} =========\n{}\n",
            self.id,
            text
        );
        let lower = text.to_lowercase();
        if PROMPT_NEEDLES.iter().any(|n| lower.contains(n)) {
            return true;
        }
        let has_yes = lower.contains("1. yes");
        has_yes && (lower.contains("2. no") || lower.contains("3. no"))
    }

    pub fn activity_age_ms(&self) -> u64 {
        now_ms().saturating_sub(self.last_active_ms.load(Ordering::SeqCst))
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        match &mut self.backend {
            Backend::Local { writer, .. } => {
                writer.write_all(bytes)?;
                writer.flush()?;
            }
            Backend::Daemon { remote_id, req_tx } => {
                req_tx
                    .send(ProtoRequest::Input {
                        session_id: *remote_id,
                        bytes: bytes.to_vec(),
                    })
                    .map_err(|_| anyhow::anyhow!("daemon channel closed"))?;
            }
        }
        Ok(())
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        if (rows, cols) == self.size {
            return Ok(());
        }
        self.size = (rows, cols);
        match &mut self.backend {
            Backend::Local { master, .. } => {
                master.resize(pty_size(rows, cols)).context("resize pty")?;
            }
            Backend::Daemon { remote_id, req_tx } => {
                let _ = req_tx.send(ProtoRequest::Resize {
                    session_id: *remote_id,
                    rows,
                    cols,
                });
            }
        }
        if let Ok(mut p) = self.parser.lock() {
            p.resize(rows, cols);
        }
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn is_alive(&mut self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match &mut self.backend {
            Backend::Local { child, .. } => match child.try_wait() {
                Ok(Some(_)) => {
                    self.alive.store(false, Ordering::SeqCst);
                    false
                }
                _ => true,
            },
            Backend::Daemon { .. } => true,
        }
    }

    /// Hard-kill: ends the underlying claude process. For daemon mode this
    /// sends `Detach { keep_session: false }`.
    #[allow(dead_code)]
    pub fn kill(&mut self) {
        match &mut self.backend {
            Backend::Local { killer, .. } => {
                let _ = killer.kill();
            }
            Backend::Daemon { remote_id, req_tx } => {
                let _ = req_tx.send(ProtoRequest::Detach {
                    session_id: *remote_id,
                    keep_session: false,
                });
            }
        }
        self.alive.store(false, Ordering::SeqCst);
    }

    /// Detach a daemon-backed session while keeping it alive on the daemon.
    /// No-op for local sessions.
    pub fn detach_keep(&mut self) {
        if let Backend::Daemon { remote_id, req_tx } = &mut self.backend {
            let _ = req_tx.send(ProtoRequest::Detach {
                session_id: *remote_id,
                keep_session: true,
            });
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Only the local backend forcibly kills the child on drop. Daemon
        // backend leaves the session to be managed by explicit kill() /
        // detach_keep() calls.
        if let Backend::Local { killer, .. } = &mut self.backend {
            let _ = killer.kill();
            self.alive.store(false, Ordering::SeqCst);
        }
    }
}
