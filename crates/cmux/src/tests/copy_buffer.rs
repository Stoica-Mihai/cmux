use super::*;

/// A screen of numbered rows, as a scrolling program shows them: `first` is
/// the number on the top row.
fn screen(first: usize, height: usize) -> Vec<Row> {
    (0..height)
        .map(|i| Row::new(format!("line {:02}", first + i), false))
        .collect()
}

/// The same, with the live chrome a full-screen program keeps at the bottom.
fn screen_with_chrome(first: usize, height: usize, tick: usize) -> Vec<Row> {
    let mut rows = screen(first, height - 2);
    rows.push(Row::new("-".repeat(20), false));
    rows.push(Row::new(format!("spinner {tick} | ctx 6%"), false));
    rows
}

#[test]
fn an_unmoved_screen_has_no_displacement() {
    let s = screen(10, 12);
    assert_eq!(displacement(&s, &s), Some(0));
}

/// Revealing earlier rows moves the held content down the screen.
#[test]
fn scrolling_back_displaces_the_content_downwards() {
    let old = screen(10, 12);
    for step in 1..=4 {
        let new = screen(10 - step, 12);
        assert_eq!(
            displacement(&old, &new),
            Some(step as isize),
            "a {step}-row scroll back"
        );
    }
}

#[test]
fn scrolling_forward_displaces_the_content_upwards() {
    let old = screen(10, 12);
    for step in 1..=4 {
        let new = screen(10 + step, 12);
        assert_eq!(
            displacement(&old, &new),
            Some(-(step as isize)),
            "a {step}-row scroll forward"
        );
    }
}

/// The step is derived, not assumed, so an uneven one is read correctly. This
/// is what claude does: 2 rows on one detent, 3 on the next.
#[test]
fn an_uneven_scroll_step_is_read_from_the_overlap() {
    let old = screen(31, 15);
    assert_eq!(displacement(&old, &screen(29, 15)), Some(2));
    assert_eq!(displacement(&old, &screen(26, 15)), Some(5));
    assert_eq!(displacement(&old, &screen(23, 15)), Some(8));
}

/// The rows a program rewrites every frame must not defeat the match.
#[test]
fn live_chrome_does_not_break_the_match() {
    let old = screen_with_chrome(31, 15, 1);
    let new = screen_with_chrome(28, 15, 999);
    assert_eq!(displacement(&old, &new), Some(3));
}

#[test]
fn two_unrelated_screens_do_not_match() {
    let old = screen(10, 12);
    let new: Vec<Row> = (0..12)
        .map(|i| Row::new(format!("something else {i}"), false))
        .collect();
    assert_eq!(displacement(&old, &new), None);
}

/// A band of blank rows matches at any offset, so blanks must not count.
#[test]
fn a_blank_screen_does_not_match_at_random() {
    let blanks: Vec<Row> = (0..12).map(|_| Row::new("   ", false)).collect();
    assert_eq!(displacement(&blanks, &blanks), None);
}

#[test]
fn a_screen_too_short_to_have_a_middle_is_refused() {
    let tiny = screen(1, 6);
    assert_eq!(displacement(&tiny, &tiny), None);
}

// ---------------------------------------------------------------------------
// Stitching
// ---------------------------------------------------------------------------

#[test]
fn a_fresh_buffer_is_the_screen_it_started_from() {
    let b = CopyBuffer::new(screen(1, 10), (10, 80));
    assert_eq!(b.rows().len(), 10);
    assert_eq!(b.top(), 0);
    assert_eq!(b.line_at(0), Some(0));
    assert_eq!(b.line_at(9), Some(9));
    assert_eq!(b.line_at(10), None);
}

/// Scrolling back reveals rows above what is held; they go on the front and
/// the viewport stays pointed at the same text.
#[test]
fn scrolling_back_grows_the_buffer_upwards() {
    let mut b = CopyBuffer::new(screen(10, 12), (12, 80));
    assert!(
        b.stitch(screen(7, 12)).is_some(),
        "a 3-row scroll back should stitch"
    );

    assert_eq!(b.len(), 15, "three rows were revealed");
    assert_eq!(b.rows()[0].text, "line 07");
    assert_eq!(b.rows()[14].text, "line 21");
    assert_eq!(b.top(), 0, "the viewport is now at the top of the buffer");
    assert_eq!(
        b.line_at(0).map(|l| b.rows()[l].text.clone()),
        Some("line 07".to_string())
    );
}

/// Scrolling forward reveals rows below; they go on the end and the viewport
/// moves down with them.
#[test]
fn scrolling_forward_grows_the_buffer_downwards() {
    let mut b = CopyBuffer::new(screen(10, 12), (12, 80));
    assert!(b.stitch(screen(13, 12)).is_some(), "a 3-row scroll forward");

    assert_eq!(b.len(), 15);
    assert_eq!(b.rows()[0].text, "line 10");
    assert_eq!(b.rows()[14].text, "line 24");
    assert_eq!(b.top(), 3);
    assert_eq!(
        b.line_at(0).map(|l| b.rows()[l].text.clone()),
        Some("line 13".to_string())
    );
}

/// Several steps in a row accumulate, which is the whole point: a selection
/// dragged for a while spans far more than one screen.
#[test]
fn repeated_scrolls_accumulate() {
    let mut b = CopyBuffer::new(screen(31, 15), (15, 80));
    for first in [29, 26, 23, 20, 15] {
        assert!(b.stitch(screen(first, 15)).is_some(), "scroll to {first}");
    }
    assert_eq!(b.rows()[0].text, "line 15");
    assert_eq!(b.rows()[b.len() - 1].text, "line 45");
    assert_eq!(b.len(), 31, "15 rows through 45");
    assert_eq!(b.top(), 0);
}

