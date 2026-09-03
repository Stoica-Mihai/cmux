//! Output collected across scrolls, so a selection can span more than one
//! screen.
//!
//! A full-screen program repaints its viewport in place and keeps no grid
//! history, so cmux holds exactly one screen of it. Dragging a selection past
//! the tile's edge sends the program a scroll, it repaints one step further,
//! and the rows that were revealed are stitched onto this buffer. The
//! selection then addresses buffer rows rather than viewport rows, and
//! survives the view moving underneath it.
//!
//! Consecutive screens overlap heavily, so how far the view moved is derived
//! from that overlap rather than assumed. A program free to scroll 2 rows one
//! time and 3 the next stays handled, and so does one that changes its step.

/// How much of the text one press selects: a cell, the word under it, or the
/// whole line. A terminal picks this from the number of clicks, and it holds
/// for the drag that follows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Granularity {
    #[default]
    Char,
    Word,
    Line,
}

/// Characters a word-granularity selection keeps together. Beyond
/// alphanumerics this is what a path, an identifier or a version is made of,
/// so one press takes the whole thing rather than stopping at every dot.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@' | '~' | '+')
}

/// Whether a row is one character drawn across the tile - a separator rule
/// rather than text.
fn is_rule(text: &str) -> bool {
    let mut chars = text.trim().chars();
    let Some(first) = chars.next() else {
        return false;
    };
    !first.is_alphanumeric() && chars.clone().count() > 0 && chars.all(|c| c == first)
}

/// Which run a character belongs to. Runs of the same class select together,
/// so a word, a stretch of spaces and a run of punctuation are each one unit.
fn class(c: char) -> u8 {
    if is_word(c) {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

/// How one row carries into the next when a selection spans both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Join {
    /// A line of its own: the rows are separated by a newline.
    Break,
    /// The terminal ran out of columns mid-text. The rows are one line and
    /// concatenate exactly, because the break can fall inside a word.
    Wrap,
    /// The program broke its own line to fit the width. The rows are one line
    /// with a single space between them, because the break fell between
    /// words, and the indent the program adds to the continuation goes.
    Reflow,
}

/// One captured row: its text, and whether the terminal wrapped it into the
/// row below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub wrapped: bool,
}

impl Row {
    pub fn new(text: impl Into<String>, wrapped: bool) -> Self {
        Self {
            text: text.into(),
            wrapped,
        }
    }
}

/// Rows near the top and bottom of a full-screen program's viewport that
/// change on their own — a spinner, a clock, a token counter — and so cannot
/// be matched between two screens.
const LIVE_EDGE_ROWS: usize = 3;

/// Rows that must agree before two screens are called the same content moved.
const MIN_AGREEMENT: usize = 3;

/// How far `new` sits from `old`, in rows, or `None` when they do not overlap
/// enough to tell. A positive value means the content moved *down* the screen,
/// which is what revealing earlier rows at the top looks like.
///
/// Rows within `LIVE_EDGE_ROWS` of either edge are ignored, and a blank row
/// never counts as agreement: a screen with a blank band would otherwise match
/// at any offset.
#[cfg(test)]
pub fn displacement(old: &[Row], new: &[Row]) -> Option<isize> {
    best_shift(old, new).map(|(shift, _)| shift)
}

/// Which rows of `new` the held rows landed on at a given shift. These bound
/// the scrolling region: a program's fixed chrome does not agree at the shift
/// its content moved by, so it falls outside the band.
fn matched_band(old: &[Row], new: &[Row], shift: isize) -> Option<(usize, usize)> {
    let mut band: Option<(usize, usize)> = None;
    for (i, held) in old.iter().enumerate() {
        let j = i as isize + shift;
        if j < 0 || j as usize >= new.len() {
            continue;
        }
        let j = j as usize;
        if held.text.trim().is_empty() || held.text != new[j].text {
            continue;
        }
        band = Some(match band {
            None => (j, j),
            Some((lo, hi)) => (lo.min(j), hi.max(j)),
        });
    }
    band
}

