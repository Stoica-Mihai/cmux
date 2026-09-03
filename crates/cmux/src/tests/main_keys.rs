use super::*;
use crate::session::Session;
use crossterm::event::KeyModifiers;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Drive a chord from the table rather than restating its key here, so a
/// rebinding moves the test with it.
fn chord(c: &keys::Chord) -> KeyEvent {
    KeyEvent::new(c.codes[0], c.mods)
}

fn session(id: u64, label: &str) -> Session {
    let (tx, _rx) = std::sync::mpsc::channel();
    Session::new_daemon(
        id,
        label.into(),
        PathBuf::from("/tmp"),
        false,
        None,
        24,
        80,
        None,
        id,
        tx,
        0,
    )
    .0
}

fn app_with(n: u64) -> App {
    let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
    for i in 1..=n {
        app.sessions.push(session(i, &format!("s{i}")));
    }
    app
}

fn mode_name(app: &App) -> &'static str {
    match app.mode {
        Mode::Dashboard => "Dashboard",
        Mode::Spawn(_) => "Spawn",
        Mode::Rename(_) => "Rename",
        Mode::Picker(_) => "Picker",
        Mode::ConfirmDetach(_) => "ConfirmDetach",
        Mode::Scrollback(_) => "Scrollback",
        Mode::Help => "Help",
        Mode::Reorder => "Reorder",
    }
}

/// Every command goes through the prefix. Ctrl+Q used to quit on its own,
/// which meant two bindings for one action and a key taken away from the
/// focused session.
#[test]
fn ctrl_q_is_not_a_quit_binding() {
    let mut app = App::new(PathBuf::from("/tmp"), (24, 80));
    handle_key(&mut app, ctrl('q')).expect("handle");
    assert!(
        !app.should_quit,
        "Ctrl+Q quit without the prefix; quitting must go through it"
    );
}

/// And the prefix reaches quit from a mode, not just the dashboard, so
/// dropping Ctrl+Q leaves no state without a way out.
#[test]
fn the_prefix_quits_from_inside_a_mode() {
    let mut app = App::new(PathBuf::from("/tmp"), (24, 80));
    app.mode = Mode::Help;

    handle_key(&mut app, ctrl('a')).expect("handle");
    assert!(app.prefix_pending, "the prefix is not armed inside a mode");

    handle_key(&mut app, plain('q')).expect("handle");
    assert!(app.should_quit, "Ctrl+A q did not quit from inside a mode");
}

/// claude reads an image out of the clipboard when it sees ctrl+v or meta+v,
/// so both have to reach it byte for byte.
#[test]
fn ctrl_v_and_alt_v_reach_the_child() {
    use crossterm::event::KeyModifiers;
    let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
    let alt_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT);
    assert_eq!(keys::encode(ctrl_v), Some(vec![0x16]), "ctrl+v is SYN");
    assert_eq!(
        keys::encode(alt_v),
        Some(vec![0x1b, b'v']),
        "meta+v is ESC v"
    );
}

/// The prefix arms, then disarms whatever the next key is, so a chord can
/// never be left half-entered.
#[test]
fn the_prefix_is_consumed_by_whatever_key_follows_it() {
    let mut app = app_with(1);
    handle_key(&mut app, ctrl('a')).expect("handle");
    assert!(app.prefix_pending);
    handle_key(&mut app, plain('z')).expect("handle");
    assert!(
        !app.prefix_pending,
        "an unrecognised chord key left the prefix armed"
    );
}

#[test]
fn the_prefix_opens_each_mode_it_is_meant_to() {
    for (c, want) in [
        (&keys::PREFIX_HELP, "Help"),
        (&keys::PREFIX_SPAWN, "Spawn"),
        (&keys::PREFIX_DETACH, "ConfirmDetach"),
        (&keys::PREFIX_REORDER, "Reorder"),
        (&keys::PREFIX_SCROLLBACK, "Scrollback"),
    ] {
        let mut app = app_with(1);
        handle_key(&mut app, chord(&keys::PREFIX)).expect("handle");
        handle_key(&mut app, chord(c)).expect("handle");
        assert_eq!(
            mode_name(&app),
            want,
            "Ctrl+A {} should open {want}, got {}",
            c.label,
            mode_name(&app)
        );
    }
}

