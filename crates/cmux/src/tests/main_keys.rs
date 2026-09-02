use super::*;
use crossterm::event::KeyModifiers;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn plain(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
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
    assert_eq!(keys::encode(alt_v), Some(vec![0x1b, b'v']), "meta+v is ESC v");
}
