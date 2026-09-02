//! Per-session status probes.
//!
//! The daemon hosts an arbitrary command, so anything it can say about a
//! session beyond "bytes moved" comes from a probe. [`build`] returns `None`
//! for [`ProbeKind::None`], and the caller then skips the polling timer
//! entirely rather than ticking a probe that reports nothing.

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use cmux_proto::{ClaudeSessionRecord, ProbeKind, SessionStatus};

pub struct ProbeCtx<'a> {
    pub pid: Option<u32>,
    pub term: &'a Term<VoidListener>,
}

/// What a probe observed. Every field is optional so a probe can report only
/// what it actually knows and leave the rest of the session state alone.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub status: Option<SessionStatus>,
    pub label: Option<String>,
    pub attention: Option<bool>,
}

pub trait StatusProbe: Send + Sync {
    fn poll(&self, ctx: &ProbeCtx<'_>) -> ProbeOutcome;
}

pub fn build(kind: &ProbeKind) -> Option<Box<dyn StatusProbe>> {
    match kind {
        ProbeKind::None => None,
        ProbeKind::Claude { resume_id, .. } => Some(Box::new(ClaudeProbe::new(resume_id.clone()))),
    }
}

/// Reads `~/.claude/sessions/<pid>.json` for status and name, and scans the
/// visible grid for a permission prompt.
///
/// `claude attach <short id>` runs the conversation in another process, so the
/// child has no record of its own. The conversation's own record is found by
/// its session id instead, and the pid it names is remembered so later ticks
/// read one file rather than the directory.
pub struct ClaudeProbe {
    session_id: Option<String>,
    holder_pid: std::sync::Mutex<Option<u32>>,
}

impl ClaudeProbe {
    fn new(session_id: Option<String>) -> Self {
        Self {
            session_id,
            holder_pid: std::sync::Mutex::new(None),
        }
    }

    /// The pid whose record carries this conversation, preferring the child's
    /// own and falling back to whichever process holds the session id.
    fn record_pid(&self, child: Option<u32>) -> Option<u32> {
        if let Some(pid) = child
            && ClaudeSessionRecord::read(pid).is_some()
        {
            return Some(pid);
        }
        let session_id = self.session_id.as_deref()?;
        if let Ok(cached) = self.holder_pid.lock()
            && let Some(pid) = *cached
            && record_names(pid, session_id)
        {
            return Some(pid);
        }
        let found = find_holder(session_id)?;
        if let Ok(mut cached) = self.holder_pid.lock() {
            *cached = Some(found);
        }
        Some(found)
    }
}

/// Whether a pid's record is still the one for this conversation.
fn record_names(pid: u32, session_id: &str) -> bool {
    ClaudeSessionRecord::read(pid)
        .and_then(|r| r.session_id)
        .is_some_and(|s| s == session_id)
}

/// The pid of the process holding a conversation, by session id.
fn find_holder(session_id: &str) -> Option<u32> {
    for entry in std::fs::read_dir(cmux_proto::claude_sessions_dir()?)
        .ok()?
        .flatten()
    {
        let Some(r) = ClaudeSessionRecord::parse(&std::fs::read(entry.path()).ok()?) else {
            continue;
        };
        if r.session_id.as_deref() == Some(session_id) && r.is_live() {
            return Some(r.pid);
        }
    }
    None
}

impl StatusProbe for ClaudeProbe {
    fn poll(&self, ctx: &ProbeCtx<'_>) -> ProbeOutcome {
        let mut out = ProbeOutcome {
            attention: Some(scan_permission_prompt(ctx.term)),
            ..Default::default()
        };
        let Some(record) = self.record_pid(ctx.pid).and_then(ClaudeSessionRecord::read) else {
            return out;
        };
        out.status = record.status;
        out.label = record.name;
        out
    }
}

fn scan_permission_prompt(term: &Term<VoidListener>) -> bool {
    cmux_proto::is_permission_prompt(&cmux_term::grid_text(term))
}

#[cfg(test)]
#[path = "tests/probe.rs"]
mod tests;
