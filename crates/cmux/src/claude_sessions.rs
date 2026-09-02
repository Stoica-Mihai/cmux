//! Claude Code's own session records, one JSON file per live process under
//! `~/.claude/sessions/<pid>.json`.
//!
//! A record with `"kind":"bg"` is a background session: claude runs the
//! conversation with no terminal attached. Such a session opens with
//! `claude attach <short id>`. `claude --resume <session id>` exits with an
//! error naming that form.
//!
//! The record names the session but not where it came from. That sits in
//! `~/.claude/jobs/<short id>/state.json`, alongside the `intent` a forked
//! session was started for and the parent it was forked from.

use std::collections::HashMap;

use cmux_proto::{ClaudeSessionRecord, Launch};

use cmux_proto::claude_sessions_dir;

use crate::util::claude_jobs_dir;

/// A conversation claude is running in the background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Background {
    /// The short id `claude attach` and `claude stop` take.
    pub job_id: String,
    /// The session's display name, when it has one. For a session claude named
    /// itself, this is the purpose it was started for rather than claude's
    /// composite of parent name and purpose.
    pub name: Option<String>,
    /// Session this one was forked from, for a forked session.
    pub forked_from: Option<String>,
}

/// The naming and fork fields of `~/.claude/jobs/<short id>/state.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct JobState {
    name: Option<String>,
    /// What a forked session was started for. claude composes an automatic
    /// name as `<parent name> <fork glyph> <intent>`.
    intent: Option<String>,
    /// `peer` and `user` name a person's choice, `auto` claude's own.
    name_source: Option<String>,
    forked_from: Option<String>,
}

impl JobState {
    /// The purpose alone for a name claude composed, the recorded name
    /// otherwise.
    fn display_name(&self) -> Option<String> {
        if self.name_source.as_deref() == Some("auto")
            && let Some(intent) = &self.intent
        {
            return Some(intent.clone());
        }
        self.name.clone()
    }
}

/// argv for opening a stored conversation: `claude attach <short id>` while
/// claude runs it in the background, `claude --resume <session id>` otherwise.
pub fn open_command(dangerous: bool, resume: Option<&str>) -> Vec<String> {
    let Some(session_id) = resume else {
        return cmux_proto::claude_command(dangerous, Launch::New);
    };
    match live_background().get(session_id) {
        Some(bg) => cmux_proto::claude_command(dangerous, Launch::Attach(&bg.job_id)),
        None => cmux_proto::claude_command(dangerous, Launch::Resume(session_id)),
    }
}

/// Live background sessions, keyed by session id. A record whose process has
/// exited, or whose pid now belongs to something else, is left out.
pub fn live_background() -> HashMap<String, Background> {
    let mut out = HashMap::new();
    let Some(dir) = claude_sessions_dir() else {
        return out;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(rec) = ClaudeSessionRecord::parse(&bytes) else {
            continue;
        };
        if !rec.background || !rec.is_live() {
            continue;
        }
        let (Some(session_id), Some(job_id)) = (rec.session_id, rec.job_id) else {
            continue;
        };
        let job = read_job_state(&job_id).unwrap_or_default();
        out.insert(
            session_id,
            Background {
                name: job.display_name().or(rec.name),
                forked_from: job.forked_from,
                job_id,
            },
        );
    }
    out
}

/// The job state a short id points at.
fn read_job_state(job_id: &str) -> Option<JobState> {
    let path = claude_jobs_dir()?.join(job_id).join("state.json");
    let bytes = std::fs::read(&path).ok()?;
    parse_job_state(&bytes)
}

fn parse_job_state(bytes: &[u8]) -> Option<JobState> {
    let v = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    let str_field = |key: &str| {
        v.get(key)
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(JobState {
        name: str_field("name"),
        intent: str_field("intent"),
        name_source: str_field("nameSource"),
        forked_from: str_field("forkParentSessionId"),
    })
}

#[cfg(test)]
#[path = "tests/claude_sessions.rs"]
mod tests;
