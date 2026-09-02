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
