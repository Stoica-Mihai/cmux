use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color as AlaColor, NamedColor, Rgb};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier, Style};
use ratatui::widgets::Widget;

#[derive(Clone, Copy, Debug)]
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

fn is_continuation(cell: &Cell) -> bool {
    cell.flags.contains(Flags::WIDE_CHAR_SPACER)
        || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
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
    pub fn new(row: u16, col: u16) -> Self {
        Self {
            anchor: (row, col),
            tip: (row, col),
        }
    }

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

/// Extract text from the focused tile within the selection range.
/// Walks `display_iter` and emits characters in row-major order, inserting
/// '\n' at row breaks.
pub fn extract_selection(term: &Term<VoidListener>, sel: TileSelection) -> String {
    let (sr, sc, er, ec) = sel.normalized();
    let display_offset = term.grid().display_offset() as i32;
    let viewport_top = -display_offset;
    let mut out = String::new();
    let mut current_row: Option<u16> = None;
    for indexed in term.grid().display_iter() {
        let row_i = indexed.point.line.0 - viewport_top;
        if row_i < 0 {
            continue;
        }
        let row = row_i as u16;
        let col = indexed.point.column.0 as u16;
        if !sel.contains(row, col) {
            continue;
        }
        if let Some(prev) = current_row
            && prev != row
        {
            for _ in prev..row {
                out.push('\n');
            }
        }
        current_row = Some(row);
        if is_continuation(indexed.cell) {
            continue;
        }
        let c = indexed.cell.c;
        out.push(if c == '\0' { ' ' } else { c });
        // also flush any zerowidth combiners attached to this cell
        if let Some(extras) = indexed.cell.zerowidth() {
            for &zc in extras {
                out.push(zc);
            }
        }
        let _ = (sr, sc, er, ec);
    }
    // trim trailing whitespace from each line for cleanliness
    out.lines()
        .map(|l| l.trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n")
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
                buf_cell.set_char(ch);
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
        AlaColor::Named(named) => {
            table[named as usize].unwrap_or_else(|| named_to_ratatui(named))
        }
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
mod tests {
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
        let row0: String = (0..5).map(|x| buf[(x, 0)].symbol().chars().next().unwrap()).collect();
        let row1: String = (0..5).map(|x| buf[(x, 1)].symbol().chars().next().unwrap()).collect();
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
}
