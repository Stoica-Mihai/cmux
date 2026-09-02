use super::*;
use std::path::PathBuf;
use std::sync::mpsc;

fn session() -> Session {
    let (tx, _rx) = mpsc::channel();
    Session::new_daemon(
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
    )
    .0
}

/// No badge state renders a bare dot. It read as decoration next to the
/// other glyphs rather than as a status.
#[test]
fn no_badge_state_is_a_bare_dot() {
    let mut s = session();
    let states = [
        (false, false, SessionStatus::Unknown, 0u64),
        (true, true, SessionStatus::Unknown, 0),
        (true, false, SessionStatus::Busy, 0),
        (true, false, SessionStatus::Idle, 60_000),
        (true, false, SessionStatus::Unknown, 10_000),
        (true, false, SessionStatus::Unknown, 60_000),
    ];
    for (alive, attention, status, age) in states {
        s.attention = attention;
        s.status = status;
        let (glyph, _) = sidebar_badge(&s, alive, age);
        assert_ne!(glyph, "·", "a badge still renders a bare dot: {glyph:?}");
    }
}

/// One glyph means one thing across the sidebar and the resume picker: the
/// dot is green while the session runs and dimmed while it does not. How long
/// it has been idle is the age column's job, not the colour's.
#[test]
fn a_session_that_is_not_running_is_the_pickers_dimmed_dot() {
    let mut s = session();
    s.attention = false;
    s.status = SessionStatus::Unknown;

    for age_ms in [10_000, 60_000, 3_600_000] {
        let (glyph, color) = sidebar_badge(&s, true, age_ms);
        assert_eq!(glyph, theme::glyph::CONNECTION, "at {age_ms}ms");
        assert_eq!(color, theme::FG_DIM, "at {age_ms}ms");
    }

    s.status = SessionStatus::Idle;
    let (glyph, color) = sidebar_badge(&s, true, 60_000);
    assert_eq!(glyph, theme::glyph::CONNECTION);
    assert_eq!(color, theme::FG_DIM);
}

/// Running is the same dot in green. One shape, two colours, matching the
/// resume picker.
#[test]
fn a_running_session_is_the_green_dot() {
    let mut s = session();
    s.attention = false;
    s.status = SessionStatus::Busy;

    let (glyph, color) = sidebar_badge(&s, true, 60_000);
    assert_eq!(glyph, theme::glyph::CONNECTION);
    assert_eq!(color, theme::ACCENT_GREEN);
}

/// The tile title numbers the session the same way the sidebar row does.
#[test]
fn the_tile_title_numbers_without_brackets() {
    let s = session();
    let title = tile_title(&s, true, false, 1);
    assert!(title.contains(" 1 "), "no bare number: {title:?}");
    assert!(!title.contains('['), "still bracketed: {title:?}");
}

/// The sidebar scrolls so the focused session is always drawn. Rendering from
/// row 0 hid everything past the fold, the focused row included.
#[test]
fn the_sidebar_window_always_contains_the_focus() {
    let rows_fit = 6usize;
    for total in [1usize, 6, 7, 12, 40] {
        for focus in 0..total {
            let (start, end) = crate::ui::widgets::viewport_window(focus, total, rows_fit);
            assert!(
                (start..end).contains(&focus),
                "focus {focus} of {total} fell outside {start}..{end}"
            );
            assert!(end - start <= rows_fit, "window taller than the sidebar");
            assert!(end <= total, "window past the end of the list");
        }
    }
}

/// The row's columns have to add up to the sidebar's width exactly: a cell
/// over and the age wraps onto the next session, a cell under and the row has
/// the ragged padding this layout replaced.
#[test]
fn the_row_budget_fills_the_width_exactly() {
    for width in 16u16..60 {
        for count in [1usize, 9, 10, 99, 100] {
            for danger in [false, true] {
                let l = SidebarLayout::new(count, danger, width);
                if l.name_w > 4 {
                    assert_eq!(
                        l.row_width(),
                        width as usize,
                        "width {width}, {count} sessions, danger {danger}"
                    );
                }
            }
        }
    }
}

/// The number column grows with the list, so a two-digit session is not cut
/// in half.
#[test]
fn the_number_column_grows_with_the_list() {
    assert_eq!(SidebarLayout::new(9, false, 30).num_w, 1);
    assert_eq!(SidebarLayout::new(10, false, 30).num_w, 2);
    assert_eq!(SidebarLayout::new(100, false, 30).num_w, 3);
    assert_eq!(SidebarLayout::new(0, false, 30).num_w, 1, "an empty list");
}

/// The danger column exists only when a session carries the flag, so nothing
/// is spent on a marker no row shows.
#[test]
fn the_danger_column_is_only_there_when_needed() {
    let tame = SidebarLayout::new(3, false, 30);
    let risky = SidebarLayout::new(3, true, 30);
    assert_eq!(
        tame.name_w,
        risky.name_w + 2,
        "the marker takes its space from the name"
    );
}

/// A sidebar too narrow to hold everything still leaves a readable name
/// rather than a negative width.
#[test]
fn a_narrow_sidebar_keeps_a_usable_name_column() {
    for width in 0u16..16 {
        let l = SidebarLayout::new(5, true, width);
        assert!(l.name_w >= 4, "width {width} gave name_w {}", l.name_w);
    }
}
