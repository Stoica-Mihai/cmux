use super::*;

fn saved(label: &str, manually_renamed: bool) -> persist::PersistedSession {
    persist::PersistedSession {
        cwd: PathBuf::from("/tmp"),
        label: label.to_string(),
        dangerous: false,
        resume_id: None,
        manually_renamed,
    }
}

/// Both directions. A name the user typed is pinned, so the probe cannot
/// undo it. A label merely carried over from the last run is not, or the
/// probe can never rename the session and the TUI ends up showing a
/// different name from the browser.
#[test]
fn only_a_user_chosen_name_is_pinned_on_the_daemon() {
    assert!(should_pin_label(&saved("mine", true)));
    assert!(!should_pin_label(&saved("saved-dirname", false)));
    assert!(!should_pin_label(&saved("", true)));
    assert!(!should_pin_label(&saved("", false)));
}

/// A drag that leaves the tile selects up to the edge it crossed. It used to
/// stop updating, freezing the selection mid-drag.
#[test]
fn a_pointer_past_an_edge_clamps_into_the_tile() {
    let tile = ratatui::layout::Rect {
        x: 10,
        y: 5,
        width: 20,
        height: 8,
    };
    assert_eq!(tile_cell(5, 10, tile), Some((0, 0)), "top-left corner");
    assert_eq!(
        tile_cell(12, 29, tile),
        Some((7, 19)),
        "bottom-right corner"
    );
    assert_eq!(tile_cell(0, 0, tile), Some((0, 0)), "above and left of it");
    assert_eq!(tile_cell(200, 200, tile), Some((7, 19)), "below and right");
    assert_eq!(tile_cell(9, 3, tile), Some((4, 0)), "left of the tile");
}

#[test]
fn a_tile_with_no_area_yields_no_cell() {
    let flat = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 4,
    };
    assert_eq!(tile_cell(2, 2, flat), None);
}

// ---------------------------------------------------------------------------
// Multi-click selection
// ---------------------------------------------------------------------------

/// A press at one cell, as the terminal reports it.
fn press_at(app: &mut App, tile: ratatui::layout::Rect, col: u16, row: u16) {
    let me = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: tile.x + col,
        row: tile.y + row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    mouse_press(app, me, tile, true);
}

/// An app with one session whose grid holds `text` on its first row.
fn app_for_selection(text: &str) -> (App, ratatui::layout::Rect) {
    let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
    let (tx, _rx) = std::sync::mpsc::channel();
    let (sess, _slot) = session::Session::new_daemon(
        1,
        "s".into(),
        PathBuf::from("/tmp"),
        false,
        None,
        10,
        80,
        None,
        1,
        tx,
        0,
    );
    app.sessions.push(sess);
    app.focus = 0;
    if let Ok(mut p) = app.sessions[0].parser.lock() {
        p.process(text.as_bytes());
    }
    let tile = ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 10,
    };
    app.last_tile_area = Some(tile);
    (app, tile)
}

/// The terminal reports no click count, so presses running together on one
/// cell are what makes a double or triple click. The count cycles, so a
/// fourth press is a single click again.
#[test]
fn presses_on_one_cell_count_up_and_then_cycle() {
    let (mut app, tile) = app_for_selection("the quick brown fox");
    use crate::copy_buffer::Granularity::*;
    for want in [Char, Word, Line, Char, Word] {
        press_at(&mut app, tile, 6, 0);
        let gran = app.sessions[0].drag.as_ref().expect("a drag").gran;
        assert_eq!(gran, want, "click {}", app.sessions[0].click_count);
    }
}

/// A press somewhere else starts counting again, or dragging across the
/// screen would keep escalating the granularity.
#[test]
fn a_press_on_another_cell_starts_over() {
    let (mut app, tile) = app_for_selection("the quick brown fox");
    press_at(&mut app, tile, 6, 0);
    press_at(&mut app, tile, 6, 0);
    assert_eq!(
        app.sessions[0].drag.as_ref().unwrap().gran,
        crate::copy_buffer::Granularity::Word
    );
    press_at(&mut app, tile, 17, 0);
    assert_eq!(
        app.sessions[0].drag.as_ref().unwrap().gran,
        crate::copy_buffer::Granularity::Char,
        "a press on a different cell is a single click"
    );
}

/// A press long after the last one is a single click again.
#[test]
fn a_press_after_the_gap_lapses_starts_over() {
    let (mut app, tile) = app_for_selection("the quick brown fox");
    press_at(&mut app, tile, 6, 0);
    press_at(&mut app, tile, 6, 0);
    assert_eq!(
        app.sessions[0].drag.as_ref().unwrap().gran,
        crate::copy_buffer::Granularity::Word
    );
    app.sessions[0].last_click_ms = util::now_ms() - (MULTI_CLICK_MS + 1);
    press_at(&mut app, tile, 6, 0);
    assert_eq!(
        app.sessions[0].drag.as_ref().unwrap().gran,
        crate::copy_buffer::Granularity::Char
    );
}

/// End to end through the buffer the copy reads: two presses take the word
/// under the pointer, three take the line.
#[test]
fn two_presses_select_a_word_and_three_select_the_line() {
    let (mut app, tile) = app_for_selection("the quick brown fox");
    press_at(&mut app, tile, 6, 0);
    press_at(&mut app, tile, 6, 0);
    let s = &app.sessions[0];
    let (buf, drag) = (s.copy.as_ref().unwrap(), s.drag.as_ref().unwrap());
    let (lo, hi) = buf.snap(drag.anchor, drag.tip, drag.gran);
    assert_eq!(buf.text_range(lo, hi), "quick");

    press_at(&mut app, tile, 6, 0);
    let s = &app.sessions[0];
    let (buf, drag) = (s.copy.as_ref().unwrap(), s.drag.as_ref().unwrap());
    let (lo, hi) = buf.snap(drag.anchor, drag.tip, drag.gran);
    assert_eq!(buf.text_range(lo, hi), "the quick brown fox");
}
