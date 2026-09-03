use super::*;
use crate::session::Session;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use std::path::PathBuf;

fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn render(w: u16, h: u16, body: impl FnOnce(&mut Frame)) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("backend");
    term.draw(body).expect("draw");
    term.backend().buffer().clone()
}

fn buffer_text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn app_with(n: u64) -> App {
    let mut app = App::new(PathBuf::from("/tmp"), (24, 80));
    for i in 1..=n {
        let (tx, _rx) = std::sync::mpsc::channel();
        app.sessions.push(
            Session::new_daemon(
                i,
                format!("s{i}"),
                PathBuf::from("/tmp"),
                false,
                None,
                24,
                80,
                None,
                i,
                tx,
                0,
            )
            .0,
        );
    }
    app
}

/// Chords are listed where they are live. At the dashboard none of them do
/// anything until the prefix is down, so listing them there is noise; once
/// it is down they are one keypress away and the full list belongs there.
#[test]
fn the_chord_list_lives_in_the_prefix_row_not_the_idle_one() {
    let idle = text(&dashboard_footer(""));
    let prefix = text(&prefix_footer());

    for chord in ["=new", "=load", "=rename", "=detach", "=sidebar", "=quit"] {
        assert!(
            prefix.contains(chord),
            "the prefix row should list {chord}: {prefix}"
        );
        assert!(
            !idle.contains(chord),
            "the idle row still lists {chord}, where it does nothing: {idle}"
        );
    }

    // The idle row still has to say how to reach them.
    assert!(idle.contains(keys::PREFIX.label), "{idle}");
    assert!(idle.contains(keys::PREFIX_HELP.label), "{idle}");
    assert!(
        idle.chars().count() < prefix.chars().count(),
        "the idle row should be the shorter of the two"
    );
}

#[test]
fn a_status_message_is_appended_to_the_idle_row() {
    let plain = text(&dashboard_footer(""));
    let with_status = text(&dashboard_footer("spawned session [2]"));
    assert!(with_status.contains("spawned session [2]"));
    assert!(with_status.chars().count() > plain.chars().count());
}

/// The row said "Ctrl+A" twice: once as its own hint, once inside a status
/// message that carried a second copy of the chord list.
#[test]
fn the_prefix_is_named_once_even_with_a_status() {
    for status in ["", "spawned session [2]", "resumed session [7]"] {
        let line = text(&dashboard_footer(status));
        let named = line.matches(keys::PREFIX.label).count();
        assert_eq!(named, 1, "the prefix is named {named} times: {line}");
    }
}

#[test]
fn the_titlebar_counts_the_sessions_and_the_focused_one() {
    let empty = buffer_text(&render(80, 1, |f| {
        draw_titlebar(f, &app_with(0), Rect::new(0, 0, 80, 1))
    }));
    assert!(empty.contains("0/0"), "{empty}");

    let mut app = app_with(3);
    app.focus = 1;
    let some = buffer_text(&render(80, 1, |f| {
        draw_titlebar(f, &app, Rect::new(0, 0, 80, 1))
    }));
    assert!(
        some.contains("2/3"),
        "focus is 1-indexed for display: {some}"
    );
}

/// Local mode and daemon mode look different on purpose: in local mode the
/// sessions die with the process, and nothing else can see them.
#[test]
fn the_titlebar_says_which_mode_it_is_in() {
    let local = buffer_text(&render(80, 1, |f| {
        draw_titlebar(f, &app_with(1), Rect::new(0, 0, 80, 1))
    }));
    assert!(local.contains("local"), "{local}");
    assert!(!local.contains("cmuxd"), "{local}");
}

#[test]
fn the_titlebar_fits_a_narrow_terminal_without_panicking() {
    for w in [1u16, 4, 12, 20] {
        let buf = render(w, 1, |f| {
            draw_titlebar(f, &app_with(2), Rect::new(0, 0, w, 1))
        });
        assert_eq!(buf.area.width, w, "it drew outside a {w}-wide area");
    }
}

#[test]
fn a_toast_lands_inside_the_tile_it_is_given() {
    let tile = Rect::new(0, 0, 40, 10);
    let buf = render(40, 10, |f| draw_toast(f, tile, "copied"));
    let text = buffer_text(&buf);
    assert!(text.contains("copied"), "{text}");
    let row = text.lines().nth(8).unwrap_or("");
    assert!(
        row.contains("copied"),
        "the toast is not on the second-last row: {text}"
    );
}

/// A tile too small to hold the chip must skip it rather than draw at a
/// negative offset, which would land the toast on someone else's rows.
#[test]
fn a_toast_is_skipped_when_the_tile_cannot_hold_it() {
    for (w, h) in [(1u16, 1u16), (4, 2), (6, 1)] {
        let buf = render(40, 10, |f| draw_toast(f, Rect::new(0, 0, w, h), "copied"));
        assert_eq!(buf.area.width, 40, "it resized the frame at {w}x{h}");
    }
}

/// Every mode has to name itself in the footer, or a modal is on screen
/// with nothing saying which keys are live.
#[test]
fn every_mode_gets_its_own_footer_tag() {
    let mut seen: Vec<String> = Vec::new();
    for mode in [
        Mode::Spawn(crate::app::SpawnState::new(PathBuf::from("/tmp"))),
        Mode::Rename(crate::app::RenameState {
            session_id: 1,
            buf: String::new(),
        }),
        Mode::ConfirmDetach(1),
        Mode::Scrollback(1),
        Mode::Help,
        Mode::Reorder,
    ] {
        let mut app = app_with(1);
        app.mode = mode;
        let line = text(&footer_for(&app));
        assert!(!line.trim().is_empty(), "a mode drew an empty footer");
        seen.push(line);
    }
    for (i, a) in seen.iter().enumerate() {
        for b in seen.iter().skip(i + 1) {
            assert_ne!(a, b, "two modes share a footer: {a}");
        }
    }
}

/// The prefix row wins over the mode row: once the prefix is down, the
/// chords it lists are the live ones whatever mode is underneath.
#[test]
fn the_prefix_row_replaces_the_mode_row_while_the_prefix_is_down() {
    let mut app = app_with(1);
    app.mode = Mode::Help;
    let without = text(&footer_for(&app));
    app.prefix_pending = true;
    let with = text(&footer_for(&app));
    assert!(with.contains("PREFIX"), "{with}");
    assert_ne!(with, without);
}

#[test]
fn every_footer_key_label_comes_from_the_chord_table() {
    let prefix = text(&prefix_footer());
    for c in [
        &keys::PREFIX_SPAWN,
        &keys::PREFIX_PICKER,
        &keys::PREFIX_RENAME,
        &keys::PREFIX_DETACH,
        &keys::PREFIX_TOGGLE_SIDEBAR,
        &keys::PREFIX_HELP,
        &keys::PREFIX_QUIT,
    ] {
        assert!(
            prefix.contains(c.label),
            "the prefix row does not name {:?}: {prefix}",
            c.label
        );
    }
}
