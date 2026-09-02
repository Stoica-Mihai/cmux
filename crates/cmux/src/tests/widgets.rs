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