/// The first row that stayed at the same screen position across two captures
/// while the content moved past it, and so belongs to the program's fixed
/// chrome rather than to its scrolling output. `None` when the whole screen
/// scrolled.
///
/// Reported in screen coordinates, and only looked for below the band: a
/// header above the content does not get in the way of stitching, while a
/// footer left in the buffer sits between two screens of output.
fn first_chrome_row(old: &[Row], new: &[Row], shift: isize) -> Option<usize> {
    let band = matched_band(old, new, shift)?;
    let content_end = (band.1 as isize - shift).max(0) as usize;
    (content_end + 1..old.len().min(new.len())).find(|&i| {
        // Identical at the same index while everything around it moved.
        old[i].text == new[i].text
    })
}

/// The shift with the most agreement, and how many rows agreed.
fn best_shift(old: &[Row], new: &[Row]) -> Option<(isize, usize)> {
    let rows = old.len().min(new.len());
    if rows <= LIVE_EDGE_ROWS * 2 {
        return None;
    }
    let span = rows as isize;
    let mut best: Option<(usize, isize)> = None;
    for shift in -(span - 1)..span {
        let mut agree = 0usize;
        for i in 0..old.len() {
            let j = i as isize + shift;
            if j < 0 || j as usize >= new.len() {
                continue;
            }
            if i < LIVE_EDGE_ROWS || i + LIVE_EDGE_ROWS >= old.len() {
                continue;
            }
            let a = &old[i];
            let b = &new[j as usize];
            if a.text.trim().is_empty() {
                continue;
            }
            if a.text == b.text {
                agree += 1;
            }
        }
        if agree >= MIN_AGREEMENT {
            // A tie goes to the smaller move: a screen of repeated rows
            // matches at several offsets, and the nearest is the honest read.
            let better = match best {
                None => true,
                Some((b_agree, b_shift)) => {
                    agree > b_agree || (agree == b_agree && shift.abs() < b_shift.abs())
                }
            };
            if better {
                best = Some((agree, shift));
            }
        }
    }
    best.map(|(agree, shift)| (shift, agree))
}

/// Rows collected from one session, oldest first, with the viewport's position
/// in them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyBuffer {
    rows: Vec<Row>,
    /// Index of the row drawn at the tile's first row.
    top: usize,
    /// Tile size the rows were captured at. A resize rewraps everything, so
    /// the rows stop corresponding to what is on screen.
    size: (u16, u16),
    /// Whether the chrome the first capture swept up has been dropped. The
    /// scrolling region is only knowable once something has moved.
    trimmed: bool,
}

