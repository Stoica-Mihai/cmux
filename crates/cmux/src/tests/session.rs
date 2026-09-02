use super::*;

fn daemon_session(remote_id: u64) -> (Session, mpsc::Receiver<ProtoRequest>) {
    let (tx, rx) = mpsc::channel();
    let (sess, _slot) = Session::new_daemon(
        1,
        "s".into(),
        PathBuf::from("/tmp"),
        false,
        None,
        24,
        80,
        None,
        remote_id,
        tx,
    );
    (sess, rx)
}

/// The confirm dialog promises the process will be killed. Dropping a
/// daemon-backed handle does not do that, so kill() has to say so — or the
/// session lives on, still listed by every other client.
#[test]
fn killing_a_daemon_session_ends_it_for_everyone() {
    let (mut sess, rx) = daemon_session(42);
    sess.kill();
    match rx.try_recv().expect("kill should send a request") {
        ProtoRequest::Detach {
            session_id,
            keep_session,
        } => {
            assert_eq!(session_id, 42);
            assert!(!keep_session, "kill must end the session, not park it");
        }
        other => panic!("expected Detach, got {other:?}"),
    }
}

/// The opposite case: quitting the TUI must leave sessions running.
#[test]
fn detach_keep_parks_the_session() {
    let (mut sess, rx) = daemon_session(7);
    sess.detach_keep();
    match rx.try_recv().expect("detach_keep should send a request") {
        ProtoRequest::Detach {
            session_id,
            keep_session,
        } => {
            assert_eq!(session_id, 7);
            assert!(keep_session, "quitting must not kill the sessions");
        }
        other => panic!("expected Detach, got {other:?}"),
    }
}

/// A client asks for a size; the pty runs at the smallest asked for by
/// anyone. Rendering at the requested size instead leaves every row past
/// the pty's height showing output from before the shrink.
#[test]
fn a_daemon_session_renders_at_the_effective_size_not_the_requested_one() {
    use alacritty_terminal::grid::Dimensions;
    let (mut sess, rx) = daemon_session(9);
    let grid = |s: &Session| {
        let p = s.parser.lock().expect("parser");
        (
            p.term.grid().screen_lines() as u16,
            p.term.grid().columns() as u16,
        )
    };
    assert_eq!(grid(&sess), (24, 80));

    sess.resize(40, 120).expect("resize");
    assert_eq!(
        grid(&sess),
        (24, 80),
        "the grid followed this client's request instead of the pty"
    );
    match rx
        .try_recv()
        .expect("the request should still reach the daemon")
    {
        ProtoRequest::Resize { rows, cols, .. } => assert_eq!((rows, cols), (40, 120)),
        other => panic!("expected Resize, got {other:?}"),
    }

    sess.apply_effective_size(26, 84);
    assert_eq!(grid(&sess), (26, 84));

    // …and asks the daemon to repaint, because the grid it just resized
    // holds reflowed leftovers of the old width.
    match rx.try_recv().expect("a repaint should have been requested") {
        ProtoRequest::Attach { session_id, .. } => assert_eq!(session_id, 9),
        other => panic!("expected Attach, got {other:?}"),
    }
}

#[test]
fn renaming_a_daemon_session_tells_the_daemon() {
    let (mut sess, rx) = daemon_session(3);
    sess.set_label("renamed".into());
    assert_eq!(sess.label, "renamed");
    match rx.try_recv().expect("set_label should send a request") {
        ProtoRequest::Rename { session_id, label } => {
            assert_eq!(session_id, 3);
            assert_eq!(label, "renamed");
        }
        other => panic!("expected Rename, got {other:?}"),
    }
}
