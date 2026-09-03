use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// A selected row is marked by its background alone. It used to also carry
/// a 1-cell accent-coloured bar down its left edge; nothing should draw a
/// glyph there now.
#[test]
fn a_selected_row_is_background_only() {
    let mut term = Terminal::new(TestBackend::new(8, 3)).expect("backend");
    term.draw(|f| selection_bg(f, Rect::new(0, 0, 8, 3)))
        .expect("draw");
    let buf = term.backend().buffer();

    for y in 0..3 {
        for x in 0..8 {
            let cell = &buf[(x, y)];
            assert_eq!(
                cell.symbol(),
                " ",
                "cell ({x},{y}) draws {:?}; a selected row should be plain background",
                cell.symbol()
            );
            assert_eq!(
                cell.bg,
                theme::BG_ACTIVE,
                "cell ({x},{y}) is not the selection background"
            );
        }
    }
}

#[test]
fn viewport_window_no_scroll() {
    assert_eq!(viewport_window(0, 5, 10), (0, 5));
    assert_eq!(viewport_window(4, 5, 10), (0, 5));
}

#[test]
fn viewport_window_scroll_to_keep_selection_visible() {
    // height=3, total=10, selected=4 → window slides to [2,5)
    assert_eq!(viewport_window(4, 10, 3), (2, 5));
    // selected at the end → window is the last `height` items
    assert_eq!(viewport_window(9, 10, 3), (7, 10));
}

#[test]
fn viewport_window_clamps_end_at_total() {
    assert_eq!(viewport_window(2, 5, 10), (0, 5));
}

#[test]
fn truncate_keeps_short_strings_intact() {
    assert_eq!(truncate("abc", 10), "abc");
    assert_eq!(truncate("abc", 3), "abc");
}

#[test]
fn truncate_prepends_ellipsis_and_keeps_tail() {
    // max=4 → keep 3 trailing chars + leading "…"
    assert_eq!(truncate("abcdef", 4), "…def");
    // multibyte: count chars, not bytes
    assert_eq!(truncate("αβγδε", 3), "…δε");
}

#[test]
fn pad_right_pads_to_width() {
    assert_eq!(pad_right("hi", 5), "hi   ");
    assert_eq!(pad_right("", 3), "   ");
}

#[test]
fn pad_right_passes_through_when_already_wide_enough() {
    assert_eq!(pad_right("exactly5", 8), "exactly5");
    assert_eq!(pad_right("longer than width", 5), "longer than width");
}

// collapse_cwd reads $HOME, so the tests can't mutate it without racing
// other tests. Both tests use whatever HOME the harness ran with, plus a
// path crafted from it.
#[test]
fn collapse_cwd_substitutes_home_prefix() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let home = home.to_string_lossy().into_owned();
    let input = format!("{}/code/x", home);
    assert_eq!(collapse_cwd(&input), "~/code/x");
}

#[test]
fn collapse_cwd_elides_deep_paths() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let home = home.to_string_lossy().into_owned();
    let input = format!("{}/a/b/c/d/e/leaf", home);
    let out = collapse_cwd(&input);
    assert!(out.starts_with("~/a/"), "got {out:?}");
    assert!(out.ends_with("/e/leaf"), "got {out:?}");
    assert!(out.contains("/…/"), "got {out:?}");
}

#[test]
fn collapse_cwd_leaves_non_home_paths_untouched_apart_from_elision() {
    let out = collapse_cwd("/var/log");
    assert_eq!(out, "/var/log");
}

// ---------------------------------------------------------------------------
// A name too long for its column
// ---------------------------------------------------------------------------

#[test]
fn a_name_that_fits_does_not_scroll() {
    for t in [0, 500, 5_000, 60_000] {
        assert_eq!(marquee("gbsp-813", 13, t), "gbsp-813", "at {t}ms");
    }
    assert_eq!(marquee("exactly-13-ch", 13, 9_999), "exactly-13-ch");
}

#[test]
fn a_zero_width_column_is_left_alone() {
    assert_eq!(marquee("anything", 0, 1234), "anything");
}

/// Every pass starts held at the beginning, so the start of the name is always
/// readable rather than only catchable mid-slide.
#[test]
fn a_long_name_is_held_at_its_start_first() {
    let name = "tool-access-management";
    for t in [0, 400, MARQUEE_HOLD_MS - 1, MARQUEE_HOLD_MS] {
        assert_eq!(marquee(name, 13, t), "tool-access-m", "at {t}ms");
    }
    assert_eq!(
        marquee(name, 13, MARQUEE_HOLD_MS + MARQUEE_STEP_MS),
        "ool-access-ma",
        "the first column should shift one step after the hold"
    );
}

#[test]
fn a_long_name_slides_one_column_at_a_time() {
    let name = "tool-access-management";
    let at = |step: u64| marquee(name, 13, MARQUEE_HOLD_MS + step * MARQUEE_STEP_MS);
    assert_eq!(at(0), "tool-access-m");
    assert_eq!(at(1), "ool-access-ma");
    assert_eq!(at(2), "ol-access-man");
    assert_eq!(at(9), "ss-management");
}

/// The window is always exactly the column width, or the age column shifts as
/// the name slides.
#[test]
fn every_frame_fills_the_column_exactly() {
    let name = "a-really-quite-long-session-name";
    for step in 0..80 {
        let shown = marquee(name, 13, MARQUEE_HOLD_MS + step * MARQUEE_STEP_MS);
        assert_eq!(shown.chars().count(), 13, "step {step}: {shown:?}");
    }
}

/// Past the end of the name the start comes back round, with the gap between
/// them, so it reads as one name cycling and not as two.
#[test]
fn the_name_comes_back_round_after_a_gap() {
    let name = "tool-access-management";
    let at = |step: u64| marquee(name, 13, MARQUEE_HOLD_MS + step * MARQUEE_STEP_MS);
    let wrapped = at(12);
    assert!(
        wrapped.contains('\u{b7}'),
        "the gap should be visible: {wrapped:?}"
    );
    let seen: Vec<String> = (0..40).map(at).collect();
    assert!(
        seen.contains(&"tool-access-m".to_string()),
        "the start never came back round"
    );
}

/// The cycle is the wall clock modulo one pass, so it never drifts or jumps.
#[test]
fn the_cycle_repeats_on_the_clock() {
    let name = "tool-access-management";
    let span = name.chars().count() + 3; // the name plus the gap
    let pass = MARQUEE_HOLD_MS + span as u64 * MARQUEE_STEP_MS;
    for t in [0, 700, 2_345, 9_876] {
        assert_eq!(
            marquee(name, 13, t),
            marquee(name, 13, t + pass),
            "at {t}ms"
        );
    }
}
