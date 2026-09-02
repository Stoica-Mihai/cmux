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
    emit_hyperlinks(&term, area, &mut out).expect("write");
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