/// Detach and rename act on the focused session, so with none focused they
/// must do nothing rather than open a modal pointing at no session.
#[test]
fn prefix_chords_that_need_a_session_do_nothing_without_one() {
    for c in [
        &keys::PREFIX_DETACH,
        &keys::PREFIX_RENAME,
        &keys::PREFIX_REORDER,
        &keys::PREFIX_SCROLLBACK,
    ] {
        let mut app = app_with(0);
        handle_key(&mut app, chord(&keys::PREFIX)).expect("handle");
        handle_key(&mut app, chord(c)).expect("handle");
        assert_eq!(
            mode_name(&app),
            "Dashboard",
            "Ctrl+A {} opened a modal with no session to act on",
            c.label
        );
    }
}

#[test]
fn the_prefix_cycles_focus_both_ways() {
    let mut app = app_with(3);
    app.focus = 0;
    handle_key(&mut app, chord(&keys::PREFIX)).expect("handle");
    handle_key(&mut app, chord(&keys::PREFIX_FOCUS_PREV)).expect("handle");
    assert_eq!(app.focus, 2, "previous from the first should wrap to last");
    handle_key(&mut app, chord(&keys::PREFIX)).expect("handle");
    handle_key(&mut app, chord(&keys::PREFIX_FOCUS_NEXT)).expect("handle");
    assert_eq!(app.focus, 0, "next from the last should wrap to first");
}

#[test]
fn a_prefix_digit_jumps_to_that_session() {
    let mut app = app_with(3);
    handle_key(&mut app, ctrl('a')).expect("handle");
    handle_key(&mut app, plain('3')).expect("handle");
    assert_eq!(app.focus, 2, "Ctrl+A 3 should focus the third session");

    handle_key(&mut app, ctrl('a')).expect("handle");
    handle_key(&mut app, plain('9')).expect("handle");
    assert_eq!(app.focus, 2, "a digit past the end should not move focus");
}

#[test]
fn the_prefix_toggles_the_sidebar_back_and_forth() {
    let mut app = app_with(1);
    let before = app.show_sidebar;
    handle_key(&mut app, chord(&keys::PREFIX)).expect("handle");
    handle_key(&mut app, chord(&keys::PREFIX_TOGGLE_SIDEBAR)).expect("handle");
    assert_ne!(app.show_sidebar, before, "the sidebar did not toggle");
    handle_key(&mut app, chord(&keys::PREFIX)).expect("handle");
    handle_key(&mut app, chord(&keys::PREFIX_TOGGLE_SIDEBAR)).expect("handle");
    assert_eq!(app.show_sidebar, before, "it did not toggle back");
}

/// Ctrl+A a sends a literal Ctrl+A to the session, which is how a nested
/// program that uses the same prefix stays reachable.
#[test]
fn the_prefix_can_send_a_literal_prefix_to_the_session() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (sess, _slot) = Session::new_daemon(
        1,
        "s".into(),
        PathBuf::from("/tmp"),
        false,
        None,
        24,
        80,
        None,
        1,
        tx,
        0,
    );
    let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
    app.sessions.push(sess);

    handle_key(&mut app, ctrl('a')).expect("handle");
    handle_key(&mut app, plain('a')).expect("handle");

    match rx.try_recv().expect("nothing was sent to the session") {
        cmux_proto::Request::Input { bytes, .. } => assert_eq!(bytes, vec![0x01]),
        other => panic!("expected Input, got {other:?}"),
    }
}

#[test]
fn confirming_a_detach_ends_the_session_and_declining_keeps_it() {
    let mut app = app_with(2);
    app.mode = Mode::ConfirmDetach(1);
    handle_key(&mut app, plain('n')).expect("handle");
    assert_eq!(app.sessions.len(), 2, "declining should keep it");
    assert_eq!(mode_name(&app), "Dashboard");

    app.mode = Mode::ConfirmDetach(1);
    handle_key(&mut app, plain('y')).expect("handle");
    assert_eq!(app.sessions.len(), 1, "confirming should end it");
    assert_eq!(app.sessions[0].id, 2, "it removed the wrong session");
}

