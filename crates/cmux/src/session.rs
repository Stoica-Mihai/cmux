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
use crate::util::now_ms;

use cmux_proto::{RING_BYTES_CAP, SCROLLBACK_LINES};

/// Deeper than a live session's grid: a replay walks the whole ring, so the
/// history it rebuilds is bounded by the ring rather than the screen.
const REPLAY_HISTORY_LINES: usize = 16_384;

pub use cmux_proto::SessionStatus;

/// Waiting for the repaint a sent scroll asked for. The child answers
/// asynchronously, so a quiet frame right after sending means "nothing has
/// arrived yet", not "the repaint is done" — the two have to be told apart or
/// the screen matched is the one from before the scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StitchWait {
    /// When the scroll went out, so a child that ignores the wheel does not
    /// wedge the drag.
    pub sent_ms: u64,
    /// Whether any bytes have come back yet.
    pub answered: bool,
}

/// A selection in copy-buffer coordinates: a line index and a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragRange {
    pub anchor: (usize, u16),
    pub tip: (usize, u16),
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
    pub exit_status: Arc<Mutex<Option<String>>>,
}

/// Latest `Event::StatusUpdate` payload that hasn't yet been merged into the
/// owning `Session` struct. Drained on the next `Session::poll_status` tick.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    pub status: SessionStatus,
    pub label: Option<String>,
    pub attention: bool,
    pub rows: u16,
    pub cols: u16,
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
    pub status: SessionStatus,
    pub claude_name: Option<String>,
    pub attention: bool,
    pub manually_renamed: bool,
    pub selection: Option<TileSelection>,
    pub mouse_down_at: Option<(u16, u16)>,
    /// Output collected across scrolls while a selection is being dragged, so
    /// the selection can span more than the one screen the grid holds.
    pub copy: Option<crate::copy_buffer::CopyBuffer>,
    /// The selection in buffer lines, which is the authoritative one.
    /// `selection` is projected from it onto whatever is currently on screen.
    pub drag: Option<DragRange>,
    /// Where a sent scroll has got to, if one is outstanding.
    pub stitch: Option<StitchWait>,
    /// The edge a drag is being held against: `Some(true)` for the top.
    /// A pointer held still sends no further events, so the scroll has to
    /// repeat on the event loop's clock rather than on mouse motion.
    pub drag_edge: Option<bool>,
    /// When the last edge scroll was sent.
    pub last_edge_scroll_ms: u64,
    last_status_check_ms: u64,
    last_perm_check_active_ms: u64,
    daemon_pending_status: Option<Arc<Mutex<Option<PendingStatus>>>>,
    /// How the child ended, once it has. Set by the events reader for a daemon
    /// session and by `is_alive` for a local one.
    exit_status: Arc<Mutex<Option<String>>>,
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

        let argv = crate::claude_sessions::open_command(dangerous, resume.as_deref());
        let mut cmd = CommandBuilder::new(&argv[0]);
        for arg in &argv[1..] {
            cmd.arg(arg);
        }
        cmd.cwd(&cwd);
        for (k, v) in cmux_proto::terminal_spawn_env() {
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
            status: SessionStatus::Unknown,
            claude_name: None,
            attention: false,
            manually_renamed: false,
            selection: None,
            mouse_down_at: None,
            copy: None,
            drag: None,
            stitch: None,
            drag_edge: None,
            last_edge_scroll_ms: 0,
            last_status_check_ms: 0,
            last_perm_check_active_ms: 0,
            daemon_pending_status: None,
            exit_status: Arc::new(Mutex::new(None)),
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
        let exit_status: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let slot = DaemonSlot {
            parser: parser.clone(),
            byte_ring: byte_ring.clone(),
            dirty: dirty.clone(),
            alive: alive.clone(),
            last_active_ms: last_active_ms.clone(),
            pending_status: pending_status.clone(),
            exit_status: exit_status.clone(),
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
            status: SessionStatus::Unknown,
            claude_name: None,
            attention: false,
            manually_renamed: false,
            selection: None,
            mouse_down_at: None,
            copy: None,
            drag: None,
            stitch: None,
            drag_edge: None,
            last_edge_scroll_ms: 0,
            last_status_check_ms: 0,
            last_perm_check_active_ms: 0,
            daemon_pending_status: Some(pending_status),
            exit_status,
            backend: Backend::Daemon { remote_id, req_tx },
        };
        (s, slot)
    }

    #[allow(dead_code)]
    pub fn is_daemon_backed(&self) -> bool {
        matches!(self.backend, Backend::Daemon { .. })
    }

    /// Set the label, telling the daemon too when it owns the pty. Writing
    /// `self.label` alone leaves the terminal and the browser disagreeing
    /// about the session's name.
    pub fn set_label(&mut self, label: String) {
        self.label = label.clone();
        if let Backend::Daemon { remote_id, req_tx } = &self.backend {
            let _ = req_tx.send(ProtoRequest::Rename {
                session_id: *remote_id,
                label,
            });
        }
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
        if self.daemon_pending_status.is_some() {
            // Take it out before touching `self`, so the slot's borrow is done
            // by the time the grid is resized.
            let pending = self
                .daemon_pending_status
                .as_ref()
                .and_then(|slot| slot.lock().ok().and_then(|mut ps| ps.take()));
            if let Some(p) = pending {
                if self.status != p.status {
                    self.status = p.status;
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
                self.attention = p.attention;
                self.apply_effective_size(p.rows, p.cols);
            }
            return;
        }

        if let Some(record) = self.pid.and_then(cmux_proto::ClaudeSessionRecord::read) {
            if let Some(next) = record.status
                && self.status != next
            {
                self.status = next;
            }
            if let Some(n) = record.name
                && self.claude_name.as_deref() != Some(n.as_str())
            {
                if !self.manually_renamed {
                    self.label = n.clone();
                }
                self.claude_name = Some(n);
            }
        }

        let last_active = self.last_active_ms.load(Ordering::SeqCst);
        if last_active != self.last_perm_check_active_ms {
            self.attention = self.detect_permission_prompt();
            self.last_perm_check_active_ms = last_active;
        }
    }

    fn detect_permission_prompt(&self) -> bool {
        let Ok(p) = self.parser.lock() else {
            return false;
        };
        let text = term_render::visible_text(&p.term);
        let prompt = cmux_proto::is_permission_prompt(&text);
        // One line per scan. Appending the whole screen every time grew the
        // file without bound for as long as the session ran.
        debug_log!(
            "/tmp/cmux-scan.log",
            "id={} chars={} prompt={}",
            self.id,
            text.len(),
            prompt
        );
        prompt
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
                // Only a request. The pty runs at the smallest size among all
                // attached clients, and the grid is resized when the daemon
                // reports back what that turned out to be.
                let _ = req_tx.send(ProtoRequest::Resize {
                    session_id: *remote_id,
                    rows,
                    cols,
                });
                return Ok(());
            }
        }
        if let Ok(mut p) = self.parser.lock() {
            p.resize(rows, cols);
        }
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Adopt the size the pty is actually running at. Rendering a grid taller
    /// than the pty leaves the rows past its height showing whatever was drawn
    /// there before the shrink.
    pub fn apply_effective_size(&mut self, rows: u16, cols: u16) {
        use alacritty_terminal::grid::Dimensions;
        if rows == 0 || cols == 0 {
            return;
        }
        // One lock for the comparison and the resize. Reading the size under
        // one and resizing under another let two callers both see a mismatch
        // and both resize, each asking the daemon for a fresh repaint.
        let resized = match self.parser.lock() {
            Ok(mut p) => {
                let current = (
                    p.term.grid().screen_lines() as u16,
                    p.term.grid().columns() as u16,
                );
                if current == (rows, cols) {
                    false
                } else {
                    p.resize(rows, cols);
                    true
                }
            }
            Err(_) => return,
        };
        if !resized {
            return;
        }
        // Ask for a repaint at the new size. The program may have already
        // drawn before this client knew, or may not redraw at all, and either
        // way the grid that was just resized holds reflowed leftovers.
        if let Backend::Daemon { remote_id, req_tx } = &self.backend {
            let _ = req_tx.send(ProtoRequest::Attach {
                session_id: *remote_id,
                want_history: false,
            });
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// How the child ended, once it has. `None` while it is still running.
    pub fn exit_status(&self) -> Option<String> {
        self.exit_status.lock().ok().and_then(|s| s.clone())
    }

    pub fn is_alive(&mut self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match &mut self.backend {
            Backend::Local { child, .. } => match child.try_wait() {
                Ok(Some(st)) => {
                    self.alive.store(false, Ordering::SeqCst);
                    if let Ok(mut slot) = self.exit_status.lock()
                        && slot.is_none()
                    {
                        *slot = Some(if st.success() {
                            "exited 0".to_string()
                        } else {
                            format!("exited {}", st.exit_code())
                        });
                    }
                    false
                }
                _ => true,
            },
            Backend::Daemon { .. } => true,
        }
    }

    /// Hard-kill: ends the underlying process. For daemon mode this sends
    /// `Detach { keep_session: false }`, so the session goes away for every
    /// client rather than only this one.
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

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
