//! Wire types and framing for the cmux ↔ cmuxd protocol.
//!
//! See `DAEMON_PLAN.md` §6 for protocol semantics.
//!
//! The daemon hosts an arbitrary command per session. Anything claude-specific
//! travels in [`ProbeKind`], which tells the daemon how to derive status for
//! that session — the daemon itself only ever execs [`Request::SpawnSession`]'s
//! `cmd`.

#![deny(unsafe_code)]

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 2 made `SpawnSession` command-generic; 3 dropped the `Snapshot` event,
/// which was always sent empty and never read; 4 put the effective grid size
/// on `StatusUpdate`, so a client renders at the size the pty actually runs
/// at rather than the one it asked for; 5 reports whether the child is still
/// alive, which nothing could see before. A stale client is rejected at the
/// handshake rather than left to fail on a decode.
pub const PROTOCOL_VERSION: u32 = 5;

/// Environment variables exported by outer terminals that misadvertise the
/// host capabilities to the child. cmux IS the terminal the child sees; strip
/// these so it doesn't try kitty-graphics / iTerm imgcat / tmux-passthrough
/// escapes that the alacritty parser silently drops. Shared between cmux (local
/// PTY) and cmuxd (daemon PTY) so the spawn environment matches byte-for-byte.
pub const TERMINAL_ENV_STRIP: &[&str] = &[
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

/// Apply [`TERMINAL_ENV_STRIP`] to the current process environment and overlay
/// canonical TERM + COLORTERM. Returns the (key, value) pairs callers should
/// install on their spawn command — both `portable-pty` and `std::process`
/// expose an `env(k, v)` method so this helper stays framework-agnostic.
pub fn terminal_spawn_env() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| !TERMINAL_ENV_STRIP.contains(&k.as_str()))
        .collect();
    // Overwrite any TERM/COLORTERM that survived (some shells preset these).
    out.retain(|(k, _)| k != "TERM" && k != "COLORTERM");
    out.push(("TERM".into(), "xterm-256color".into()));
    out.push(("COLORTERM".into(), "truecolor".into()));
    out
}

/// What a Claude Code session opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch<'a> {
    /// A fresh conversation.
    New,
    /// A stored conversation, by session id.
    Resume(&'a str),
    /// A session claude is already running in the background, by the short id
    /// `claude agents` lists.
    Attach(&'a str),
}

/// argv for a Claude Code session. Single source of truth for the flags, so
/// the local-PTY and daemon paths cannot drift apart. `claude attach` accepts
/// no options, so the dangerous flag is dropped on that form.
pub fn claude_command(dangerous: bool, launch: Launch<'_>) -> Vec<String> {
    if let Launch::Attach(job_id) = launch {
        return vec![
            "claude".to_string(),
            "attach".to_string(),
            job_id.to_string(),
        ];
    }
    let mut cmd = vec!["claude".to_string()];
    if dangerous {
        cmd.push("--dangerously-skip-permissions".to_string());
    }
    if let Launch::Resume(id) = launch {
        cmd.push("--resume".to_string());
        cmd.push(id.to_string());
    }
    cmd
}

// ---------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SessionStatus {
    #[default]
    Unknown,
    Busy,
    Idle,
}

impl SessionStatus {
    /// Parse the `status` field of a claude session JSON file.
    pub fn from_status_str(s: &str) -> Self {
        match s {
            "busy" => Self::Busy,
            "idle" => Self::Idle,
            _ => Self::Unknown,
        }
    }
}

/// How the daemon should derive status for a session. `None` leaves the
/// session as a plain PTY with no introspection and no polling timer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ProbeKind {
    #[default]
    None,
    /// Claude Code: reads `~/.claude/sessions/<pid>.json` for status and name,
    /// and scans the grid for permission prompts. `dangerous` and `resume_id`
    /// describe how the session was started, for display only — the flags
    /// themselves already live in the session's `cmd`.
    Claude {
        dangerous: bool,
        resume_id: Option<String>,
    },
}

impl ProbeKind {
    pub fn dangerous(&self) -> bool {
        matches!(
            self,
            Self::Claude {
                dangerous: true,
                ..
            }
        )
    }

