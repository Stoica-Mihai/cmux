//! Per-session status probes.
//!
//! The daemon hosts an arbitrary command, so anything it can say about a
//! session beyond "bytes moved" comes from a probe. [`build`] returns `None`
//! for [`ProbeKind::None`], and the caller then skips the polling timer
//! entirely rather than ticking a probe that reports nothing.

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::cell::Flags;
use cmux_proto::{ProbeKind, SessionStatus};

const PROMPT_NEEDLES: &[&str] = &[
    "do you want to proceed",
    "allow this",
    "apply this edit",
    "requires approval",
    "don't ask again",
    "esc to cancel",
];

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
        ProbeKind::Claude { .. } => Some(Box::new(ClaudeProbe)),
    }
}

/// Reads `~/.claude/sessions/<pid>.json` for status and name, and scans the
/// visible grid for a permission prompt.
pub struct ClaudeProbe;

impl StatusProbe for ClaudeProbe {
    fn poll(&self, ctx: &ProbeCtx<'_>) -> ProbeOutcome {
        let mut out = ProbeOutcome {
            attention: Some(scan_permission_prompt(ctx.term)),
            ..Default::default()
        };
        let (Some(pid), Some(home)) = (ctx.pid, std::env::var_os("HOME")) else {
            return out;
        };
        let path = std::path::PathBuf::from(home)
            .join(".claude")
            .join("sessions")
            .join(format!("{}.json", pid));
        let Ok(bytes) = std::fs::read(&path) else {
            return out;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return out;
        };
        if let Some(s) = v.get("status").and_then(|x| x.as_str()) {
            out.status = Some(SessionStatus::from_status_str(s));
        }
        if let Some(n) = v.get("name").and_then(|x| x.as_str())
            && !n.is_empty()
        {
            out.label = Some(n.to_string());
        }
        out
    }
}

/// Flatten the visible grid to text, one line per row.
pub fn grid_text(term: &Term<VoidListener>) -> String {
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
    text
}

fn scan_permission_prompt(term: &Term<VoidListener>) -> bool {
    let lower = grid_text(term).to_lowercase();
    if PROMPT_NEEDLES.iter().any(|n| lower.contains(n)) {
        return true;
    }
    let has_yes = lower.contains("1. yes");
    has_yes && (lower.contains("2. no") || lower.contains("3. no"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::Config as TermConfig;
    use alacritty_terminal::vte::ansi::Processor;

    use crate::session::TermSize;

    fn term_showing(text: &str) -> Term<VoidListener> {
        let size = TermSize {
            lines: 24,
            cols: 80,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut proc: Processor = Processor::new();
        proc.advance(&mut term, text.replace('\n', "\r\n").as_bytes());
        term
    }

    #[test]
    fn detects_a_permission_prompt() {
        assert!(scan_permission_prompt(&term_showing(
            "Edit file src/main.rs?\nDo you want to proceed?"
        )));
        assert!(scan_permission_prompt(&term_showing(
            "Run this command?\n1. Yes\n2. No, and tell Claude what to do"
        )));
    }

    #[test]
    fn leaves_ordinary_output_alone() {
        assert!(!scan_permission_prompt(&term_showing(
            "$ cargo build\n   Compiling cmuxd v0.1.0\n    Finished in 4.63s"
        )));
        // "1. Yes" alone is a list item, not a prompt.
        assert!(!scan_permission_prompt(&term_showing(
            "Checklist:\n1. Yes it compiles\n2. Ship it"
        )));
    }

    #[test]
    fn build_gives_no_probe_for_a_plain_pty() {
        assert!(build(&ProbeKind::None).is_none());
        assert!(
            build(&ProbeKind::Claude {
                dangerous: false,
                resume_id: None,
            })
            .is_some()
        );
    }

    #[test]
    fn claude_probe_reports_attention_even_with_no_pid() {
        let term = term_showing("Do you want to proceed?");
        let outcome = ClaudeProbe.poll(&ProbeCtx {
            pid: None,
            term: &term,
        });
        assert_eq!(outcome.attention, Some(true));
        assert_eq!(outcome.status, None);
        assert_eq!(outcome.label, None);
    }
}
