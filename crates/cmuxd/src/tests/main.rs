use super::*;

/// Insert out of id order; `list` must still come back ascending. With a
/// HashMap the insertion order is not the iteration order, so this fails
/// if the sort is removed.
#[tokio::test]
async fn list_is_ordered_by_session_id() {
    let registry = Registry::new();
    for id in [4u64, 1, 5, 3, 2] {
        let sess = Session::spawn(
            id,
            format!("s{id}"),
            PathBuf::from("/tmp"),
            vec!["/bin/sleep".into(), "30".into()],
            cmux_proto::ProbeKind::None,
            24,
            80,
        )
        .expect("spawn /bin/sleep");
        registry.insert(sess).await;
    }
    let ids: Vec<u64> = registry.list().await.into_iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);

    for s in registry.sessions.lock().await.values() {
        s.kill();
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn bare_cmuxd_runs_the_daemon_with_no_http() {
    assert_eq!(parse_cli(Vec::<String>::new()), Cli::Run(Config::default()));
}

#[test]
fn http_is_opt_in_and_takes_an_optional_address() {
    // Bare --http means the default address.
    assert_eq!(
        parse_cli(args(&["--http"])),
        Cli::Run(Config {
            http: Some(DEFAULT_HTTP_ADDR.to_string()),
        })
    );
    // Both spellings of an explicit address.
    let want = Cli::Run(Config {
        http: Some("0.0.0.0:9000".to_string()),
    });
    assert_eq!(parse_cli(args(&["--http", "0.0.0.0:9000"])), want);
    assert_eq!(parse_cli(args(&["--http=0.0.0.0:9000"])), want);
}

/// `--http` must not swallow a following flag as its address.
#[test]
fn http_does_not_consume_the_next_flag() {
    assert!(matches!(
        parse_cli(args(&["--http", "--version"])),
        Cli::Print(_)
    ));
    assert!(matches!(
        parse_cli(args(&["--http", "--nope"])),
        Cli::Reject(_)
    ));
}

/// The bug this guards: argv was ignored, so `cmuxd --help` started a
/// daemon and blocked instead of printing anything.
#[test]
fn help_and_version_print_instead_of_daemonizing() {
    let help = parse_cli(vec!["--help".to_string()]);
    assert!(
        matches!(help, Cli::Print(ref t) if t.contains("Usage: cmuxd")),
        "{help:?}"
    );
    assert_eq!(parse_cli(vec!["-h".to_string()]), help);

    match parse_cli(vec!["--version".to_string()]) {
        Cli::Print(t) => assert_eq!(t, format!("cmuxd {SERVER_VERSION}")),
        other => panic!("expected a version line, got {other:?}"),
    }
}

#[test]
fn an_unknown_argument_is_rejected_not_ignored() {
    match parse_cli(vec!["--nope".to_string()]) {
        Cli::Reject(msg) => assert!(msg.contains("--nope"), "{msg}"),
        other => panic!("expected a rejection, got {other:?}"),
    }
}

#[test]
fn fallback_socket_dir_nests_home_under_the_uid_dir() {
    assert_eq!(
        fallback_socket_dir(1000, "/home/mcs"),
        PathBuf::from("/tmp/cmux-1000/home/mcs")
    );
    assert_eq!(
        fallback_socket_dir(0, "/root"),
        PathBuf::from("/tmp/cmux-0/root")
    );
}

fn sleeper(id: u64) -> Arc<Session> {
    Session::spawn(
        id,
        format!("s{id}"),
        PathBuf::from("/tmp"),
        vec!["/bin/sleep".into(), "30".into()],
        cmux_proto::ProbeKind::None,
        24,
        80,
    )
    .expect("spawn /bin/sleep")
}

#[test]
fn session_ids_and_client_ids_come_from_separate_counters() {
    let registry = Registry::new();
    assert_eq!(registry.alloc_id(), 1);
    assert_eq!(registry.alloc_id(), 2);
    assert_eq!(
        registry.alloc_client_id(),
        1,
        "a client id should not be drawn from the session pool"
    );
    assert_eq!(registry.alloc_client_id(), 2);
    assert_eq!(registry.alloc_id(), 3, "allocating a client moved the ids");
}

#[tokio::test]
async fn removing_a_session_takes_it_out_once() {
    let registry = Registry::new();
    let sess = sleeper(1);
    registry.insert(sess.clone()).await;
    assert!(registry.get(1).await.is_some());
    assert!(registry.remove(1).await.is_some());
    assert!(registry.get(1).await.is_none(), "it is still registered");
    assert!(
        registry.remove(1).await.is_none(),
        "removing twice should not hand out a second copy"
    );
    sess.kill();
}

/// The pty runs at the smallest size among attached clients, so a client
/// that leaves has to be forgotten in every session at once. While it was
/// not, a phone that closed its tab kept every session pinned small.
#[tokio::test]
async fn a_departing_client_stops_holding_every_session_small() {
    let registry = Registry::new();
    let (a, b) = (sleeper(1), sleeper(2));
    registry.insert(a.clone()).await;
    registry.insert(b.clone()).await;

    // A second client stays attached, so the size the phone was holding down
    // is observable after it leaves. With nobody left the pty keeps whatever
    // size it is running at, which is what stops a detach from making the
    // program repaint.
    let desktop = registry.alloc_client_id();
    let phone = registry.alloc_client_id();
    for s in [&a, &b] {
        s.set_client_size(desktop, 40, 120)
            .expect("size for the desktop");
        s.set_client_size(phone, 10, 40)
            .expect("size for the phone");
        let info = s.info();
        assert_eq!(
            (info.rows, info.cols),
            (10, 40),
            "session {} should follow the smallest client",
            info.id
        );
    }

    registry.drop_client_everywhere(phone).await;

    for s in [&a, &b] {
        let info = s.info();
        assert_eq!(
            (info.rows, info.cols),
            (40, 120),
            "session {} stayed small after the client left",
            info.id
        );
        assert_eq!(s.attached_clients(), 1, "only the desktop should be left");
    }
    a.kill();
    b.kill();
}

#[tokio::test]
async fn a_spawned_session_is_labelled_after_its_directory() {
    let registry = Arc::new(Registry::new());
    let info = spawn_session(
        &registry,
        PathBuf::from("/tmp/some-project"),
        vec!["/bin/sleep".into(), "30".into()],
        cmux_proto::ProbeKind::None,
        None,
        24,
        80,
    )
    .await
    .expect("spawn");
    assert_eq!(info.label, "some-project");

    let named = spawn_session(
        &registry,
        PathBuf::from("/tmp/some-project"),
        vec!["/bin/sleep".into(), "30".into()],
        cmux_proto::ProbeKind::None,
        Some("chosen".into()),
        24,
        80,
    )
    .await
    .expect("spawn");
    assert_eq!(named.label, "chosen", "an explicit label should win");

    for s in registry.sessions.lock().await.values() {
        s.kill();
    }
}

/// openpty rejects a zero dimension, so a client asking for one must be
/// clamped rather than failing the spawn.
#[tokio::test]
async fn a_zero_size_request_is_clamped_instead_of_refused() {
    let registry = Arc::new(Registry::new());
    let info = spawn_session(
        &registry,
        PathBuf::from("/tmp"),
        vec!["/bin/sleep".into(), "30".into()],
        cmux_proto::ProbeKind::None,
        None,
        0,
        0,
    )
    .await
    .expect("a zero size should not fail the spawn");
    assert!(info.rows >= 1 && info.cols >= 1, "got {info:?}");

    for s in registry.sessions.lock().await.values() {
        s.kill();
    }
}

#[tokio::test]
async fn spawning_an_unknown_program_reports_the_program_name() {
    let registry = Arc::new(Registry::new());
    let err = spawn_session(
        &registry,
        PathBuf::from("/tmp"),
        vec!["definitely-not-a-real-binary".into()],
        cmux_proto::ProbeKind::None,
        None,
        24,
        80,
    )
    .await
    .expect_err("an unknown program should not spawn");
    let text = format!("{err:#}");
    assert!(
        text.contains("definitely-not-a-real-binary"),
        "the error should name the program: {text}"
    );
    assert!(
        registry.sessions.lock().await.is_empty(),
        "a failed spawn should register nothing"
    );
}

async fn frame_pair() -> (
    tokio::net::unix::OwnedWriteHalf,
    tokio::net::unix::OwnedReadHalf,
) {
    let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
    let (_, w) = a.into_split();
    let (r, _) = b.into_split();
    (w, r)
}

#[tokio::test]
async fn a_frame_survives_the_round_trip() {
    let (mut w, mut r) = frame_pair().await;
    let sent = cmux_proto::Request::Resize {
        session_id: 7,
        rows: 30,
        cols: 100,
    };
    write_frame_async(&mut w, &sent).await.expect("write");
    let got: cmux_proto::Request = read_frame_async(&mut r).await.expect("read");
    assert!(
        matches!(
            got,
            cmux_proto::Request::Resize {
                session_id: 7,
                rows: 30,
                cols: 100
            }
        ),
        "got {got:?}"
    );
}

#[tokio::test]
async fn frames_come_back_in_the_order_they_were_written() {
    let (mut w, mut r) = frame_pair().await;
    for id in 1..=3u64 {
        write_frame_async(
            &mut w,
            &cmux_proto::Request::Detach {
                session_id: id,
                keep_session: true,
            },
        )
        .await
        .expect("write");
    }
    for want in 1..=3u64 {
        match read_frame_async::<cmux_proto::Request>(&mut r)
            .await
            .expect("read")
        {
            cmux_proto::Request::Detach { session_id, .. } => assert_eq!(session_id, want),
            other => panic!("expected Detach, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_closed_stream_reads_as_eof_not_an_io_error() {
    let (w, mut r) = frame_pair().await;
    drop(w);
    match read_frame_async::<cmux_proto::Request>(&mut r).await {
        Err(FrameError::Eof) => {}
        other => panic!("expected Eof, got {other:?}"),
    }
}

/// A length header larger than the cap has to be refused before the
/// payload is allocated, or a bad peer can ask for gigabytes.
#[tokio::test]
async fn an_oversized_length_header_is_refused() {
    let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
    let (_, mut raw) = a.into_split();
    let (mut r, _keep) = b.into_split();
    let too_big = cmux_proto::MAX_FRAME_BYTES + 1;
    raw.write_all(&too_big.to_le_bytes()).await.expect("write");
    raw.flush().await.expect("flush");
    // Bounded, because without the check the read blocks on a payload that
    // never comes rather than returning anything.
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_frame_async::<cmux_proto::Request>(&mut r),
    )
    .await
    .expect("the header should be refused, not waited on");
    match got {
        Err(FrameError::TooLarge(n)) => assert_eq!(n, too_big),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[tokio::test]
async fn a_truncated_payload_reads_as_eof() {
    let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
    let (_, mut raw) = a.into_split();
    let (mut r, _keep) = b.into_split();
    raw.write_all(&64u32.to_le_bytes()).await.expect("len");
    raw.write_all(b"{\"tru").await.expect("partial");
    raw.flush().await.expect("flush");
    drop(raw);
    match read_frame_async::<cmux_proto::Request>(&mut r).await {
        Err(FrameError::Eof) => {}
        other => panic!("expected Eof, got {other:?}"),
    }
}

#[tokio::test]
async fn a_well_framed_but_unparseable_payload_is_a_decode_error() {
    let (a, b) = tokio::net::UnixStream::pair().expect("socketpair");
    let (_, mut raw) = a.into_split();
    let (mut r, _keep) = b.into_split();
    let junk = b"not json at all";
    raw.write_all(&(junk.len() as u32).to_le_bytes())
        .await
        .expect("len");
    raw.write_all(junk).await.expect("payload");
    raw.flush().await.expect("flush");
    match read_frame_async::<cmux_proto::Request>(&mut r).await {
        Err(FrameError::Eof) | Err(FrameError::Io(_)) | Err(FrameError::TooLarge(_)) => {
            panic!("a bad payload should be a decode error, not a transport one")
        }
        Err(_) => {}
        Ok(msg) => panic!("junk decoded to {msg:?}"),
    }
}
