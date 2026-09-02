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
        cmd: claude_command(true, Launch::Resume("abc")),
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
    assert_eq!(claude_command(false, Launch::New), vec!["claude"]);
    assert_eq!(
        claude_command(true, Launch::New),
        vec!["claude", "--dangerously-skip-permissions"]
    );
    assert_eq!(
        claude_command(false, Launch::Resume("s1")),
        vec!["claude", "--resume", "s1"]
    );
    assert_eq!(
        claude_command(true, Launch::Resume("s1")),
        vec!["claude", "--dangerously-skip-permissions", "--resume", "s1"]
    );
}

#[test]
fn attaching_runs_the_subcommand_and_carries_no_flags() {
    assert_eq!(
        claude_command(false, Launch::Attach("acd5a98f")),
        vec!["claude", "attach", "acd5a98f"]
    );
    assert_eq!(
        claude_command(true, Launch::Attach("acd5a98f")),
        vec!["claude", "attach", "acd5a98f"]
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
    let env: std::collections::HashMap<String, String> = terminal_spawn_env().into_iter().collect();

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

#[test]
fn a_named_permission_phrasing_is_a_prompt() {
    for screen in [
        "Do you want to proceed?",
        "  ALLOW THIS  ",
        "esc to cancel",
        "Don't ask again",
    ] {
        assert!(is_permission_prompt(screen), "{screen:?}");
    }
}

/// claude also asks with a numbered list and no recognisable phrase, so the
/// yes/no shape counts even when no needle matches.
#[test]
fn a_numbered_choice_is_a_prompt() {
    assert!(is_permission_prompt("1. Yes\n2. No, keep going"));
    assert!(is_permission_prompt("1. yes\n3. no, and tell me why"));
    assert!(
        !is_permission_prompt("1. yes"),
        "a yes with no no is not a prompt"
    );
}

#[test]
fn ordinary_output_is_not_a_prompt() {
    for screen in ["", "Cherry-picked f1d7af2", "running tests"] {
        assert!(!is_permission_prompt(screen), "{screen:?}");
    }
}

/// Every needle has to match, and the match is case-insensitive because
/// the prompt's own casing changes between claude versions.
#[test]
fn each_prompt_needle_is_detected_whatever_its_case() {
    for needle in PROMPT_NEEDLES {
        assert!(is_permission_prompt(needle), "{needle:?} was not detected");
        assert!(
            is_permission_prompt(&needle.to_uppercase()),
            "{needle:?} was not detected in upper case"
        );
    }
}

const BG_RECORD: &[u8] = br#"{"pid":6454,"sessionId":"ecfd6d61-238b-4524-94e9-a57deef12082",
    "cwd":"/home/mcs","procStart":"90348","kind":"bg","jobId":"ecfd6d61",
    "name":"gbsp-813","status":"idle"}"#;

#[test]
fn a_background_record_parses_every_field() {
    let r = ClaudeSessionRecord::parse(BG_RECORD).unwrap();
    assert_eq!(r.pid, 6454);
    assert_eq!(r.proc_start.as_deref(), Some("90348"));
    assert_eq!(
        r.session_id.as_deref(),
        Some("ecfd6d61-238b-4524-94e9-a57deef12082")
    );
    assert_eq!(r.job_id.as_deref(), Some("ecfd6d61"));
    assert_eq!(r.name.as_deref(), Some("gbsp-813"));
    assert_eq!(r.status, Some(SessionStatus::Idle));
    assert!(r.background);
}

/// A foreground session records no job id and is not background, and the
/// readers that only want status and name still get them.
#[test]
fn a_foreground_record_parses_what_it_has() {
    let json = br#"{"pid":1971,"sessionId":"s1","kind":"cli","name":"gbsp-147","status":"busy"}"#;
    let r = ClaudeSessionRecord::parse(json).unwrap();
    assert!(!r.background);
    assert_eq!(r.job_id, None);
    assert_eq!(r.name.as_deref(), Some("gbsp-147"));
    assert_eq!(r.status, Some(SessionStatus::Busy));
}

#[test]
fn a_record_with_no_pid_is_unusable() {
    assert_eq!(ClaudeSessionRecord::parse(br#"{"sessionId":"s1"}"#), None);
    assert_eq!(ClaudeSessionRecord::parse(b"not json"), None);
    assert_eq!(ClaudeSessionRecord::parse(b"{}"), None);
}

/// Empty strings are absent, not names.
#[test]
fn blank_fields_read_as_missing() {
    let json = br#"{"pid":1,"name":"","sessionId":"","jobId":""}"#;
    let r = ClaudeSessionRecord::parse(json).unwrap();
    assert_eq!((r.name, r.session_id, r.job_id), (None, None, None));
}

/// A record whose process is gone, or whose pid now belongs to something
/// else, is not live. Checked against this test process's own `/proc` entry
/// rather than a stub, so the field arithmetic is exercised for real.
#[test]
fn liveness_follows_the_recorded_start_time() {
    let me = std::process::id();
    let record = |proc_start: Option<&str>| ClaudeSessionRecord {
        pid: me,
        proc_start: proc_start.map(str::to_string),
        session_id: None,
        job_id: None,
        name: None,
        status: None,
        background: false,
    };
    let actual = super::proc_start_of(me).expect("own /proc entry");

    assert!(
        record(Some(&actual)).is_live(),
        "own start time should match"
    );
    assert!(
        record(None).is_live(),
        "a live pid with no start time counts"
    );
    assert!(
        !record(Some("0")).is_live(),
        "a mismatched start time means the pid was reused"
    );
}

#[test]
fn an_impossible_pid_is_not_live() {
    for proc_start in [Some("1"), None] {
        let record = ClaudeSessionRecord {
            pid: u32::MAX,
            proc_start: proc_start.map(str::to_string),
            session_id: None,
            job_id: None,
            name: None,
            status: None,
            background: false,
        };
        assert!(!record.is_live());
    }
}

fn info(alive: bool, exit_status: Option<&str>) -> SessionInfo {
    SessionInfo {
        id: 1,
        label: "s".into(),
        cwd: std::path::PathBuf::from("/tmp"),
        cmd: vec!["bash".into()],
        probe: ProbeKind::None,
        rows: 24,
        cols: 80,
        spawned_at_ms: 1,
        last_active_ms: 2,
        status: SessionStatus::Unknown,
        attention: false,
        alive,
        exit_status: exit_status.map(str::to_string),
    }
}

/// Both states, because a client tells a finished session from a running
/// one by exactly these two fields.
#[test]
fn liveness_and_exit_status_survive_the_wire() {
    for (alive, status) in [(true, None), (false, Some("exited 3"))] {
        let sent = info(alive, status);
        let json = serde_json::to_string(&sent).expect("encode");
        let got: SessionInfo = serde_json::from_str(&json).expect("decode");
        assert_eq!(got.alive, alive, "in {json}");
        assert_eq!(got.exit_status.as_deref(), status, "in {json}");
    }
}

/// The status can carry an OS error string, and a quote in it must not
/// break the frame it travels in.
#[test]
fn an_exit_status_holding_a_quote_round_trips() {
    let sent = Event::SessionExited {
        id: 7,
        status: r#"wait failed: no such "thing""#.into(),
    };
    let mut buf = Vec::new();
    write_frame(&mut buf, &sent).expect("write");
    let got: Event = read_frame(&mut Cursor::new(buf)).expect("read");
    match got {
        Event::SessionExited { id, status } => {
            assert_eq!(id, 7);
            assert_eq!(status, r#"wait failed: no such "thing""#);
        }
        other => panic!("expected SessionExited, got {other:?}"),
    }
}

#[test]
fn frames_written_back_to_back_read_back_in_order() {
    let mut buf = Vec::new();
    for id in 1..=3u64 {
        write_frame(
            &mut buf,
            &Request::Detach {
                session_id: id,
                keep_session: true,
            },
        )
        .expect("write");
    }
    let mut cur = Cursor::new(buf);
    for want in 1..=3u64 {
        match read_frame::<_, Request>(&mut cur).expect("read") {
            Request::Detach { session_id, .. } => assert_eq!(session_id, want),
            other => panic!("expected Detach, got {other:?}"),
        }
    }
    assert!(
        matches!(read_frame::<_, Request>(&mut cur), Err(FrameError::Eof)),
        "the stream should be exhausted"
    );
}

/// The read side already refuses an oversized header; the write side has to
/// refuse too, or the daemon emits a frame no client will accept.
#[test]
fn an_oversized_payload_is_refused_before_it_is_written() {
    let huge = "x".repeat(MAX_FRAME_BYTES as usize + 16);
    let mut buf = Vec::new();
    let err = write_frame(
        &mut buf,
        &Event::Error {
            request_id: None,
            message: huge,
        },
    )
    .unwrap_err();
    assert!(matches!(err, FrameError::TooLarge(_)), "got {err:?}");
    assert!(
        buf.is_empty(),
        "a refused frame still wrote {} bytes",
        buf.len()
    );
}

#[test]
fn a_payload_that_is_not_the_expected_shape_is_a_decode_error() {
    let mut buf = Vec::new();
    let junk = b"not json at all";
    buf.extend_from_slice(&(junk.len() as u32).to_le_bytes());
    buf.extend_from_slice(junk);
    let err = read_frame::<_, Request>(&mut Cursor::new(buf)).unwrap_err();
    assert!(
        !matches!(err, FrameError::Eof | FrameError::TooLarge(_)),
        "a bad payload should not look like a transport failure: {err:?}"
    );
}
