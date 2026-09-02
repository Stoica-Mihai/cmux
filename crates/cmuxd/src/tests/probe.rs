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
    let outcome = ClaudeProbe::new(None).poll(&ProbeCtx {
        pid: None,
        term: &term,
    });
    assert_eq!(outcome.attention, Some(true));
    assert_eq!(outcome.status, None);
    assert_eq!(outcome.label, None);
}

/// `claude attach` runs the conversation elsewhere, so the child pid has no
/// record. The probe then looks the record up by session id, and reports
/// nothing at all when no process holds that conversation.
#[test]
fn an_unheld_session_id_reports_no_name() {
    let term = term_showing("nothing interesting");
    let outcome =
        ClaudeProbe::new(Some("00000000-0000-0000-0000-000000000000".into())).poll(&ProbeCtx {
            pid: None,
            term: &term,
        });
    assert_eq!(outcome.label, None);
    assert_eq!(outcome.status, None);
    assert_eq!(outcome.attention, Some(false));
}

/// The probe reads whichever process holds the conversation, which for an
/// attached session is not the child it was given.
#[test]
fn a_held_session_id_is_read_from_the_holder() {
    let holder = std::process::id();
    let session_id = format!("cmux-probe-test-{holder}");
    let dir = match std::env::var_os("HOME") {
        Some(h) => std::path::PathBuf::from(h).join(".claude").join("sessions"),
        None => return,
    };
    if !dir.is_dir() {
        return;
    }
    let path = dir.join(format!("{holder}.json"));
    let record = format!(
        r#"{{"pid":{holder},"sessionId":"{session_id}","name":"probe-test-name","status":"idle"}}"#
    );
    if std::fs::write(&path, record).is_err() {
        return;
    }

    let term = term_showing("nothing interesting");
    let outcome = ClaudeProbe::new(Some(session_id)).poll(&ProbeCtx {
        pid: None,
        term: &term,
    });
    let _ = std::fs::remove_file(&path);

    assert_eq!(outcome.label.as_deref(), Some("probe-test-name"));
    assert_eq!(outcome.status, Some(SessionStatus::Idle));
}

/// The other half of the same call: a quiet screen must clear attention,
/// not merely leave it unset, or the badge sticks once it has been raised.
#[test]
fn a_quiet_screen_clears_attention_rather_than_leaving_it_unsaid() {
    let term = term_showing("$ cargo build\n    Finished in 4.63s");
    let outcome = ClaudeProbe::new(None).poll(&ProbeCtx {
        pid: None,
        term: &term,
    });
    assert_eq!(
        outcome.attention,
        Some(false),
        "a quiet screen should say so, not stay silent"
    );
}

#[test]
fn a_numbered_prompt_needs_both_of_its_choices() {
    assert!(scan_permission_prompt(&term_showing(
        "1. Yes\n3. No thanks"
    )));
    assert!(
        !scan_permission_prompt(&term_showing("2. No, not this one")),
        "a lone no is not a prompt"
    );
}

#[test]
fn an_empty_screen_is_not_a_prompt() {
    assert!(!scan_permission_prompt(&term_showing("")));
}
