use super::*;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::Processor;

fn build(rows: usize, cols: usize, bytes: &[u8]) -> Term<VoidListener> {
    let size = TermSize { lines: rows, cols };
    let mut term = Term::new(TermConfig::default(), &size, VoidListener);
    let mut proc: Processor = Processor::new();
    proc.advance(&mut term, bytes);
    term
}

#[test]
fn grid_text_reads_the_screen_row_by_row() {
    let term = build(3, 10, b"hello\r\nworld");
    let text = grid_text(&term);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("hello"));
    assert!(lines[1].starts_with("world"));
}

/// A double-width character occupies two cells but has one glyph, so the
/// spacer must not become a second character in the text.
#[test]
fn a_double_width_character_is_not_doubled() {
    let term = build(1, 10, "aあb".as_bytes());
    assert_eq!(grid_text(&term).trim_end(), "aあb");
}

#[test]
fn a_blank_cell_reads_as_a_space() {
    let term = build(1, 4, b"");
    assert_eq!(grid_text(&term), "    ");
}

#[test]
fn low_colours_use_their_short_codes() {
    assert_eq!(color_sgr(CellColor::Indexed(3), true), "33");
    assert_eq!(color_sgr(CellColor::Indexed(3), false), "43");
    assert_eq!(color_sgr(CellColor::Indexed(9), true), "91");
    assert_eq!(color_sgr(CellColor::Indexed(9), false), "101");
}

#[test]
fn high_colours_and_truecolour_use_the_extended_form() {
    assert_eq!(color_sgr(CellColor::Indexed(208), true), "38;5;208");
    assert_eq!(
        color_sgr(CellColor::Rgb(10, 200, 30), true),
        "38;2;10;200;30"
    );
    assert_eq!(
        color_sgr(CellColor::Rgb(10, 200, 30), false),
        "48;2;10;200;30"
    );
}

#[test]
fn a_default_colour_defers_to_the_terminal() {
    assert_eq!(color_sgr(CellColor::Default, true), "39");
    assert_eq!(color_sgr(CellColor::Default, false), "49");
}

/// Every SGR opens with a reset, so a cell cannot inherit the previous one's
/// attributes.
#[test]
fn every_sgr_resets_before_it_sets() {
    let plain = sgr(CellColor::Default, CellColor::Default, Flags::empty());
    assert_eq!(plain, "\x1b[0;39;49m");

    let loud = sgr(
        CellColor::Indexed(1),
        CellColor::Default,
        Flags::BOLD | Flags::ITALIC | Flags::INVERSE,
    );
    assert!(loud.starts_with("\x1b[0;"), "{loud:?}");
    for code in ["1", "3", "7"] {
        assert!(
            loud.contains(&format!(";{code};")) || loud.contains(&format!(";{code}m")),
            "{code} missing from {loud:?}"
        );
    }
}

#[test]
fn any_underline_style_emits_the_underline_code() {
    let underlined = sgr(CellColor::Default, CellColor::Default, Flags::UNDERLINE);
    assert!(underlined.contains(";4;"), "{underlined:?}");
}

#[test]
fn grid_rows_reports_each_row_and_its_wrap() {
    let t = build(4, 20, b"AAAAAAAAAAAAAAAAAAAABBB\r\nplain\r\n");
    let rows = grid_rows(&t);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].0, "AAAAAAAAAAAAAAAAAAAA");
    assert!(rows[0].1, "the filled row wrapped into the next");
    assert_eq!(rows[1].0.trim_end(), "BBB");
    assert!(!rows[1].1, "the continuation does not itself wrap");
    assert_eq!(rows[2].0.trim_end(), "plain");
    assert!(!rows[2].1);
}

/// Every row is the grid's full width, so a caller can map a column to a cell
/// without guessing.
#[test]
fn grid_rows_are_full_width() {
    let t = build(3, 12, b"hi\r\n");
    for (text, _) in grid_rows(&t) {
        assert_eq!(text.chars().count(), 12);
    }
}
