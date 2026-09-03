use super::*;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::Processor;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

fn build(rows: usize, cols: usize, bytes: &[u8]) -> Term<VoidListener> {
    let config = TermConfig {
        scrolling_history: 1000,
        ..Default::default()
    };
    let size = TermSize { lines: rows, cols };
    let mut term = Term::new(config, &size, VoidListener);
    let mut proc: Processor = Processor::new();
    proc.advance(&mut term, bytes);
    term
}

#[test]
fn renders_plain_text() {
    let term = build(3, 10, b"hello\r\nworld");
    let area = Rect::new(0, 0, 10, 3);
    let mut buf = Buffer::empty(area);
    TermWidget::new(&term).render(area, &mut buf);
    let row0: String = (0..5)
        .map(|x| buf[(x, 0)].symbol().chars().next().unwrap())
        .collect();
    let row1: String = (0..5)
        .map(|x| buf[(x, 1)].symbol().chars().next().unwrap())
        .collect();
    assert_eq!(row0, "hello");
    assert_eq!(row1, "world");
}

#[test]
fn visible_text_round_trip() {
    let term = build(3, 10, b"line1\r\nline2\r\nline3");
    let text = visible_text(&term);
    let joined: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
    assert_eq!(joined, vec!["line1", "line2", "line3"]);
}

#[test]
fn red_sgr_maps_to_red_fg() {
    let term = build(1, 5, b"\x1b[31mX\x1b[0m");
    let area = Rect::new(0, 0, 5, 1);
    let mut buf = Buffer::empty(area);
    TermWidget::new(&term).render(area, &mut buf);
    assert_eq!(buf[(0, 0)].symbol(), "X");
    assert_eq!(buf[(0, 0)].fg, RColor::Red);
}

const LINK: &[u8] = b"see \x1b]8;;https://example.com/a\x1b\\HERE\x1b]8;;\x1b\\ now";

fn painted(rows: usize, cols: usize, bytes: &[u8], area: Rect) -> String {
    let term = build(rows, cols, bytes);
    let mut out: Vec<u8> = Vec::new();
    let files = crate::file_links::FileLinks::default();
    emit_hyperlinks(&term, area, &files, &mut out).expect("write");
    String::from_utf8(out).expect("utf8")
}

#[test]
fn a_hyperlink_is_re_emitted_with_its_target() {
    let out = painted(3, 40, LINK, Rect::new(0, 0, 40, 3));
    assert!(
        out.contains("\x1b]8;id="),
        "no OSC 8 open in the pass: {out:?}"
    );
    assert!(
        out.contains("https://example.com/a"),
        "the target is missing: {out:?}"
    );
    assert!(
        out.contains("\x1b]8;;\x1b\\"),
        "the link is never closed: {out:?}"
    );
}

/// The four cells of `HERE` share one link, so they are opened once between
/// them rather than once each.
#[test]
fn a_contiguous_link_is_one_run() {
    let out = painted(3, 40, LINK, Rect::new(0, 0, 40, 3));
    assert_eq!(out.matches("\x1b]8;id=").count(), 1, "{out:?}");
    assert_eq!(out.matches("\x1b]8;;\x1b\\").count(), 1, "{out:?}");
    for ch in ['H', 'E', 'R'] {
        assert!(out.contains(ch), "the run dropped {ch}: {out:?}");
    }
}

#[test]
fn text_with_no_link_paints_nothing() {
    assert_eq!(painted(3, 40, b"just words", Rect::new(0, 0, 40, 3)), "");
}

/// Two links on one row stay two runs, so one target cannot leak onto the
/// other's cells.
#[test]
fn separate_links_stay_separate_runs() {
    let two =
        b"\x1b]8;;https://a.test\x1b\\A\x1b]8;;\x1b\\ \x1b]8;;https://b.test\x1b\\B\x1b]8;;\x1b\\";
    let out = painted(3, 40, two, Rect::new(0, 0, 40, 3));
    assert_eq!(out.matches("\x1b]8;id=").count(), 2, "{out:?}");
    assert!(
        out.contains("https://a.test") && out.contains("https://b.test"),
        "{out:?}"
    );
}

/// The pass writes over a rendered tile, so a cell the tile never showed must
/// not be painted either.
#[test]
fn a_link_outside_the_tile_is_not_painted() {
    let out = painted(3, 40, LINK, Rect::new(0, 0, 2, 3));
    assert_eq!(out, "", "painted outside the tile: {out:?}");
}

/// The run is placed with an absolute cursor move inside the tile, so the
/// tile's own offset has to land in the sequence.
#[test]
fn the_run_is_positioned_inside_the_tile() {
    let out = painted(3, 40, LINK, Rect::new(10, 5, 30, 3));
    assert!(out.contains("\x1b[6;15H"), "wrong placement: {out:?}");
}

fn sel(anchor: (u16, u16), tip: (u16, u16)) -> TileSelection {
    TileSelection { anchor, tip }
}