/// Scrolling back and then forward returns the viewport without duplicating
/// any row.
#[test]
fn scrolling_back_then_forward_does_not_duplicate() {
    let mut b = CopyBuffer::new(screen(10, 12), (12, 80));
    assert!(b.stitch(screen(7, 12)).is_some());
    assert!(b.stitch(screen(10, 12)).is_some());

    assert_eq!(b.len(), 15, "no rows were added twice");
    assert_eq!(b.top(), 3);
    let texts: Vec<&str> = b.rows().iter().map(|r| r.text.as_str()).collect();
    let mut sorted = texts.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), texts.len(), "a row appears twice: {texts:?}");
}

/// A screen that cannot be matched leaves the buffer alone. Guessing would
/// attach text the user never dragged over to their selection.
#[test]
fn an_unmatched_screen_is_refused() {
    let mut b = CopyBuffer::new(screen(10, 12), (12, 80));
    let before = b.clone();
    let unrelated: Vec<Row> = (0..12)
        .map(|i| Row::new(format!("unrelated {i}"), false))
        .collect();

    assert!(
        b.stitch(unrelated).is_none(),
        "an unmatched screen must be refused"
    );
    assert_eq!(b, before, "the buffer changed anyway");
}

#[test]
fn an_empty_screen_is_refused() {
    let mut b = CopyBuffer::new(screen(10, 12), (12, 80));
    assert!(b.stitch(Vec::new()).is_none());
    assert_eq!(b.len(), 12);
}

// ---------------------------------------------------------------------------
// Reading text back
// ---------------------------------------------------------------------------

/// The viewport mapping is the inverse of `line_at`, which is what the
/// highlight needs to paint a buffer line at the right row.
#[test]
fn the_viewport_mapping_round_trips() {
    let mut b = CopyBuffer::new(screen(10, 12), (12, 80));
    assert!(b.stitch(screen(7, 12)).is_some());
    for vp in 0..12u16 {
        let line = b.line_at(vp).expect("on screen");
        assert_eq!(b.viewport_row(line, 12), Some(vp));
    }
    let above = b.top();
    assert_eq!(
        b.viewport_row(above.wrapping_sub(1), 12),
        None,
        "off the top"
    );
    assert_eq!(b.viewport_row(b.len() + 5, 12), None, "off the bottom");
}

// ---------------------------------------------------------------------------
// Column-aware extraction, as a flowing selection reads it
// ---------------------------------------------------------------------------

#[test]
fn a_range_starts_and_ends_at_its_columns() {
    let b = CopyBuffer::new(screen(1, 6), (6, 20));
    // "line 01" — from column 5 is "01".
    assert_eq!(b.text_range((0, 5), (0, 6)), "01");
    assert_eq!(b.text_range((0, 5), (1, 3)), "01\nline");
    assert_eq!(
        b.text_range((1, 0), (3, 6)),
        "line 02\nline 03\nline 04",
        "middle lines come whole"
    );
}

#[test]
fn a_range_is_normalised_when_dragged_upwards() {
    let b = CopyBuffer::new(screen(1, 6), (6, 20));
    assert_eq!(b.text_range((3, 6), (1, 0)), b.text_range((1, 0), (3, 6)));
}

#[test]
fn a_range_across_a_wrap_reads_as_one_line() {
    let rows = vec![
        Row::new("carries on ", true),
        Row::new("across the wrap", false),
        Row::new("next", false),
    ];
    let b = CopyBuffer::new(rows, (3, 15));
    assert_eq!(b.text_range((0, 0), (1, 14)), "carries on across the wrap");
}

#[test]
fn a_range_past_the_end_is_clamped() {
    let b = CopyBuffer::new(screen(1, 3), (3, 20));
    assert_eq!(b.text_range((0, 0), (99, 99)), "line 01\nline 02\nline 03");
}

#[test]
fn a_single_cell_range_is_one_character() {
    let b = CopyBuffer::new(screen(1, 3), (3, 20));
    assert_eq!(b.text_range((0, 0), (0, 0)), "l");
}

/// A program that fills only part of the tile leaves blank rows under its
/// output. Appending those built a blank band the next screen could not be
/// matched against, and the stitching stopped after a few steps.
#[test]
fn trailing_blank_rows_are_not_collected() {
    /// 15 content rows, two of chrome, three blank — the shape a full-screen
    /// program in a taller tile actually produces.
    fn padded(first: usize, tick: usize) -> Vec<Row> {
        let mut rows = screen(first, 15);
        rows.push(Row::new("-".repeat(40), false));
        rows.push(Row::new(format!("spinner {tick} | ctx 6%"), false));
        rows.extend((0..3).map(|_| Row::new("", false)));
        rows
    }

    let mut b = CopyBuffer::new(padded(1, 1), (20, 40));
    for (step, first) in [4usize, 7, 10, 13, 16, 19, 22].into_iter().enumerate() {
        assert!(
            b.stitch(padded(first, step + 2)).is_some(),
            "step {step} to line {first} was refused"
        );
    }

    let texts: Vec<&str> = b.rows().iter().map(|r| r.text.as_str()).collect();
    assert!(
        texts.iter().any(|t| t.starts_with("line 01")),
        "the first screen is gone: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("line 36")),
        "the last screen never arrived: {texts:?}"
    );
    let blanks = texts.iter().filter(|t| t.trim().is_empty()).count();
    assert!(blanks <= 3, "{blanks} blank rows were collected: {texts:?}");
}
