use super::*;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::Processor;

use crate::session::TermSize;

fn term_of(rows: usize, cols: usize, input: &str) -> Term<VoidListener> {
    let size = TermSize { lines: rows, cols };
    let mut term = Term::new(TermConfig::default(), &size, VoidListener);
    let mut proc: Processor = Processor::new();
    proc.advance(&mut term, input.replace('\n', "\r\n").as_bytes());
    term
}

/// The property that matters: rendering a grid and feeding the result to a
/// fresh terminal reproduces the same grid. Anything the renderer drops or
/// mis-encodes shows up as a differing cell.
fn assert_round_trips(rows: usize, cols: usize, input: &str) {
    let original = term_of(rows, cols, input);
    let bytes = render(&original);

    let size = TermSize { lines: rows, cols };
    let mut replayed = Term::new(TermConfig::default(), &size, VoidListener);
    let mut proc: Processor = Processor::new();
    proc.advance(&mut replayed, &bytes);

    for row in 0..rows {
        let line = Line(row as i32);
        for col in 0..cols {
            let a = &original.grid()[line][Column(col)];
            let b = &replayed.grid()[line][Column(col)];
            assert_eq!(
                (a.c, a.fg, a.bg, a.flags),
                (b.c, b.fg, b.bg, b.flags),
                "cell ({row},{col}) differs after a round trip\n\
                 rendered: {:?}",
                String::from_utf8_lossy(&bytes)
            );
            assert_eq!(
                a.flags.contains(Flags::WRAPLINE),
                b.flags.contains(Flags::WRAPLINE),
                "cell ({row},{col}) lost its wrap after a round trip\n\
                 rendered: {:?}",
                String::from_utf8_lossy(&bytes)
            );
            assert_eq!(
                a.hyperlink().map(|h| h.uri().to_string()),
                b.hyperlink().map(|h| h.uri().to_string()),
                "cell ({row},{col}) lost its hyperlink after a round trip\n\
                 rendered: {:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }
    assert_eq!(
        original.grid().cursor.point,
        replayed.grid().cursor.point,
        "cursor moved during the round trip"
    );
    // Input modes decide whether the program ever receives a wheel report or
    // a bracketed paste, so a client that loses them loses those inputs.
    for (flag, name) in [
        (TermMode::MOUSE_REPORT_CLICK, "1000"),
        (TermMode::MOUSE_DRAG, "1002"),
        (TermMode::MOUSE_MOTION, "1003"),
        (TermMode::SGR_MOUSE, "1006"),
        (TermMode::BRACKETED_PASTE, "2004"),
        (TermMode::APP_CURSOR, "app cursor"),
    ] {
        assert_eq!(
            original.mode().contains(flag),
            replayed.mode().contains(flag),
            "mode {name} did not survive the round trip\nrendered: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }
}

#[test]
fn plain_text_round_trips() {
    assert_round_trips(6, 20, "hello\nworld");
}

#[test]
fn colours_and_attributes_round_trip() {
    assert_round_trips(
        6,
        40,
        "\x1b[31mred\x1b[0m \x1b[1;32mbold green\x1b[0m\n\
         \x1b[44mblue bg\x1b[0m \x1b[3;4mitalic underline\x1b[0m",
    );
}

#[test]
fn indexed_and_truecolour_round_trip() {
    assert_round_trips(
        4,
        40,
        "\x1b[38;5;208morange\x1b[0m \x1b[38;2;10;200;30mrgb\x1b[0m\n\
         \x1b[48;5;27mon indexed\x1b[0m",
    );
}

#[test]
fn a_full_screen_program_round_trips() {
    // Enter the alt screen, paint, and leave the cursor somewhere odd.
    let input = "\x1b[?1049h\x1b[2J\x1b[HTOP\n\x1b[7minverse row\x1b[0m\n\x1b[5;3H";
    assert_round_trips(8, 30, input);
    assert!(is_alt_screen(&term_of(8, 30, input)));
}

#[test]
fn the_alt_screen_is_re_entered_so_later_output_lands_there() {
    let alt = term_of(5, 10, "\x1b[?1049hX");
    assert!(render(&alt).starts_with(b"\x1b[?1049h"));

    let primary = term_of(5, 10, "X");
    assert!(!render(&primary).starts_with(b"\x1b[?1049h"));
}

#[test]
fn a_hidden_cursor_stays_hidden() {
    let hidden = term_of(4, 10, "\x1b[?25lhi");
    assert!(render(&hidden).ends_with(b"\x1b[?25l"));

    let shown = term_of(4, 10, "hi");
    assert!(!render(&shown).ends_with(b"\x1b[?25l"));
}

/// claude prints clickable text as an OSC 8 link. A client is handed the
/// re-rendered grid, so a link that survives only in the raw bytes is a link
/// the client cannot show.
#[test]
fn a_hyperlink_round_trips() {
    assert_round_trips(
        3,
        40,
        "see \x1b]8;;https://example.com/a\x1b\\HERE\x1b]8;;\x1b\\ now",
    );
}

#[test]
fn two_links_on_one_row_keep_their_own_targets() {
    assert_round_trips(
        3,
        40,
        "\x1b]8;;https://a.test\x1b\\A\x1b]8;;\x1b\\ \x1b]8;;https://b.test\x1b\\B\x1b]8;;\x1b\\",
    );
}

/// A program that wrote past the tile's width wrapped; the client must see one
/// logical line, or copying the pair pastes a sentence split at the width.
#[test]
fn a_wrapped_row_round_trips_as_one_line() {
    assert_round_trips(4, 20, &"x".repeat(34));
}

/// And a break the program asked for is still a break.
#[test]
fn a_real_line_break_round_trips() {
    assert_round_trips(4, 20, "short\r\nalso short");
}

/// The wrap can fall on the space between two words. Trimming the row's
/// trailing blank there glued them together in the client's copy.
#[test]
fn a_wrap_on_a_space_keeps_the_word_boundary() {
    let filler = "x".repeat(17);
    assert_round_trips(4, 20, &format!("{filler} second"));
}

/// The modes a full-screen program turns on to read the mouse must reach a
/// client, or its wheel events are encoded in a form the program ignores.
#[test]
fn the_input_modes_round_trip() {
    assert_round_trips(
        4,
        20,
        "\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004hready",
    );
}