#[test]
fn a_detach_confirmed_for_a_session_that_is_gone_does_nothing() {
    let mut app = app_with(1);
    app.mode = Mode::ConfirmDetach(99);
    handle_key(&mut app, plain('y')).expect("handle");
    assert_eq!(app.sessions.len(), 1, "it removed an unrelated session");
    assert_eq!(mode_name(&app), "Dashboard");
}

#[test]
fn reorder_moves_the_focused_session_and_esc_leaves_the_mode() {
    let mut app = app_with(3);
    app.focus = 0;
    app.mode = Mode::Reorder;
    handle_key(&mut app, key(KeyCode::Down)).expect("handle");
    let order: Vec<u64> = app.sessions.iter().map(|s| s.id).collect();
    assert_eq!(order, vec![2, 1, 3], "the session did not move down");
    assert_eq!(app.focus, 1, "focus should follow the session it moved");
    assert_eq!(mode_name(&app), "Reorder", "it should stay in reorder");

    handle_key(&mut app, key(KeyCode::Esc)).expect("handle");
    assert_eq!(mode_name(&app), "Dashboard");
}

#[test]
fn reorder_at_the_end_of_the_list_does_not_wrap_out_of_range() {
    let mut app = app_with(2);
    app.focus = 0;
    app.mode = Mode::Reorder;
    handle_key(&mut app, key(KeyCode::Up)).expect("handle");
    let order: Vec<u64> = app.sessions.iter().map(|s| s.id).collect();
    assert_eq!(order.len(), 2, "a session was lost");
    assert!(app.focus < app.sessions.len(), "focus went out of range");
}

#[test]
fn renaming_commits_on_enter_and_discards_on_escape() {
    let mut app = app_with(1);
    app.mode = Mode::Rename(RenameState {
        session_id: 1,
        buf: String::new(),
    });
    for c in "newname".chars() {
        handle_key(&mut app, plain(c)).expect("handle");
    }
    handle_key(&mut app, key(KeyCode::Enter)).expect("handle");
    assert_eq!(app.sessions[0].label, "newname");
    assert_eq!(mode_name(&app), "Dashboard");

    app.mode = Mode::Rename(RenameState {
        session_id: 1,
        buf: "discarded".into(),
    });
    handle_key(&mut app, key(KeyCode::Esc)).expect("handle");
    assert_eq!(
        app.sessions[0].label, "newname",
        "escape should discard the edit"
    );
}

#[test]
fn backspace_edits_the_rename_buffer() {
    let mut app = app_with(1);
    app.mode = Mode::Rename(RenameState {
        session_id: 1,
        buf: "abc".into(),
    });
    handle_key(&mut app, key(KeyCode::Backspace)).expect("handle");
    handle_key(&mut app, key(KeyCode::Enter)).expect("handle");
    assert_eq!(app.sessions[0].label, "ab");
}

#[test]
fn help_closes_on_any_key() {
    let mut app = app_with(1);
    app.mode = Mode::Help;
    handle_key(&mut app, plain('x')).expect("handle");
    assert_eq!(mode_name(&app), "Dashboard", "help should close");
}

/// With the daemon gone there is nothing left to drive, so every key ends
/// the session rather than being sent into a dead socket.
#[test]
fn any_key_quits_once_the_daemon_is_lost() {
    let mut app = app_with(1);
    app.daemon_lost = true;
    handle_key(&mut app, plain('x')).expect("handle");
    assert!(app.should_quit);
}

#[test]
fn an_ordinary_key_reaches_the_focused_session() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (sess, _slot) = Session::new_daemon(
        1,
        "s".into(),
        PathBuf::from("/tmp"),
        false,
        None,
        24,
        80,
        None,
        1,
        tx,
        0,
    );
    let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
    app.sessions.push(sess);

    handle_key(&mut app, plain('x')).expect("handle");
    match rx.try_recv().expect("the keystroke never reached the pty") {
        cmux_proto::Request::Input { bytes, .. } => assert_eq!(bytes, b"x".to_vec()),
        other => panic!("expected Input, got {other:?}"),
    }
}
