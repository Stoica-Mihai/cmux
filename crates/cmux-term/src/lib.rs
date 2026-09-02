//! Terminal-grid knowledge shared by the TUI and the daemon.
//!
//! Both binaries parse PTY output into an `alacritty_terminal::Term` and then
//! ask the same questions of the grid: what size is it, which cells hold no
//! glyph of their own, what does the visible screen say in plain text, and how
//! is a cell's appearance written back out as escape sequences. Each of those
//! answers lives here once.

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::{Cell, Flags};

/// Grid dimensions for constructing and resizing a `Term`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub lines: usize,
    pub cols: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// A cell that carries no glyph of its own: the second half of a double-width
/// character, or the padding before one that would not fit.
pub fn is_continuation(cell: &Cell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

/// The visible grid as plain text, one line per row.
pub fn grid_text(term: &Term<VoidListener>) -> String {
    let mut out = String::new();
    let mut current_line: Option<i32> = None;
    for indexed in term.grid().display_iter() {
        let line = indexed.point.line.0;
        if Some(line) != current_line {
            if current_line.is_some() {
                out.push('\n');
            }
            current_line = Some(line);
        }
        if is_continuation(indexed.cell) {
            continue;
        }
        let c = indexed.cell.c;
        out.push(if c == '\0' { ' ' } else { c });
    }
    out
}

/// The visible grid as one entry per row: its text, and whether the terminal
/// wrapped it into the row below. `grid_text` answers the same question for a
/// whole screen at once; this keeps the rows apart, which is what anything
/// stitching screens together needs.
pub fn grid_rows(term: &Term<VoidListener>) -> Vec<(String, bool)> {
    use alacritty_terminal::index::{Column, Line};
    let grid = term.grid();
    let cols = grid.columns();
    let mut rows = Vec::with_capacity(grid.screen_lines());
    for row in 0..grid.screen_lines() {
        let line = Line(row as i32);
        let mut text = String::with_capacity(cols);
        for col in 0..cols {
            let cell = &grid[line][Column(col)];
            if is_continuation(cell) {
                continue;
            }
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
        }
        let wrapped = cols > 0 && grid[line][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        rows.push((text, wrapped));
    }
    rows
}

/// A cell colour resolved far enough to write as SGR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellColor {
    /// The terminal's own default, which is the honest answer for a colour
    /// with no portable code of its own.
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// SGR for a cell's colours and attributes. Resets first and then sets, which
/// is longer than a minimal diff and never wrong about what the previous cell
/// left behind.
pub fn sgr(fg: CellColor, bg: CellColor, flags: Flags) -> String {
    let mut parts: Vec<String> = vec!["0".into()];
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if flags.contains(flag) {
            parts.push(code.into());
        }
    }
    if flags.intersects(Flags::ALL_UNDERLINES) {
        parts.push("4".into());
    }
    parts.push(color_sgr(fg, true));
    parts.push(color_sgr(bg, false));
    format!("\x1b[{}m", parts.join(";"))
}

/// The colour half of an SGR, as a parameter fragment without the escape.
pub fn color_sgr(color: CellColor, foreground: bool) -> String {
    let (basic, bright, extended, default) = if foreground {
        (30, 90, 38, 39)
    } else {
        (40, 100, 48, 49)
    };
    match color {
        CellColor::Rgb(r, g, b) => format!("{extended};2;{r};{g};{b}"),
        CellColor::Indexed(i) if i < 8 => format!("{}", basic + i as u16),
        CellColor::Indexed(i) if i < 16 => format!("{}", bright + (i as u16 - 8)),
        CellColor::Indexed(i) => format!("{extended};5;{i}"),
        CellColor::Default => format!("{default}"),
    }
}

#[cfg(test)]
#[path = "tests/term.rs"]
mod tests;
