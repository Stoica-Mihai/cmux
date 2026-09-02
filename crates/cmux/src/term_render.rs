use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color as AlaColor, NamedColor, Rgb};
pub use cmux_term::TermSize;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use cmux_term::{CellColor, is_continuation};
use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::widgets::Widget;

pub struct TermWidget<'a> {
    term: &'a Term<VoidListener>,
    selection: Option<TileSelection>,
    cursor_bg: Option<RColor>,
}

impl<'a> TermWidget<'a> {
    pub fn new(term: &'a Term<VoidListener>) -> Self {
        Self {
            term,
            selection: None,
            cursor_bg: None,
        }
    }

    pub fn with_selection(mut self, sel: Option<TileSelection>) -> Self {
        self.selection = sel;
        self
    }

    pub fn with_cursor_bg(mut self, bg: RColor) -> Self {
        self.cursor_bg = Some(bg);
        self
    }
}

/// Viewport-relative selection between two cells. Coordinates are 0-indexed
/// row, col within the focused tile's inner area. The two endpoints may be in
/// any order; `normalized()` produces (top-left, bottom-right) in linear text
/// order.
#[derive(Debug, Clone, Copy)]
pub struct TileSelection {
    pub anchor: (u16, u16),
    pub tip: (u16, u16),
}

impl TileSelection {
    /// Returns (start_row, start_col, end_row, end_col) ordered for a linear
    /// row-major sweep.
    pub fn normalized(&self) -> (u16, u16, u16, u16) {
        let (ar, ac) = self.anchor;
        let (br, bc) = self.tip;
        if (ar, ac) <= (br, bc) {
            (ar, ac, br, bc)
        } else {
            (br, bc, ar, ac)
        }
    }

    pub fn contains(&self, row: u16, col: u16) -> bool {
        let (sr, sc, er, ec) = self.normalized();
        if row < sr || row > er {
            return false;
        }
        if sr == er {
            col >= sc && col <= ec
        } else if row == sr {
            col >= sc
        } else if row == er {
            col <= ec
        } else {
            true
        }
    }
}

const PALETTE_LEN: usize = 269;

impl<'a> Widget for TermWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let content = self.term.renderable_content();
        let display_offset = content.display_offset as i32;
        let palette = content.colors;
        let mode = content.mode;
        let cursor = content.cursor;

        let mut palette_table: [Option<RColor>; PALETTE_LEN] = [None; PALETTE_LEN];
        for i in 0..PALETTE_LEN {
            palette_table[i] = palette[i].map(rgb_to_ratatui);
        }

        let max_rows = area.height as usize;
        let max_cols = area.width as usize;
        let viewport_top = -display_offset;

        for indexed in content.display_iter {
            let cell = indexed.cell;
            let row = (indexed.point.line.0 - viewport_top) as isize;
            let col = indexed.point.column.0 as isize;
            if row < 0 || col < 0 {
                continue;
            }
            let row = row as usize;
            let col = col as usize;
            if row >= max_rows || col >= max_cols {
                continue;
            }
            if is_continuation(cell) {
                continue;
            }

            let (mut fg, mut bg) = (
                convert_color(cell.fg, &palette_table),
                convert_color(cell.bg, &palette_table),
            );
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let mut mods = Modifier::empty();
            if cell.flags.contains(Flags::BOLD) {
                mods |= Modifier::BOLD;
            }
            if cell.flags.contains(Flags::DIM) {
                mods |= Modifier::DIM;
            }
            if cell.flags.contains(Flags::ITALIC) {
                mods |= Modifier::ITALIC;
            }
            if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                mods |= Modifier::UNDERLINED;
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                mods |= Modifier::CROSSED_OUT;
            }
            if cell.flags.contains(Flags::HIDDEN) {
                fg = bg;
            }

            let in_selection = self
                .selection
                .map(|s| s.contains(row as u16, col as u16))
                .unwrap_or(false);
            if in_selection {
                bg = crate::theme::SELECTION_BG;
                if matches!(fg, RColor::Reset) {
                    fg = crate::theme::FG;
                }
            }

            let style = Style::default().fg(fg).bg(bg).add_modifier(mods);
            let ch = if cell.c == '\0' { ' ' } else { cell.c };
            let x = area.x + col as u16;
            let y = area.y + row as u16;
            if let Some(buf_cell) = buf.cell_mut((x, y)) {
                if let Some(extras) = cell.zerowidth() {
                    let mut sym = String::with_capacity(1 + extras.len());
                    sym.push(ch);
                    for c in extras {
                        sym.push(*c);
                    }
                    buf_cell.set_symbol(&sym);
                } else {
                    buf_cell.set_char(ch);
                }
                buf_cell.set_style(style);
            }
        }

        // Always render the cursor (multiplexer semantics) when the live view is in
        // focus. Claude may emit DECTCEM ?25l, but the operator still wants to see
        // where input lands.
        if display_offset == 0 {
            let row = (cursor.point.line.0 - viewport_top) as isize;
            let col = cursor.point.column.0 as isize;
            if row >= 0 && col >= 0 {
                let row = row as usize;
                let col = col as usize;
                if row < max_rows && col < max_cols {
                    let x = area.x + col as u16;
                    let y = area.y + row as u16;
                    if let Some(buf_cell) = buf.cell_mut((x, y)) {
                        let existing = buf_cell.style();
                        let underlying_fg = existing.fg.unwrap_or(RColor::Reset);
                        let underlying_bg = existing.bg.unwrap_or(RColor::Reset);
                        let fallback_bg = if matches!(underlying_fg, RColor::Reset) {
                            RColor::Gray
                        } else {
                            underlying_fg
                        };
                        let new_bg = self.cursor_bg.unwrap_or(fallback_bg);
                        let new_fg = if matches!(underlying_bg, RColor::Reset) {
                            RColor::Black
                        } else {
                            underlying_bg
                        };
                        let style = Style::default()
                            .fg(new_fg)
                            .bg(new_bg)
                            .add_modifier(existing.add_modifier - Modifier::REVERSED);
                        buf_cell.set_style(style);
                    }
                }
            }
        }
        let _ = mode;
    }
}

