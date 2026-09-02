use super::*;
use crate::ui::popups::harness::{
    assert_inside, assert_legible, chips, painted_bounds, render, text, try_render,
};

/// Every `keys::PREFIX_*` chord, read out of the keys source itself so a
/// chord added there turns up here without anyone editing this test.
fn prefix_chords() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for line in include_str!("../keys.rs").lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pub const PREFIX_") {
            let name = rest.split(':').next().unwrap_or_default().trim();
            pending = Some(format!("PREFIX_{name}"));
        } else if let Some(rest) = line.strip_prefix("label: \"")
            && let Some(name) = pending.take()
        {
            out.push((name, rest.trim_end_matches("\",").to_string()));
        }
    }
    out
}

#[test]
fn the_chord_reader_still_understands_the_keys_source() {
    let found = prefix_chords();
    assert!(
        found.contains(&("PREFIX_QUIT".to_string(), "q".to_string())),
        "the keys.rs reader lost PREFIX_QUIT, so the coverage test below proves nothing: {found:?}"
    );
    assert!(
        found.contains(&("PREFIX_HELP".to_string(), "?".to_string())),
        "the keys.rs reader lost PREFIX_HELP: {found:?}"
    );
    for (name, label) in &found {
        assert!(!label.is_empty(), "keys::{name} parsed with an empty label");
    }
}

#[test]
fn every_prefix_chord_keys_defines_has_a_chip_in_the_help() {
    let buf = render(100, 40, |f| draw(f, Rect::new(0, 0, 100, 40)));
    let listed = chips(&buf);

    for (name, label) in prefix_chords() {
        assert!(
            listed.iter().any(|chip| chip.contains(&label)),
            "keys::{name} ({label:?}) has no chip in the help sheet; chips are {listed:?}"
        );
    }
}

#[test]
fn the_badge_legend_uses_the_glyphs_the_sidebar_draws() {
    let buf = render(100, 40, |f| draw(f, Rect::new(0, 0, 100, 40)));
    let out = text(&buf);

    for glyph in [
        theme::glyph::CONNECTION,
        theme::glyph::EXITED,
        theme::glyph::PERMISSION,
    ] {
        assert!(
            out.contains(glyph),
            "the badge legend never draws {glyph:?}:\n{out}"
        );
    }
}

#[test]
fn it_groups_the_chords_and_says_how_to_close() {
    let buf = render(100, 40, |f| draw(f, Rect::new(0, 0, 100, 40)));
    let out = text(&buf);

    for needle in [
        "cmux keys",
        "Prefix chords",
        "Mouse",
        "Sidebar badges",
        "press any key to close",
    ] {
        assert!(out.contains(needle), "the sheet lacks {needle:?}:\n{out}");
    }
    assert_legible(&buf, "help");
}

#[test]
fn it_stays_inside_the_rect_it_is_handed() {
    let area = Rect::new(10, 2, 100, 40);
    let buf = render(120, 44, |f| draw(f, area));
    assert_inside(&buf, area, "the help sheet");
}

#[test]
fn it_survives_a_terminal_smaller_than_the_sheet() {
    let small = try_render(20, 5, |f| draw(f, Rect::new(0, 0, 20, 5)))
        .unwrap_or_else(|e| panic!("the help sheet dies in a 20x5 terminal: {e}"));
    assert!(
        text(&small).contains("cmux keys"),
        "at 20x5 the sheet drew nothing readable:\n{}",
        text(&small)
    );

    let tiny = try_render(1, 1, |f| draw(f, Rect::new(0, 0, 1, 1)))
        .unwrap_or_else(|e| panic!("the help sheet dies in a 1x1 terminal: {e}"));
    assert!(
        painted_bounds(&tiny).is_some(),
        "at 1x1 the sheet drew nothing at all"
    );
}