    pub fn resume_id(&self) -> Option<&str> {
        match self {
            Self::Claude { resume_id, .. } => resume_id.as_deref(),
            Self::None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollOp {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u64,
    pub label: String,
    pub cwd: PathBuf,
    /// argv the daemon exec'd, `cmd[0]` being the program.
    pub cmd: Vec<String>,
    pub probe: ProbeKind,
    pub rows: u16,
    pub cols: u16,
    pub spawned_at_ms: u64,
    pub last_active_ms: u64,
    pub status: SessionStatus,
    /// The session wants the user to look at it (claude: a permission prompt).
    pub attention: bool,
    /// False once the child has exited. Nothing reported this before, so a
    /// crashed session went on being listed as running for ever.
    pub alive: bool,
    /// How it ended, once it has.
    pub exit_status: Option<String>,
}

// ---------------------------------------------------------------------------
// Request / Event enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Request {
    Hello {
        client_version: String,
        want_protocol: u32,
    },
    ListSessions,
    SpawnSession {
        cwd: PathBuf,
        /// argv to exec. Must be non-empty; the daemon rejects an empty one.
        cmd: Vec<String>,
        probe: ProbeKind,
        label: Option<String>,
        rows: u16,
        cols: u16,
    },
    Attach {
        session_id: u64,
        want_history: bool,
    },
    Detach {
        session_id: u64,
        keep_session: bool,
    },
    Input {
        session_id: u64,
        bytes: Vec<u8>,
    },
    Resize {
        session_id: u64,
        rows: u16,
        cols: u16,
    },
    Rename {
        session_id: u64,
        label: String,
    },
    Scroll {
        session_id: u64,
        op: ScrollOp,
    },
    Subscribe {
        session_id: u64,
    },
    Unsubscribe {
        session_id: u64,
    },
    Shutdown,
    KillAll,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Event {
    Welcome {
        server_version: String,
        protocol: u32,
        session_count: usize,
    },
    SessionList {
        sessions: Vec<SessionInfo>,
    },
    SessionSpawned {
        id: u64,
        info: SessionInfo,
    },
    SessionExited {
        id: u64,
        status: String,
    },
    FrameDelta {
        id: u64,
        bytes: Vec<u8>,
    },
    StatusUpdate {
        id: u64,
        status: SessionStatus,
        label: Option<String>,
        attention: bool,
        /// The size the pty is actually running at, which is the minimum over
        /// every attached client. A client that renders at its own requested
        /// size instead leaves the rows beyond this one holding stale output.
        rows: u16,
        cols: u16,
    },
    /// Daemon → client: replay your snapshot, you lagged.
    Resync {
        id: u64,
    },
    Error {
        request_id: Option<u64>,
        message: String,
    },
    Goodbye {
        reason: String,
    },
}

/// Outer envelope so requests get an optional id for correlating responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub id: Option<u64>,
    pub body: T,
}

// ---------------------------------------------------------------------------
// Framing: u32_le length prefix + JSON payload
// ---------------------------------------------------------------------------

/// Maximum payload length the codec will accept (8 MiB). Sized to leave room
/// for Snapshot variants without ballooning unbounded.
/// One of claude's own session records, `~/.claude/sessions/<pid>.json`.
/// claude writes one per live process; three places in cmux read it, so the
/// field names live here rather than in each of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSessionRecord {
    pub pid: u32,
    /// Start time in clock ticks, matched against `/proc/<pid>/stat` to tell a
    /// live process from a reused pid.
    pub proc_start: Option<String>,
    pub session_id: Option<String>,
    /// The short id `claude attach` and `claude stop` take.
    pub job_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<SessionStatus>,
    /// `"kind":"bg"`: claude runs the conversation with no terminal attached.
    pub background: bool,
}

impl ClaudeSessionRecord {
    /// A record is usable once it names a pid.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let v = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
        let text = |key: &str| {
            v.get(key)
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            pid: v.get("pid").and_then(|x| x.as_u64())? as u32,
            proc_start: text("procStart"),
            session_id: text("sessionId"),
            job_id: text("jobId"),
            name: text("name"),
            status: text("status").map(|s| SessionStatus::from_status_str(&s)),
            background: v.get("kind").and_then(|x| x.as_str()) == Some("bg"),
        })
    }

    /// Read the record a pid would have written.
    pub fn read(pid: u32) -> Option<Self> {
        Self::parse(&std::fs::read(claude_session_path(pid)?).ok()?)
    }

    /// Whether the process that wrote this record is still running. The pid
    /// alone is not enough, because the kernel reuses pids; the recorded start
    /// time settles it.
    pub fn is_live(&self) -> bool {
        match (self.proc_start.as_deref(), proc_start_of(self.pid)) {
            (Some(recorded), Some(actual)) => recorded == actual,
            (None, Some(_)) => true,
            _ => false,
        }
    }
}

/// Field 22 of `/proc/<pid>/stat`, the process start time in clock ticks. The
/// comm field before it may hold spaces and parentheses, so the split is on
/// its closing one.
fn proc_start_of(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(19).map(str::to_string)
}

/// Where claude keeps its session records.
pub fn claude_sessions_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join(".claude")
            .join("sessions"),
    )
}

/// The record file a given pid writes.
pub fn claude_session_path(pid: u32) -> Option<std::path::PathBuf> {
    Some(claude_sessions_dir()?.join(format!("{pid}.json")))
}

/// Raw PTY history kept per session, in bytes. The daemon holds the
/// authoritative ring and a client mirrors it, so both ends size it the same.
pub const RING_BYTES_CAP: usize = 1_048_576;

/// Lines of grid history a session's terminal keeps.
pub const SCROLLBACK_LINES: usize = 4096;

/// On-screen phrasings that mean claude is waiting on a permission answer.
const PROMPT_NEEDLES: &[&str] = &[
    "do you want to proceed",
    "allow this",
    "apply this edit",
    "requires approval",
    "don't ask again",
    "esc to cancel",
];

/// Whether a session's visible text is asking the user for permission. Both
/// the daemon's probe and the local-PTY path ask this of their own grid, so
/// the phrasings and the numbered-choice fallback live here rather than in
/// each of them.
pub fn is_permission_prompt(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    if PROMPT_NEEDLES.iter().any(|n| lower.contains(n)) {
        return true;
    }
    lower.contains("1. yes") && (lower.contains("2. no") || lower.contains("3. no"))
}

pub const MAX_FRAME_BYTES: u32 = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    #[error("frame too large: {0} > {max}", max = MAX_FRAME_BYTES)]
    TooLarge(u32),
    #[error("malformed JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("connection closed")]
    Eof,
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    let payload = serde_json::to_vec(msg)?;
    if payload.len() as u64 > MAX_FRAME_BYTES as u64 {
        return Err(FrameError::TooLarge(payload.len() as u32));
    }
    let len = payload.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&payload)?;
    w.flush()?;
    Ok(())
}

pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<T, FrameError> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Eof),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    let msg = serde_json::from_slice(&payload)?;
    Ok(msg)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests/proto.rs"]
mod tests;
