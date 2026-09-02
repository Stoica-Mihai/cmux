use super::*;

const AUTO_NAMED_FORK: &[u8] = br#"{"name":"he-https-mtn \u2442 he-https-claro",
    "nameSource":"auto","intent":"he-https-claro",
    "forkParentSessionId":"e1896a80-d12f-4174-bdb3-f74ae67f14fb",
    "sessionId":"fe3a2938-355e-4a89-b42f-f3e843d66983"}"#;

#[test]
fn an_auto_named_fork_shows_its_purpose_and_its_parent() {
    let job = parse_job_state(AUTO_NAMED_FORK).unwrap();
    assert_eq!(job.display_name().as_deref(), Some("he-https-claro"));
    assert_eq!(
        job.forked_from.as_deref(),
        Some("e1896a80-d12f-4174-bdb3-f74ae67f14fb")
    );
}

#[test]
fn a_name_a_person_chose_is_kept_whole() {
    let json = br#"{"name":"gbsp-813","nameSource":"peer","intent":"something else"}"#;
    let job = parse_job_state(json).unwrap();
    assert_eq!(job.display_name().as_deref(), Some("gbsp-813"));
    assert_eq!(job.forked_from, None);

    let json = br#"{"name":"gbsp-1054","nameSource":"user"}"#;
    assert_eq!(
        parse_job_state(json).unwrap().display_name().as_deref(),
        Some("gbsp-1054")
    );
}

#[test]
fn an_auto_name_with_no_purpose_is_kept_whole() {
    let json = br#"{"name":"some-session","nameSource":"auto"}"#;
    assert_eq!(
        parse_job_state(json).unwrap().display_name().as_deref(),
        Some("some-session")
    );
}

#[test]
fn a_job_state_with_no_naming_fields_reports_nothing() {
    assert_eq!(parse_job_state(b"{}").unwrap().display_name(), None);
    assert_eq!(parse_job_state(b"not json"), None);
}

#[test]
fn a_session_with_no_live_record_resumes() {
    assert_eq!(
        open_command(false, Some("no-such-session-id")),
        vec!["claude", "--resume", "no-such-session-id"]
    );
    assert_eq!(open_command(false, None), vec!["claude"]);
}

/// The live-background lookup agrees with a direct read of the same records.
/// A stale record, whose pid has exited or been reused, must not appear.
#[test]
fn live_background_matches_the_records_on_disk() {
    let found = live_background();
    let dir = match cmux_proto::claude_sessions_dir() {
        Some(d) if d.is_dir() => d,
        _ => return,
    };
    let mut expected = 0usize;
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        let Some(r) = cmux_proto::ClaudeSessionRecord::parse(&bytes) else {
            continue;
        };
        if let Some(id) = &r.session_id
            && r.job_id.is_some()
            && r.background
            && r.is_live()
        {
            expected += 1;
            assert!(
                found.contains_key(id),
                "live background session {id} is missing from the lookup"
            );
        }
    }
    assert_eq!(
        found.len(),
        expected,
        "the lookup and the records on disk disagree"
    );
}
