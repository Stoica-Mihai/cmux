use super::*;
use crate::ui::TileSizes;
use crate::ui::popups::harness::{app_with, render, text as buffer_text};
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
        0,
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

/// The badge ranking decides what a row shows when two states are true at
/// once, so each rank has to beat the one below it.
#[test]
fn the_badge_ranking_puts_exit_above_attention_above_busy() {
    let mut s = session();
    s.attention = true;
    s.status = SessionStatus::Busy;

    let (dead, _) = sidebar_badge(&s, false, 0);
    assert_eq!(dead, theme::glyph::EXITED, "exit should outrank everything");

    let (attn, _) = sidebar_badge(&s, true, 0);
    assert_eq!(
        attn,
        theme::glyph::PERMISSION,
        "attention should outrank busy"
    );

    s.attention = false;
    let (busy, _) = sidebar_badge(&s, true, 60_000);
    assert_ne!(busy, theme::glyph::PERMISSION);
}

/// Recent output counts as running even before the probe says so, or a
/// session that just printed a page looks quiet while it is still working.
/// The glyph is fixed now, so the colour is what has to move.
#[test]
fn recent_output_reads_as_running_without_waiting_for_the_probe() {
    let mut s = session();
    s.status = SessionStatus::Unknown;
    let (_, fresh) = sidebar_badge(&s, true, 100);
    let (_, stale) = sidebar_badge(&s, true, 60_000);
    assert_ne!(fresh, stale, "output age made no difference to the badge");
}

/// A dead tile has to be tellable from a live one at a glance, and the
/// focused one from the rest.
#[test]
fn the_tile_border_separates_dead_focused_and_idle() {
    let s = session();
    let dead = tile_border_color(&s, false, true, false, 0);
    let focused = tile_border_color(&s, true, true, false, 0);
    let idle = tile_border_color(&s, true, false, false, 0);
    let zoomed = tile_border_color(&s, true, true, true, 0);
    assert_ne!(dead, focused);
    assert_ne!(focused, idle);
    assert_ne!(zoomed, focused);
}

#[test]
fn the_tile_cursor_colour_follows_the_status() {
    let mut s = session();
    s.status = SessionStatus::Busy;
    let busy = tile_cursor_bg(&s);
    s.status = SessionStatus::Idle;
    let idle = tile_cursor_bg(&s);
    assert_ne!(busy, idle);

    s.attention = true;
    assert_eq!(
        tile_cursor_bg(&s),
        theme::ACCENT_RED,
        "attention should override the status colour"
    );
}

#[test]
fn the_tile_title_carries_the_number_the_label_and_the_state() {
    let mut s = session();
    s.label = "worker".into();
    let live = tile_title(&s, true, false, 2);
    assert!(live.contains(" 2 "), "no session number: {live}");
    assert!(live.contains("worker"), "{live}");
    assert!(!live.contains("exited"), "{live}");

    let dead = tile_title(&s, false, false, 2);
    assert!(dead.contains("exited"), "a dead tile should say so: {dead}");

    let zoomed = tile_title(&s, true, true, 2);
    assert_ne!(zoomed, live, "a zoomed tile should be marked");
}

#[test]
fn the_dashboard_lists_every_session_in_the_sidebar() {
    let mut app = app_with(&["alpha", "beta", "gamma"]);
    let mut sizes: TileSizes = Vec::new();
    let buf = render(100, 24, |f| {
        draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut sizes)
    });
    let out = buffer_text(&buf);
    for label in ["alpha", "beta", "gamma"] {
        assert!(out.contains(label), "{label} is missing: {out}");
    }
}

#[test]
fn the_dashboard_draws_with_no_sessions_at_all() {
    let mut app = app_with(&[]);
    let mut sizes: TileSizes = Vec::new();
    let buf = render(100, 24, |f| {
        draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut sizes)
    });
    assert_eq!(buf.area.width, 100);
}

/// Both sides of the toggle, because hiding the sidebar is what gives the
/// tiles the extra width.
#[test]
fn hiding_the_sidebar_gives_the_tile_more_width() {
    let mut app = app_with(&["alpha"]);
    let mut with: TileSizes = Vec::new();
    app.show_sidebar = true;
    render(100, 24, |f| {
        draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut with)
    });
    let mut without: TileSizes = Vec::new();
    app.show_sidebar = false;
    render(100, 24, |f| {
        draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut without)
    });
    assert!(
        without[0].2 > with[0].2,
        "hiding the sidebar did not widen the tile: {with:?} then {without:?}"
    );
}

#[test]
fn the_dashboard_survives_a_terminal_too_small_for_its_layout() {
    for (w, h) in [(1u16, 1u16), (4, 3), (10, 4), (20, 6)] {
        let mut app = app_with(&["alpha", "beta"]);
        let mut sizes: TileSizes = Vec::new();
        let buf = render(w, h, |f| {
            draw_dashboard(f, &mut app, Rect::new(0, 0, w, h), &mut sizes)
        });
        assert_eq!(buf.area.width, w, "it drew outside a {w}x{h} frame");
    }
}
/// A new session is created at [`App::tile_size`], and the dashboard draws the
/// focused session at the whole main area. The two must agree: while they did
/// not, every session was born at a size nothing drew it at, so the first
/// focus resized the pty, the program repainted, and the age restarted.
#[test]
fn a_session_is_born_at_the_size_the_dashboard_draws_it_at() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
    app.sessions.push(session());
    app.focus = 0;

    let mut term = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    let mut sizes: crate::ui::TileSizes = Vec::new();
    term.draw(|f| crate::ui::draw(f, &mut app, &mut sizes))
        .expect("draw the dashboard");

    assert_eq!(
        sizes,
        vec![(0, app.tile_size().0, app.tile_size().1)],
        "the drawn tile and the size a new session is spawned at disagree"
    );
}
