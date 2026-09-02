use super::*;

#[test]
fn spawn_rejects_an_empty_command() {
    let result = Session::spawn(
        1,
        "x".into(),
        PathBuf::from("/tmp"),
        Vec::<String>::new(),
        ProbeKind::None,
        24,
        80,
    );
    let err = match result {
        Ok(_) => panic!("empty argv must not spawn a session"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("empty command"), "got: {err}");
}

#[test]
fn spawn_runs_an_arbitrary_program_and_reports_it() {
    let sess = Session::spawn(
        7,
        "echo-test".into(),
        PathBuf::from("/tmp"),
        vec!["/bin/echo".into(), "hello".into()],
        ProbeKind::None,
        30,
        100,
    )
    .expect("spawn /bin/echo");

    let info = sess.info();
    assert_eq!(info.cmd, vec!["/bin/echo", "hello"]);
    assert_eq!(info.probe, ProbeKind::None);
    assert_eq!((info.rows, info.cols), (30, 100));
    assert!(!sess.has_probe());
}

/// Both directions: a probe name must reach `info()` — the browser reads
/// that — but must not clobber a name the user chose.
#[test]
fn the_probe_names_a_session_until_someone_renames_it() {
    let sess = Session::spawn(
        9,
        "spawned".into(),
        PathBuf::from("/tmp"),
        vec!["/bin/sleep".into(), "30".into()],
        ProbeKind::None,
        24,
        80,
    )
    .expect("spawn");
    assert_eq!(sess.info().label, "spawned");

    assert!(sess.take_probe_label("probe-named"));
    assert_eq!(
        sess.info().label,
        "probe-named",
        "the API the browser reads must see the probe's name"
    );

    sess.rename("mine".into());
    assert_eq!(sess.info().label, "mine");
    assert!(
        !sess.take_probe_label("probe-again"),
        "a manual rename must survive the next probe tick"
    );
    assert_eq!(sess.info().label, "mine");
    sess.kill();
}

/// Attach, replay what the daemon hands a new client into a fresh
/// terminal, and require the same grid *and* the same screen buffer.
///
/// The fixture deliberately overflows the 1 MiB ring. A complete ring
/// replays correctly, so the bug only shows once the front has been
/// dropped and the replay starts mid-stream, missing the alt-screen
/// switch that framed everything after it. That is the state any
/// long-running full-screen program reaches.
fn assert_attach_reproduces_the_session(script: &str, expect_alt: bool) {
    let (rows, cols) = (10u16, 40u16);
    let sess = Session::spawn(
        1,
        "t".into(),
        PathBuf::from("/tmp"),
        vec!["/bin/sh".into(), "-c".into(), script.into()],
        ProbeKind::None,
        rows,
        cols,
    )
    .expect("spawn");
    std::thread::sleep(std::time::Duration::from_millis(1200));

    let payload = sess.attach_payload();
    let live = sess.term_state.lock().expect("term");
    assert_eq!(
        crate::snapshot::is_alt_screen(&live.term),
        expect_alt,
        "the fixture did not put the session where the test expects"
    );

    let size = TermSize {
        lines: rows as usize,
        cols: cols as usize,
    };
    let mut fresh = Term::new(TermConfig::default(), &size, VoidListener);
    let mut proc: Processor = Processor::new();
    proc.advance(&mut fresh, &payload);

    assert_eq!(
        crate::snapshot::is_alt_screen(&fresh),
        crate::snapshot::is_alt_screen(&live.term),
        "the attaching client ended up in the other screen buffer"
    );
    for row in 0..rows as usize {
        let line = alacritty_terminal::index::Line(row as i32);
        for col in 0..cols as usize {
            let a = &live.term.grid()[line][alacritty_terminal::index::Column(col)];
            let b = &fresh.grid()[line][alacritty_terminal::index::Column(col)];
            assert_eq!(
                (a.c, a.fg, a.bg, a.flags),
                (b.c, b.fg, b.bg, b.flags),
                "cell ({row},{col}) differs; an attaching client sees something else"
            );
        }
    }
    drop(live);
    sess.kill();
}

#[test]
fn attaching_reproduces_a_long_running_full_screen_program() {
    assert_attach_reproduces_the_session(
        "printf '\\033[?1049h\\033[2J\\033[H'; \
         head -c 1400000 /dev/zero | tr '\\0' 'x'; \
         printf '\\033[2J\\033[HFINAL\\r\\n\\033[7mROW2\\033[0m'; sleep 5",
        true,
    );
}

#[test]
fn attaching_reproduces_a_program_that_repaints_in_place() {
    assert_attach_reproduces_the_session(
        "printf 'old frame\\r\\n'; \
         printf '\\033[2J\\033[H\\033[32mnew frame\\033[0m\\r\\n'; sleep 5",
        false,
    );
}

fn proc_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().next()?.chars().next()
}

/// A finished session used to leave a zombie in the process table and go
/// on being reported as running, because nothing ever waited on the child.
#[test]
fn a_finished_session_is_reaped_and_reported() {
    let sess = Session::spawn(
        1,
        "shortlived".into(),
        PathBuf::from("/tmp"),
        // Lives briefly, so "starts alive" is not a race against a child
        // that has already exited by the time the assertion runs.
        vec!["/bin/sh".into(), "-c".into(), "sleep 0.4; exit 3".into()],
        ProbeKind::None,
        24,
        80,
    )
    .expect("spawn");
    let pid = sess.pid.expect("a pid");
    assert!(sess.info().alive, "it should start out alive");

    // The reader thread waits on the child as soon as the pty closes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while sess.info().alive && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    assert_ne!(
        proc_state(pid),
        Some('Z'),
        "pid {pid} is a zombie; nothing waited on it"
    );
    let info = sess.info();
    assert!(!info.alive, "the session is still reported as running");
    assert_eq!(
        info.exit_status.as_deref(),
        Some("exited 3"),
        "the exit status was not recorded"
    );
}

#[test]
fn a_claude_session_gets_a_probe() {
    let sess = Session::spawn(
        8,
        "claude-test".into(),
        PathBuf::from("/tmp"),
        vec!["/bin/true".into()],
        ProbeKind::Claude {
            dangerous: true,
            resume_id: None,
        },
        24,
        80,
    )
    .expect("spawn");
    assert!(sess.has_probe());
    assert!(sess.info().probe.dangerous());
}
