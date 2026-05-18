use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::debug_log;
use crate::term_render::{self, TermSize};
use crate::util::{claude_sessions_dir, now_ms};

const SCROLLBACK_LINES: usize = 4096;

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
    #[allow(dead_code)]
    pub created_ms: u64,
    pub pid: Option<u32>,
    pub claude_status: String,
    pub claude_name: Option<String>,
    pub permission_pending: bool,
    pub manually_renamed: bool,
    last_status_check_ms: u64,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    _reader_thread: JoinHandle<()>,
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
        for (k, v) in std::env::vars() {
            cmd.env(k, v);
        }
        if std::env::var_os("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }

        let child = pair.slave.spawn_command(cmd).context("spawn claude")?;
        let killer = child.clone_killer();
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().context("clone reader")?;
        let writer = pair.master.take_writer().context("take writer")?;
        let parser = Arc::new(Mutex::new(TerminalState::new(rows, cols)));
        let alive = Arc::new(AtomicBool::new(true));
        let spawn_ms = now_ms();
        let last_active_ms = Arc::new(AtomicU64::new(spawn_ms));

        let reader_thread = {
            let parser = parser.clone();
            let alive = alive.clone();
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
                                last_active.store(now_ms(), Ordering::SeqCst);
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
            created_ms: spawn_ms,
            pid,
            claude_status: String::new(),
            claude_name: None,
            permission_pending: false,
            manually_renamed: false,
            last_status_check_ms: 0,
            master: pair.master,
            writer,
            child,
            killer,
            _reader_thread: reader_thread,
        })
    }

    pub fn poll_status(&mut self) {
        let now = now_ms();
        if now.saturating_sub(self.last_status_check_ms) < 500 {
            return;
        }
        self.last_status_check_ms = now;

        if let Some(pid) = self.pid {
            let path = claude_sessions_dir().map(|d| d.join(format!("{}.json", pid)));
            if let Some(path) = path
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
            {
                if let Some(s) = v.get("status").and_then(|x| x.as_str()) {
                    self.claude_status = s.to_string();
                }
                if let Some(n) = v.get("name").and_then(|x| x.as_str()) {
                    let n = n.to_string();
                    if !self.manually_renamed && !n.is_empty() && self.claude_name.as_deref() != Some(&n) {
                        self.label = n.clone();
                    }
                    self.claude_name = Some(n);
                }
            }
        }

        self.permission_pending = self.detect_permission_prompt();
    }

    fn detect_permission_prompt(&self) -> bool {
        let Ok(p) = self.parser.lock() else { return false };
        let text = term_render::visible_text(&p.term);
        debug_log!(
            &format!("/tmp/cmux-screen-{}.txt", self.id),
            "\n========= scan at id={} =========\n{}\n",
            self.id,
            text
        );
        let lower = text.to_lowercase();
        lower.contains("do you want to proceed")
            || lower.contains("allow this")
            || lower.contains("apply this edit")
            || lower.contains("requires approval")
            || lower.contains("don't ask again")
            || lower.contains("esc to cancel")
            || (lower.contains("1. yes") && lower.contains("3. no"))
            || (lower.contains("1. yes") && lower.contains("2. no"))
    }

    pub fn activity_age_ms(&self) -> u64 {
        now_ms().saturating_sub(self.last_active_ms.load(Ordering::SeqCst))
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        if (rows, cols) == self.size {
            return Ok(());
        }
        self.size = (rows, cols);
        self.master
            .resize(pty_size(rows, cols))
            .context("resize pty")?;
        if let Ok(mut p) = self.parser.lock() {
            p.resize(rows, cols);
        }
        Ok(())
    }

    pub fn is_alive(&mut self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match self.child.try_wait() {
            Ok(Some(_)) => {
                self.alive.store(false, Ordering::SeqCst);
                false
            }
            _ => true,
        }
    }

    #[allow(dead_code)]
    pub fn exit_status(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(format!("{}", status)),
            _ => None,
        }
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
        self.alive.store(false, Ordering::SeqCst);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}