fn convert_color(c: AlaColor, table: &[Option<RColor>; PALETTE_LEN]) -> RColor {
    match c {
        AlaColor::Spec(rgb) => rgb_to_ratatui(rgb),
        AlaColor::Named(named) => table[named as usize].unwrap_or_else(|| named_to_ratatui(named)),
        AlaColor::Indexed(i) => {
            let idx = i as usize;
            if let Some(c) = table.get(idx).copied().flatten() {
                c
            } else if i < 16 {
                named_to_ratatui(indexed_low_to_named(i))
            } else {
                RColor::Indexed(i)
            }
        }
    }
}

fn rgb_to_ratatui(rgb: Rgb) -> RColor {
    RColor::Rgb(rgb.r, rgb.g, rgb.b)
}

fn indexed_low_to_named(i: u8) -> NamedColor {
    match i {
        0 => NamedColor::Black,
        1 => NamedColor::Red,
        2 => NamedColor::Green,
        3 => NamedColor::Yellow,
        4 => NamedColor::Blue,
        5 => NamedColor::Magenta,
        6 => NamedColor::Cyan,
        7 => NamedColor::White,
        8 => NamedColor::BrightBlack,
        9 => NamedColor::BrightRed,
        10 => NamedColor::BrightGreen,
        11 => NamedColor::BrightYellow,
        12 => NamedColor::BrightBlue,
        13 => NamedColor::BrightMagenta,
        14 => NamedColor::BrightCyan,
        15 => NamedColor::BrightWhite,
        _ => NamedColor::Foreground,
    }
}

fn named_to_ratatui(n: NamedColor) -> RColor {
    match n {
        NamedColor::Black => RColor::Black,
        NamedColor::Red => RColor::Red,
        NamedColor::Green => RColor::Green,
        NamedColor::Yellow => RColor::Yellow,
        NamedColor::Blue => RColor::Blue,
        NamedColor::Magenta => RColor::Magenta,
        NamedColor::Cyan => RColor::Cyan,
        NamedColor::White => RColor::Gray,
        NamedColor::BrightBlack => RColor::DarkGray,
        NamedColor::BrightRed => RColor::LightRed,
        NamedColor::BrightGreen => RColor::LightGreen,
        NamedColor::BrightYellow => RColor::LightYellow,
        NamedColor::BrightBlue => RColor::LightBlue,
        NamedColor::BrightMagenta => RColor::LightMagenta,
        NamedColor::BrightCyan => RColor::LightCyan,
        NamedColor::BrightWhite => RColor::White,
        NamedColor::DimBlack => RColor::Black,
        NamedColor::DimRed => RColor::Red,
        NamedColor::DimGreen => RColor::Green,
        NamedColor::DimYellow => RColor::Yellow,
        NamedColor::DimBlue => RColor::Blue,
        NamedColor::DimMagenta => RColor::Magenta,
        NamedColor::DimCyan => RColor::Cyan,
        NamedColor::DimWhite => RColor::Gray,
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::DimForeground
        | NamedColor::BrightForeground => RColor::Reset,
    }
}

