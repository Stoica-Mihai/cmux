//! Re-render a session's grid as escape sequences.
//!
//! Attaching replayed the raw byte ring, which reconstructs a shell's history
//! correctly but turns a full-screen program into garbage: the client
//! re-executes thousands of stale drawing commands, out of context. This
//! renders the grid as it stands, so a client is handed the picture instead of
//! the film of how it was painted.
//!
//! Output is ordinary escape sequences rather than a serialized `Term`, so the
//! two ends need not agree on an alacritty version and the browser's xterm.js
//! consumes it unchanged.

use std::fmt::Write as _;

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// A full-screen program is running, which is exactly when replaying the byte
/// ring goes wrong: its output assumes a screen it already painted.
pub(crate) fn is_alt_screen(term: &Term<VoidListener>) -> bool {
    term.mode().contains(TermMode::ALT_SCREEN)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
        }
    }
}

/// Escape sequences that reproduce the visible grid, its pen, and the cursor.
pub(crate) fn render(term: &Term<VoidListener>) -> Vec<u8> {
    let grid = term.grid();
    let rows = grid.screen_lines();
    let cols = grid.columns();

    let mut out = String::with_capacity(rows * cols * 2);
    // Match the screen buffer first, so deltas that follow land in the same
    // place the server is drawing them.
    if is_alt_screen(term) {
        out.push_str("\x1b[?1049h");
    }
    out.push_str("\x1b[H\x1b[2J\x1b[m");

    let mut pen = Pen::default();
    for row in 0..rows {
        if row > 0 {
            out.push_str("\r\n");
        }
        let line = Line(row as i32);
        // Everything past the last written cell is already blank after the
        // clear, so stopping early keeps the payload small.
        let last = (0..cols)
            .rev()
            .find(|&col| {
                let cell = &grid[line][Column(col)];
                cell.c != ' '
                    || cell.bg != Color::Named(NamedColor::Background)
                    || cell.flags.contains(Flags::INVERSE)
            })
            .map(|c| c + 1)
            .unwrap_or(0);

        for col in 0..last {
            let cell = &grid[line][Column(col)];
            // The cell after a double-width char holds no glyph of its own.
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let want = Pen {
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags,
            };
            if want != pen {
                out.push_str(&sgr(&want));
                pen = want;
            }
            out.push(if cell.c == '\0' { ' ' } else { cell.c });
        }
        if pen != Pen::default() {
            out.push_str("\x1b[m");
            pen = Pen::default();
        }
    }

    let Point { line, column } = grid.cursor.point;
    let _ = write!(out, "\x1b[{};{}H", line.0 + 1, column.0 + 1);
    if !term.mode().contains(TermMode::SHOW_CURSOR) {
        out.push_str("\x1b[?25l");
    }
    out.into_bytes()
}

/// Reset first, then set. Longer than a minimal diff and never wrong about
/// what the previous pen left behind.
fn sgr(pen: &Pen) -> String {
    let mut parts: Vec<String> = vec!["0".into()];
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if pen.flags.contains(flag) {
            parts.push(code.into());
        }
    }
    if pen.flags.intersects(Flags::ALL_UNDERLINES) {
        parts.push("4".into());
    }
    parts.push(color_sgr(pen.fg, true));
    parts.push(color_sgr(pen.bg, false));
    format!("\x1b[{}m", parts.join(";"))
}

fn color_sgr(color: Color, foreground: bool) -> String {
    let (basic, bright, extended, default) = if foreground {
        (30, 90, 38, 39)
    } else {
        (40, 100, 48, 49)
    };
    match color {
        Color::Spec(rgb) => format!("{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) if i < 8 => format!("{}", basic + i as u16),
        Color::Indexed(i) if i < 16 => format!("{}", bright + (i as u16 - 8)),
        Color::Indexed(i) => format!("{extended};5;{i}"),
        Color::Named(named) => {
            let n = named as usize;
            if n < 8 {
                format!("{}", basic + n as u16)
            } else if n < 16 {
                format!("{}", bright + (n - 8) as u16)
            } else {
                // Foreground/Background and alacritty's dim variants have no
                // portable code; the terminal's own default is the honest
                // answer rather than a guessed palette entry.
                format!("{default}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
            }
        }
        assert_eq!(
            original.grid().cursor.point,
            replayed.grid().cursor.point,
            "cursor moved during the round trip"
        );
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
}
