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

/// The DEC private modes that decide how input reaches the program, as set
/// sequences. Anything not set is left alone: a client starts with them off.
fn input_modes(term: &Term<VoidListener>) -> String {
    let mode = term.mode();
    let mut out = String::new();
    for (flag, code) in [
        (TermMode::MOUSE_REPORT_CLICK, "1000"),
        (TermMode::MOUSE_DRAG, "1002"),
        (TermMode::MOUSE_MOTION, "1003"),
        (TermMode::SGR_MOUSE, "1006"),
        (TermMode::UTF8_MOUSE, "1005"),
        (TermMode::ALTERNATE_SCROLL, "1007"),
        (TermMode::BRACKETED_PASTE, "2004"),
        (TermMode::FOCUS_IN_OUT, "1004"),
        (TermMode::APP_CURSOR, "1"),
    ] {
        if mode.contains(flag) {
            let _ = write!(out, "\x1b[?{code}h");
        }
    }
    out
}

/// Whether a row ends in a wrap into the row below.
fn wraps(
    grid: &alacritty_terminal::Grid<alacritty_terminal::term::cell::Cell>,
    row: usize,
) -> bool {
    let cols = grid.columns();
    cols > 0
        && grid[Line(row as i32)][Column(cols - 1)]
            .flags
            .contains(Flags::WRAPLINE)
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
    // Input modes the program turned on. A client that attaches later has to
    // know them, or it cannot encode a mouse report the program will accept
    // and its wheel and paste go nowhere.
    out.push_str(&input_modes(term));
    out.push_str("\x1b[H\x1b[2J\x1b[m");

    let mut pen = Pen::default();
    let mut link: Option<String> = None;
    for row in 0..rows {
        // A row the program wrapped carries no break of its own: writing one
        // makes the client's grid hold two logical lines, and a copy of the
        // pair then pastes a sentence split at the tile's width.
        if row > 0 && !wraps(grid, row - 1) {
            out.push_str("\r\n");
        }
        let line = Line(row as i32);
        // A wrapped row is written at full width: the client's own wrap has to
        // land on the same column, and its last cell may be the space between
        // two words. Stopping early there glued them together.
        let last = if wraps(grid, row) {
            cols
        } else {
            // Everything past the last written cell is already blank after the
            // clear, so stopping early keeps the payload small.
            (0..cols)
                .rev()
                .find(|&col| {
                    let cell = &grid[line][Column(col)];
                    cell.c != ' '
                        || cell.bg != Color::Named(NamedColor::Background)
                        || cell.flags.contains(Flags::INVERSE)
                })
                .map(|c| c + 1)
                .unwrap_or(0)
        };

        for col in 0..last {
            let cell = &grid[line][Column(col)];
            // The link opens on its first cell and closes on the first cell
            // past it, so a run reaches the client as one link.
            let want_link = cell
                .hyperlink()
                .map(|h| (h.id().to_string(), h.uri().to_string()));
            let want_key = want_link
                .as_ref()
                .map(|(id, uri)| format!("{id}\u{1}{uri}"));
            if want_key != link {
                if link.is_some() {
                    out.push_str("\x1b]8;;\x1b\\");
                }
                if let Some((id, uri)) = &want_link {
                    let _ = write!(out, "\x1b]8;id={id};{uri}\x1b\\");
                }
                link = want_key;
            }
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
                out.push_str(&cmux_term::sgr(
                    to_cell_color(want.fg),
                    to_cell_color(want.bg),
                    want.flags,
                ));
                pen = want;
            }
            out.push(if cell.c == '\0' { ' ' } else { cell.c });
        }
        if link.is_some() {
            out.push_str("\x1b]8;;\x1b\\");
            link = None;
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

#[cfg(test)]
#[path = "tests/snapshot.rs"]
mod tests;

/// An alacritty colour resolved as far as the shared SGR encoder needs it.
/// `Foreground`/`Background` and alacritty's dim variants have no portable
/// code, so the terminal's own default is the honest answer.
fn to_cell_color(color: Color) -> cmux_term::CellColor {
    match color {
        Color::Spec(rgb) => cmux_term::CellColor::Rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => cmux_term::CellColor::Indexed(i),
        Color::Named(named) => {
            let n = named as usize;
            if n < 16 {
                cmux_term::CellColor::Indexed(n as u8)
            } else {
                cmux_term::CellColor::Default
            }
        }
    }
}