/// A drag has two directions and one of them is the mirror of the other,
/// so both have to agree on the same ordered range.
#[test]
fn a_selection_normalizes_the_same_dragged_either_way() {
    let forward = sel((0, 2), (3, 5)).normalized();
    let backward = sel((3, 5), (0, 2)).normalized();
    assert_eq!(forward, (0, 2, 3, 5));
    assert_eq!(
        backward, forward,
        "dragging back should give the same range"
    );
}

#[test]
fn a_selection_within_one_row_covers_only_that_span() {
    let s = sel((1, 2), (1, 4));
    assert!(!s.contains(1, 1), "col 1 is before the start");
    assert!(s.contains(1, 2), "the start col is included");
    assert!(s.contains(1, 4), "the end col is included");
    assert!(!s.contains(1, 5), "col 5 is past the end");
    assert!(!s.contains(0, 3), "a different row is not covered");
    assert!(!s.contains(2, 3));
}

#[test]
fn a_multi_row_selection_covers_the_ends_partially_and_the_middle_whole() {
    let s = sel((1, 3), (3, 2));
    assert!(!s.contains(1, 2), "before the anchor on the first row");
    assert!(s.contains(1, 3));
    assert!(s.contains(1, 99), "the first row runs to its end");
    assert!(
        s.contains(2, 0) && s.contains(2, 99),
        "middle rows are whole"
    );
    assert!(s.contains(3, 2));
    assert!(!s.contains(3, 3), "past the tip on the last row");
    assert!(!s.contains(0, 5) && !s.contains(4, 0));
}

/// Indices 0-7 are the normal colours and 8-15 their bright counterparts,
/// so a table off by eight silently swaps every colour in the UI.
#[test]
fn the_low_palette_indices_map_to_their_named_colours() {
    assert_eq!(named_to_ratatui(indexed_low_to_named(0)), RColor::Black);
    assert_eq!(named_to_ratatui(indexed_low_to_named(1)), RColor::Red);
    assert_eq!(named_to_ratatui(indexed_low_to_named(7)), RColor::Gray);
    assert_eq!(named_to_ratatui(indexed_low_to_named(8)), RColor::DarkGray);
    assert_eq!(named_to_ratatui(indexed_low_to_named(9)), RColor::LightRed);
    assert_eq!(named_to_ratatui(indexed_low_to_named(15)), RColor::White);
}

#[test]
fn a_bright_sgr_renders_brighter_than_its_normal_pair() {
    let normal = build(1, 3, b"\x1b[31mX");
    let bright = build(1, 3, b"\x1b[91mX");
    let render = |t: &Term<VoidListener>| {
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        TermWidget::new(t).render(area, &mut buf);
        buf[(0, 0)].fg
    };
    assert_eq!(render(&normal), RColor::Red);
    assert_eq!(render(&bright), RColor::LightRed);
}

#[test]
fn a_true_colour_sgr_survives_as_rgb() {
    let term = build(1, 3, b"\x1b[38;2;18;52;86mX");
    let area = Rect::new(0, 0, 3, 1);
    let mut buf = Buffer::empty(area);
    TermWidget::new(&term).render(area, &mut buf);
    assert_eq!(buf[(0, 0)].fg, RColor::Rgb(0x12, 0x34, 0x56));
}

#[test]
fn rendering_into_a_zero_sized_area_draws_nothing() {
    let term = build(3, 10, b"hello");
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
    TermWidget::new(&term).render(Rect::new(0, 0, 0, 0), &mut buf);
    TermWidget::new(&term).render(Rect::new(0, 0, 10, 0), &mut buf);
    TermWidget::new(&term).render(Rect::new(0, 0, 0, 3), &mut buf);
    assert_eq!(buf[(0, 0)].symbol(), " ", "nothing should have been drawn");
}

#[test]
fn rendering_into_an_area_smaller_than_the_grid_clips() {
    let term = build(5, 20, b"aaaaaaaaaa\r\nbbbbbbbbbb\r\ncccccccccc");
    let area = Rect::new(0, 0, 4, 2);
    let mut buf = Buffer::empty(area);
    TermWidget::new(&term).render(area, &mut buf);
    assert_eq!(buf[(0, 0)].symbol(), "a");
    assert_eq!(buf[(0, 1)].symbol(), "b");
}

#[test]
fn a_selection_highlight_only_covers_the_selected_cells() {
    let term = build(1, 6, b"abcdef");
    let area = Rect::new(0, 0, 6, 1);
    let mut plain = Buffer::empty(area);
    TermWidget::new(&term).render(area, &mut plain);
    let mut marked = Buffer::empty(area);
    TermWidget::new(&term)
        .with_selection(Some(sel((0, 1), (0, 3))))
        .render(area, &mut marked);

    assert_eq!(plain[(0, 0)].bg, marked[(0, 0)].bg, "col 0 is not selected");
    assert_ne!(
        plain[(2, 0)].bg,
        marked[(2, 0)].bg,
        "col 2 is selected and should look different"
    );
    assert_eq!(plain[(5, 0)].bg, marked[(5, 0)].bg, "col 5 is not selected");
}