pub fn visible_text(term: &Term<VoidListener>) -> String {
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

#[cfg(test)]
#[path = "tests/term_render.rs"]
mod tests;

// ---------------------------------------------------------------------------
// OSC 8 hyperlink passthrough
// ---------------------------------------------------------------------------

/// Re-print the hyperlinked cells of a rendered tile, wrapped in OSC 8, so the
/// outer terminal carries the link target and not just the visible text.
///
/// ratatui's `Cell` has no hyperlink attribute, and an escape inside a cell
/// symbol corrupts `Buffer::diff`, which derives how many following cells to
/// skip from `symbol().width()`. So this runs after the frame is drawn, over
/// the cells the frame just painted.
///
/// Cells sharing a hyperlink id and sitting next to each other are emitted as
/// one run, opened and closed within a single write.
pub fn emit_hyperlinks<W: std::io::Write>(
    term: &Term<VoidListener>,
    area: Rect,
    out: &mut W,
) -> std::io::Result<()> {
    let content = term.renderable_content();
    let viewport_top = -(content.display_offset as i32);
    let palette = content.colors;
    let mut palette_table: [Option<RColor>; PALETTE_LEN] = [None; PALETTE_LEN];
    for i in 0..PALETTE_LEN {
        palette_table[i] = palette[i].map(rgb_to_ratatui);
    }

    let mut run: Option<Run> = None;
    for indexed in content.display_iter {
        let cell = indexed.cell;
        let row = indexed.point.line.0 - viewport_top;
        let col = indexed.point.column.0 as i32;
        let inside = row >= 0
            && col >= 0
            && (row as usize) < area.height as usize
            && (col as usize) < area.width as usize;

        let link = if inside && !is_continuation(cell) {
            cell.hyperlink()
        } else {
            None
        };
        let Some(link) = link else {
            if let Some(r) = run.take() {
                r.write(out)?;
            }
            continue;
        };

        let x = area.x + col as u16;
        let y = area.y + row as u16;
        let ch = if cell.c == '\0' { ' ' } else { cell.c };
        let fg = to_cell_color(convert_color(cell.fg, &palette_table));
        let bg = to_cell_color(convert_color(cell.bg, &palette_table));

        match run.as_mut() {
            Some(r) if r.extends(&link, x, y) => r.push(ch, fg, bg, cell.flags),
            _ => {
                if let Some(r) = run.take() {
                    r.write(out)?;
                }
                let mut r = Run::new(&link, x, y);
                r.push(ch, fg, bg, cell.flags);
                run = Some(r);
            }
        }
    }
    if let Some(r) = run.take() {
        r.write(out)?;
    }
    Ok(())
}

/// One horizontal stretch of cells sharing a hyperlink.
struct Run {
    id: String,
    uri: String,
    x: u16,
    y: u16,
    /// Each cell's glyph with the colours and attributes to print it under.
    cells: Vec<(char, CellColor, CellColor, Flags)>,
}

impl Run {
    fn new(link: &alacritty_terminal::term::cell::Hyperlink, x: u16, y: u16) -> Self {
        Self {
            id: link.id().to_string(),
            uri: link.uri().to_string(),
            x,
            y,
            cells: Vec::new(),
        }
    }

    /// The next cell continues this run when it carries the same link and sits
    /// immediately to the right.
    fn extends(&self, link: &alacritty_terminal::term::cell::Hyperlink, x: u16, y: u16) -> bool {
        self.id == link.id()
            && self.uri == link.uri()
            && y == self.y
            && x as usize == self.x as usize + self.cells.len()
    }

    fn push(&mut self, ch: char, fg: CellColor, bg: CellColor, flags: Flags) {
        self.cells.push((ch, fg, bg, flags));
    }

    fn write<W: std::io::Write>(self, out: &mut W) -> std::io::Result<()> {
        write!(out, "\x1b[{};{}H", self.y + 1, self.x + 1)?;
        write!(out, "\x1b]8;id={};{}\x1b\\", self.id, self.uri)?;
        for (ch, fg, bg, flags) in &self.cells {
            out.write_all(cmux_term::sgr(*fg, *bg, *flags).as_bytes())?;
            write!(out, "{}", ch)?;
        }
        out.write_all(b"\x1b]8;;\x1b\\\x1b[0m")?;
        Ok(())
    }
}

/// A palette-resolved ratatui colour, as the shared SGR encoder takes it.
fn to_cell_color(c: RColor) -> CellColor {
    match c {
        RColor::Rgb(r, g, b) => CellColor::Rgb(r, g, b),
        RColor::Indexed(i) => CellColor::Indexed(i),
        _ => CellColor::Default,
    }
}
