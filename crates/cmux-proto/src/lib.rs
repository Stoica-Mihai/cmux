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
/// at rather than the one it asked for. A stale client is rejected at the
/// handshake rather than left to fail on a decode.
pub const PROTOCOL_VERSION: u32 = 4;

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

/// argv for a Claude Code session. Single source of truth for the flags, so
/// the local-PTY and daemon paths cannot drift apart.
pub fn claude_command(dangerous: bool, resume_id: Option<&str>) -> Vec<String> {
    let mut cmd = vec!["claude".to_string()];
    if dangerous {
        cmd.push("--dangerously-skip-permissions".to_string());
    }
    if let Some(id) = resume_id {
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
mod tests {
    use super::*;
    use std::io::Cursor;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + std::fmt::Debug>(msg: T) {
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &msg).expect("write");
        let mut cur = Cursor::new(&buf);
        let back: T = read_frame(&mut cur).expect("read");
        assert_eq!(format!("{:?}", msg), format!("{:?}", back));
    }

    #[test]
    fn request_hello() {
        round_trip(Request::Hello {
            client_version: "0.1.0".into(),
            want_protocol: PROTOCOL_VERSION,
        });
    }

    #[test]
    fn request_spawn_generic_command() {
        round_trip(Request::SpawnSession {
            cwd: PathBuf::from("/tmp"),
            cmd: vec!["bash".into(), "-l".into()],
            probe: ProbeKind::None,
            label: Some("shell".into()),
            rows: 40,
            cols: 120,
        });
    }

    #[test]
    fn request_spawn_claude_probe() {
        round_trip(Request::SpawnSession {
            cwd: PathBuf::from("/tmp"),
            cmd: claude_command(true, Some("abc")),
            probe: ProbeKind::Claude {
                dangerous: true,
                resume_id: Some("abc".into()),
            },
            label: Some("dev".into()),
            rows: 24,
            cols: 80,
        });
    }

    #[test]
    fn request_input_bytes() {
        round_trip(Request::Input {
            session_id: 7,
            bytes: vec![0x1b, b'[', b'A'],
        });
    }

    #[test]
    fn event_welcome() {
        round_trip(Event::Welcome {
            server_version: "0.1.0".into(),
            protocol: PROTOCOL_VERSION,
            session_count: 3,
        });
    }

    #[test]
    fn event_framedelta() {
        round_trip(Event::FrameDelta {
            id: 1,
            bytes: vec![1, 2, 3, 4, 5],
        });
    }

    #[test]
    fn envelope_round_trip() {
        round_trip(Envelope {
            id: Some(42),
            body: Request::ListSessions,
        });
    }

    #[test]
    fn truncated_frame_is_eof() {
        let mut cur = Cursor::new(Vec::<u8>::new());
        let err = read_frame::<_, Request>(&mut cur).unwrap_err();
        assert!(matches!(err, FrameError::Eof));
    }

    #[test]
    fn oversize_frame_rejected_on_read() {
        let mut buf = Vec::new();
        let huge_len = MAX_FRAME_BYTES + 1;
        buf.extend_from_slice(&huge_len.to_le_bytes());
        let mut cur = Cursor::new(buf);
        let err = read_frame::<_, Request>(&mut cur).unwrap_err();
        assert!(matches!(err, FrameError::TooLarge(_)));
    }

    #[test]
    fn claude_command_matches_the_flags_it_is_given() {
        assert_eq!(claude_command(false, None), vec!["claude"]);
        assert_eq!(
            claude_command(true, None),
            vec!["claude", "--dangerously-skip-permissions"]
        );
        assert_eq!(
            claude_command(false, Some("s1")),
            vec!["claude", "--resume", "s1"]
        );
        assert_eq!(
            claude_command(true, Some("s1")),
            vec!["claude", "--dangerously-skip-permissions", "--resume", "s1"]
        );
    }

    #[test]
    fn probe_accessors_read_through_the_claude_variant() {
        let claude = ProbeKind::Claude {
            dangerous: true,
            resume_id: Some("s1".into()),
        };
        assert!(claude.dangerous());
        assert_eq!(claude.resume_id(), Some("s1"));

        let tame = ProbeKind::Claude {
            dangerous: false,
            resume_id: None,
        };
        assert!(!tame.dangerous());
        assert_eq!(tame.resume_id(), None);

        assert!(!ProbeKind::None.dangerous());
        assert_eq!(ProbeKind::None.resume_id(), None);
    }

    #[test]
    fn session_status_parses_the_claude_json_field() {
        assert_eq!(SessionStatus::from_status_str("busy"), SessionStatus::Busy);
        assert_eq!(SessionStatus::from_status_str("idle"), SessionStatus::Idle);
        assert_eq!(
            SessionStatus::from_status_str("something-else"),
            SessionStatus::Unknown
        );
    }

    // terminal_spawn_env reads $TERM_PROGRAM, $TMUX, …, mutating the process
    // env in tests would race with parallel tests in this crate. Instead the
    // test asserts canonical-override behavior using whatever the host env
    // happens to be: TERM/COLORTERM must always equal the cmux defaults, the
    // strip list must never appear in the output regardless of what was set,
    // and unrelated vars must pass through.
    #[test]
    fn terminal_spawn_env_overrides_term_and_strips_listed() {
        let env: std::collections::HashMap<String, String> =
            terminal_spawn_env().into_iter().collect();

        // Canonical overrides — no matter what TERM/COLORTERM were set to in
        // the host env, cmux forces these values.
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));

        // Strip list — these must never appear in the spawn env even if the
        // host has them set. Pick a representative subset.
        for name in &[
            "TERM_PROGRAM",
            "TMUX",
            "WT_SESSION",
            "ITERM_SESSION_ID",
            "KITTY_WINDOW_ID",
        ] {
            assert!(
                !env.contains_key(*name),
                "{name} should be stripped from the spawn env"
            );
        }

        // Pass-through smoke: PATH is essentially always set by any shell that
        // ran cargo test, and PATH is not on the strip list.
        if std::env::var("PATH").is_ok() {
            assert!(env.contains_key("PATH"));
        }
    }
}