impl CopyBuffer {
    /// Start from what is on screen now.
    pub fn new(screen: Vec<Row>, size: (u16, u16)) -> Self {
        Self {
            rows: screen,
            top: 0,
            size,
            trimmed: false,
        }
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Buffer index of the row drawn at the tile's first row.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The buffer row a viewport row is showing, if any.
    pub fn line_at(&self, viewport_row: u16) -> Option<usize> {
        let line = self.top + viewport_row as usize;
        (line < self.rows.len()).then_some(line)
    }

    /// The viewport row a buffer line is drawn at, if it is on screen.
    pub fn viewport_row(&self, line: usize, height: u16) -> Option<u16> {
        let rel = line.checked_sub(self.top)?;
        (rel < height as usize).then_some(rel as u16)
    }

    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Merge a freshly captured screen. Rows revealed above or below what is
    /// held get added, and the viewport position follows the content.
    ///
    /// Reports how many rows went on the front, because every line index the
    /// caller holds shifts by that much. `None` when the screens could not be
    /// matched, which leaves the buffer untouched: a caller that cannot tell
    /// how far the view moved must not guess, or the selection ends up
    /// covering text the user never dragged over.
    pub fn stitch(&mut self, screen: Vec<Row>) -> Option<usize> {
        if screen.is_empty() {
            return None;
        }
        let height = screen.len();
        let visible_end = (self.top + height).min(self.rows.len());
        let visible = &self.rows[self.top.min(self.rows.len())..visible_end];

        let (shift, _) = best_shift(visible, &screen)?;
        let (band_lo, band_hi) = matched_band(visible, &screen, shift)?;
        if shift == 0 {
            // Nothing moved, so nothing was revealed.
            return Some(0);
        }
        let step = shift.unsigned_abs();

        // The first move is when the scrolling region becomes knowable, and
        // whatever the initial capture swept up below it has to go: a band of
        // fixed chrome in the middle of the output is text the user never
        // dragged over, and nothing later can be matched against it.
        if !self.trimmed {
            if let Some(chrome) = first_chrome_row(visible, &screen, shift) {
                let content_end = self.top + chrome;
                if content_end < self.rows.len() {
                    self.rows.truncate(content_end);
                }
            }
            self.trimmed = true;
        }

        // Buffer line each screen row now stands at.
        let new_top = self.top as isize - shift;
        // The rows just past the matched band, in the direction the content
        // moved. Reading from the screen's edge instead would pick up chrome.
        let (from, to) = if shift > 0 {
            (band_lo.saturating_sub(step), band_lo)
        } else {
            ((band_hi + 1).min(height), (band_hi + 1 + step).min(height))
        };

        // Of those, keep only the ones that fall outside what is already
        // held: scrolling back and then forward revisits rows, and adding
        // them twice would repeat them in the copy.
        let mut prepend: Vec<Row> = Vec::new();
        let mut append: Vec<Row> = Vec::new();
        for (offset, row) in screen[from.min(to)..to].iter().enumerate() {
            let line = new_top + (from + offset) as isize;
            if line < 0 {
                prepend.push(row.clone());
            } else if line as usize >= self.rows.len() {
                append.push(row.clone());
            }
        }
        let grew_up = prepend.len();
        if grew_up > 0 {
            self.rows.splice(0..0, prepend);
        }
        self.rows.extend(append);
        self.top = (new_top + grew_up as isize).max(0) as usize;
        Some(grew_up)
    }

    /// Capture what a terminal is showing now.
    pub fn capture(
        term: &alacritty_terminal::Term<alacritty_terminal::event::VoidListener>,
        size: (u16, u16),
    ) -> Self {
        Self::new(rows_of(term), size)
    }

    /// Merge what a terminal is showing now.
    pub fn stitch_term(
        &mut self,
        term: &alacritty_terminal::Term<alacritty_terminal::event::VoidListener>,
    ) -> Option<usize> {
        self.stitch(rows_of(term))
    }

    /// Whether the next row continues this one, and how.
    ///
    /// The terminal's own wrap flag is authoritative when it is set. It is not
    /// set for a program that wraps its own text - claude emits each visual
    /// line separately, so the terminal never runs out of columns - and for
    /// those the wrap has to be recognised from the shape of the two rows.
    ///
    /// The test is the wrapping decision itself: if the next row's first word
    /// could not have fitted in the columns this row left free, then this row
    /// was broken to make it fit rather than ended on purpose. A line that
    /// stops with room to spare for the word that follows was a line the
    /// program chose to end.
    fn join_after(&self, line: usize) -> Join {
        let Some(row) = self.rows.get(line) else {
            return Join::Break;
        };
        if row.wrapped {
            return Join::Wrap;
        }
        let Some(next) = self.rows.get(line + 1) else {
            return Join::Break;
        };
        let trimmed = row.text.trim_end_matches(' ');
        let filled = trimmed.chars().count();
        // A blank row on either side ends the line: that is a paragraph break.
        let cols = self.size.1 as usize;
        if filled == 0 || next.text.trim().is_empty() || cols == 0 {
            return Join::Break;
        }
        // A rule drawn across the tile is decoration, not prose. On this row it
        // leaves no free columns and would swallow what follows; on the next
        // it is wide enough that no line could have held it, and would be
        // pulled up into the line above.
        if is_rule(trimmed) || is_rule(next.text.trim_end_matches(' ')) {
            return Join::Break;
        }
        let first_word = next
            .text
            .trim_start()
            .split(' ')
            .next()
            .map(|w| w.chars().count())
            .unwrap_or(0);
        let free = cols.saturating_sub(filled);
        if first_word + 1 > free {
            Join::Reflow
        } else {
            Join::Break
        }
    }

    /// Columns of the run at `col` on `line`, inclusive. A press on a space
    /// takes the stretch of spaces and one on punctuation takes that run, as a
    /// terminal does.
    pub fn word_bounds(&self, line: usize, col: u16) -> (u16, u16) {
        let Some(row) = self.rows.get(line) else {
            return (col, col);
        };
        let chars: Vec<char> = row.text.chars().collect();
        if chars.is_empty() {
            return (col, col);
        }
        let at = (col as usize).min(chars.len() - 1);
        let want = class(chars[at]);
        let mut lo = at;
        while lo > 0 && class(chars[lo - 1]) == want {
            lo -= 1;
        }
        let mut hi = at;
        while hi + 1 < chars.len() && class(chars[hi + 1]) == want {
            hi += 1;
        }
        (lo as u16, hi as u16)
    }

    /// First and last cell of the logical line `line` sits in, following the
    /// wraps the terminal made. A line the terminal broke over three rows is
    /// one unit, because that is what it was before the width forced it.
    pub fn line_bounds(&self, line: usize) -> ((usize, u16), (usize, u16)) {
        let last = self.rows.len().saturating_sub(1);
        let line = line.min(last);
        let mut start = line;
        while start > 0 && self.rows[start - 1].wrapped {
            start -= 1;
        }
        let mut end = line;
        while end < last && self.rows[end].wrapped {
            end += 1;
        }
        let end_col = self.rows[end]
            .text
            .chars()
            .count()
            .saturating_sub(1)
            .try_into()
            .unwrap_or(u16::MAX);
        ((start, 0), (end, end_col))
    }

    /// The two endpoints a selection covers at `gran`, ordered. The first is
    /// pulled back to the start of its unit and the second pushed out to the
    /// end of its own, so a drag grows a unit at a time rather than a cell at
    /// a time.
    pub fn snap(
        &self,
        a: (usize, u16),
        b: (usize, u16),
        gran: Granularity,
    ) -> ((usize, u16), (usize, u16)) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        match gran {
            Granularity::Char => (lo, hi),
            Granularity::Word => {
                let (start, _) = self.word_bounds(lo.0, lo.1);
                let (_, end) = self.word_bounds(hi.0, hi.1);
                ((lo.0, start), (hi.0, end))
            }
            Granularity::Line => {
                let (start, _) = self.line_bounds(lo.0);
                let (_, end) = self.line_bounds(hi.0);
                (start, end)
            }
        }
    }

