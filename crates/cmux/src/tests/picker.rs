use super::*;
use crate::transcripts::Transcript;
use crate::ui::popups::harness::{
    assert_inside, assert_legible, painted_bounds, render, row, text, try_render,
};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const FULL: Rect = Rect {
    x: 0,
    y: 0,
    width: 100,
    height: 30,
};
const CWD: &str = "/tmp/proj";

fn transcript(id: &str, cwd: &str, title: Option<&str>) -> Transcript {
    Transcript {
        session_id: id.to_string(),
        path: PathBuf::from(cwd).join(format!("{id}.jsonl")),
        cwd: PathBuf::from(cwd),
        forked_from: None,
        mtime: SystemTime::now() - Duration::from_secs(90),
        file_size: 4096,
        custom_title: title.map(str::to_string),
    }
}

/// A settled picker over a fixed list. Built through `new` and then
/// overwritten, so no test reads the user's real `~/.claude/projects`.
fn state(all: Vec<Transcript>, selected: usize) -> PickerState {
    let mut s = PickerState::new();
    s.items = (0..all.len()).collect();
    s.all = all;
    s.selected = selected;
    s.dangerous = false;
    s.filter = String::new();
    s.previews.clear();
    s.scanning = false;
    s
}

fn layout_of(name_w: usize, cwd_w: usize) -> RowLayout {
    RowLayout {
        name_w,
        fork_w: 0,
        cwd_w,
    }
}

fn one_row(t: Transcript, is_sel: bool) -> ratatui::buffer::Buffer {
    let s = state(vec![transcript(&t.session_id, CWD, None)], 0);
    render(60, 1, |f| {
        draw_row(
            f,
            Rect::new(0, 0, 60, 1),
            &s,
            &t,
            is_sel,
            &layout_of(14, 20),
        )
    })
}

/// Cells in row 0 carrying the accent the selected row is drawn in.
fn accented_cells(buf: &ratatui::buffer::Buffer) -> usize {
    (0..buf.area.width)
        .filter(|x| {
            let cell = &buf[(*x, 0u16)];
            cell.fg == theme::BORDER_FOCUS && cell.modifier.contains(Modifier::BOLD)
        })
        .count()
}

#[test]
fn the_title_counts_the_transcripts_it_found() {
    let s = state(
        vec![
            transcript("aaaaaaaa1", "/tmp/one", None),
            transcript("bbbbbbbb2", "/tmp/two", None),
            transcript("cccccccc3", "/tmp/three", None),
        ],
        0,
    );
    let buf = render(100, 30, |f| draw(f, FULL, &s));
    let out = text(&buf);

    assert!(
        out.contains("Resume past session (3 found)"),
        "the title does not count what it listed:\n{out}"
    );
    assert_legible(&buf, "picker");
}

/// The other state of the same title: until the scan lands there is no count
/// to give, and the title has to say so rather than claim zero.
#[test]
fn the_title_says_it_is_still_scanning() {
    let mut s = state(Vec::new(), 0);
    s.scanning = true;
    let out = text(&render(100, 30, |f| draw(f, FULL, &s)));
    assert!(
        out.contains("Resume past session (scanning...)"),
        "a scan in progress reads as an empty result:\n{out}"
    );
}

#[test]
fn an_empty_filter_invites_typing() {
    let s = state(vec![transcript("aaaaaaaa1", "/tmp/one", None)], 0);
    let buf = render(60, 1, |f| draw_filter_line(f, Rect::new(0, 0, 60, 1), &s));
    let out = text(&buf);

    assert!(out.contains("filter:"), "no filter prompt:\n{out}");
    assert!(
        out.contains("(type to search by cwd or --name)"),
        "an empty filter does not say what it searches:\n{out}"
    );
}

#[test]
fn a_typed_filter_shows_what_it_matched() {
    let mut s = state(
        vec![
            transcript("aaaaaaaa1", "/tmp/one", None),
            transcript("bbbbbbbb2", "/tmp/two", None),
            transcript("cccccccc3", "/tmp/three", None),
        ],
        0,
    );
    s.filter = "tw".to_string();
    s.items = vec![1];

    let buf = render(60, 1, |f| draw_filter_line(f, Rect::new(0, 0, 60, 1), &s));
    let out = text(&buf);

    assert!(
        out.contains("filter: tw"),
        "the typed filter is missing:\n{out}"
    );
    assert!(
        out.contains("(1/3)"),
        "the matched-of-total count is missing:\n{out}"
    );
}

#[test]
fn an_empty_list_says_where_it_looked() {
    let s = state(Vec::new(), 0);
    let buf = render(60, 5, |f| draw_rows(f, Rect::new(0, 0, 60, 5), &s));
    let out = text(&buf);
    assert!(
        out.contains("(no past sessions found in ~/.claude/projects)"),
        "an empty picker leaves the user guessing:\n{out}"
    );
}