    /// The text between two points, as a flowing selection: the first line
    /// from its column, the last to its column, the lines between whole.
    pub fn text_range(&self, from: (usize, u16), to: (usize, u16)) -> String {
        let (start, end) = if from <= to { (from, to) } else { (to, from) };
        let last = end.0.min(self.rows.len().saturating_sub(1));
        let mut out = String::new();
        let mut reflowed_into = false;
        for line in start.0.min(last)..=last {
            let row = &self.rows[line];
            let chars: Vec<char> = row.text.chars().collect();
            let lo = if line == start.0 {
                (start.1 as usize).min(chars.len())
            } else {
                0
            };
            let hi = if line == end.0 {
                (end.1 as usize + 1).min(chars.len())
            } else {
                chars.len()
            };
            let piece: String = chars[lo.min(hi)..hi].iter().collect();
            // Carrying on from a reflowed row: the indent the program puts on
            // a continuation is layout, not text.
            let piece = if reflowed_into {
                piece.trim_start_matches(' ')
            } else {
                piece.as_str()
            };
            let join = if line < last {
                self.join_after(line)
            } else {
                Join::Break
            };
            reflowed_into = join == Join::Reflow;
            match join {
                Join::Wrap => out.push_str(piece),
                Join::Reflow => {
                    out.push_str(piece.trim_end_matches(' '));
                    out.push(' ');
                }
                Join::Break => {
                    out.push_str(piece.trim_end_matches(' '));
                    if line < last {
                        out.push('\n');
                    }
                }
            }
        }
        out
    }
}

/// A terminal's visible rows, as this buffer holds them.
fn rows_of(term: &alacritty_terminal::Term<alacritty_terminal::event::VoidListener>) -> Vec<Row> {
    cmux_term::grid_rows(term)
        .into_iter()
        .map(|(text, wrapped)| Row::new(text, wrapped))
        .collect()
}

#[cfg(test)]
#[path = "tests/copy_buffer.rs"]
mod tests;