#[test]
fn a_selected_row_is_accented_on_the_selection_background() {
    let sel = one_row(transcript("aaaaaaaa1", CWD, None), true);
    let plain = one_row(transcript("aaaaaaaa1", CWD, None), false);

    assert!(
        text(&sel).contains(&collapse_cwd(CWD)),
        "the selected row lost its cwd"
    );
    assert!(
        text(&plain).contains(&collapse_cwd(CWD)),
        "the plain row lost its cwd"
    );

    assert_eq!(
        sel[(0u16, 0u16)].bg,
        theme::BG_ACTIVE,
        "the selected row has no selection background"
    );
    assert_eq!(
        plain[(0u16, 0u16)].bg,
        Color::Reset,
        "the unselected row is painted with a selection background"
    );
    assert!(
        accented_cells(&sel) > 0,
        "the selected row draws nothing in the focus accent, so selection is invisible"
    );
    assert_eq!(
        accented_cells(&plain),
        0,
        "the unselected row is drawn in the focus accent too"
    );
}

#[test]
fn a_long_cwd_is_cut_to_its_column_with_a_leading_ellipsis() {
    let long = format!("/{}", "d".repeat(60));
    let t = transcript("aaaaaaaa1", &long, None);
    let s = state(vec![transcript("aaaaaaaa1", &long, None)], 0);
    let buf = render(60, 1, |f| {
        draw_row(f, Rect::new(0, 0, 60, 1), &s, &t, false, &layout_of(14, 15))
    });
    let line = row(&buf, 0);

    assert!(line.contains('…'), "a long cwd was not truncated: {line:?}");
    assert!(
        !line.contains(&long),
        "the full 61-char cwd was drawn: {line:?}"
    );
    assert_eq!(
        line.chars().count(),
        60,
        "the row no longer matches the width it was given: {line:?}"
    );
}

#[test]
fn the_name_column_appears_only_when_a_transcript_carries_one() {
    let named = state(vec![transcript("aaaaaaaa1", CWD, Some("mytitle"))], 0);
    let plain = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);

    let with = render(80, 1, |f| draw_rows(f, Rect::new(0, 0, 80, 1), &named));
    let without = render(80, 1, |f| draw_rows(f, Rect::new(0, 0, 80, 1), &plain));

    let with_row = row(&with, 0);
    let without_row = row(&without, 0);
    assert!(
        with_row
            .trim_start()
            .starts_with(&format!("{} mytitle", theme::glyph::CONNECTION)),
        "the name column is missing: {with_row:?}"
    );
    assert!(
        without_row.trim_start().starts_with(&format!(
            "{} {}",
            theme::glyph::CONNECTION,
            collapse_cwd(CWD)
        )),
        "an empty name column is still reserved: {without_row:?}"
    );
}

#[test]
fn the_preview_pane_shows_the_selected_transcript() {
    let mut s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
    s.previews.insert(
        "aaaaaaaa1".to_string(),
        "hello from the transcript".to_string(),
    );

    let loaded = text(&render(100, 30, |f| draw(f, FULL, &s)));
    assert!(
        loaded.contains(" preview "),
        "the preview pane has no heading:\n{loaded}"
    );
    assert!(
        loaded.contains("hello from the transcript"),
        "the loaded preview is not shown:\n{loaded}"
    );

    let pending = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
    let pending = text(&render(100, 30, |f| draw(f, FULL, &pending)));
    assert!(
        pending.contains("(loading...)"),
        "a transcript with no preview yet shows nothing at all:\n{pending}"
    );
}

#[test]
fn the_hint_row_names_every_key_it_offers() {
    let s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
    let out = text(&render(100, 30, |f| draw(f, FULL, &s)));

    for key in [
        keys::PICKER_FILTER_CLEAR.label,
        keys::PICKER_PICK.label,
        keys::PICKER_TOGGLE_DANGER.label,
        keys::PICKER_CANCEL.label,
    ] {
        assert!(out.contains(key), "the hint row lacks {key:?}:\n{out}");
    }
    for label in ["select", "filter", "clear", "open", "cancel"] {
        assert!(out.contains(label), "the hint row lacks {label:?}:\n{out}");
    }
}

#[test]
fn it_stays_inside_the_rect_it_is_handed() {
    let s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
    let area = Rect::new(5, 2, 80, 24);
    let buf = render(100, 30, |f| draw(f, area, &s));
    assert_inside(&buf, area, "the resume picker");
}

#[test]
fn it_survives_a_terminal_smaller_than_the_popup() {
    let s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);

    let small = try_render(20, 5, |f| draw(f, Rect::new(0, 0, 20, 5), &s))
        .unwrap_or_else(|e| panic!("the resume picker dies in a 20x5 terminal: {e}"));
    assert!(
        text(&small).contains("Resume"),
        "at 20x5 the popup drew nothing readable:\n{}",
        text(&small)
    );

    let tiny = try_render(1, 1, |f| draw(f, Rect::new(0, 0, 1, 1), &s))
        .unwrap_or_else(|e| panic!("the resume picker dies in a 1x1 terminal: {e}"));
    assert!(
        painted_bounds(&tiny).is_none(),
        "at 1x1 the picker has no room left after its 1-cell margin, so it should draw nothing"
    );
}
