//! Reusable chip helpers, popup frames, and text utilities. Pure layout
//! primitives — no `App` dependency, so every other ui submodule can pull
//! from here without circular imports.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::theme;

/// Compute the visible `[start, end)` index range for a single-column list
/// given the selected index, total item count, and viewport row count. Keeps
/// the selection on-screen by sliding the window forward when `selected`
/// would otherwise fall past the bottom edge. Centralizes the `selected + 1
/// - visible` arithmetic shared by the picker and spawn dir-list.
pub(super) fn viewport_window(selected: usize, total: usize, height: usize) -> (usize, usize) {
    let start = if selected >= height {
        selected + 1 - height
    } else {
        0
    };
    let end = (start + height).min(total);
    (start, end)
}

/// Highlight a list row with a muted background. Works for single-row and
/// multi-row selections (sidebar uses height=3); callers gate on their own
/// selection predicate. Row text is inset by 2 columns whether or not a row
/// is selected, so highlighting never shifts anything.
pub(super) fn selection_bg(f: &mut Frame, row_area: Rect) {
    f.render_widget(
        Block::default().style(Style::default().bg(theme::BG_ACTIVE)),
        row_area,
    );
}

/// Solid rounded chip with a single label run.
pub(super) fn chip(label: &str, bg: Color) -> Vec<Span<'static>> {
    let fg = Color::Rgb(0x0a, 0x0a, 0x0f);
    vec![
        Span::styled("\u{E0B6}", Style::default().fg(bg)),
        Span::styled(
            label.trim().to_string(),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{E0B4}", Style::default().fg(bg)),
    ]
}

/// Keyboard-chord chip — uses the muted active-row background so the chip
/// reads as "press this" inside hint strings.
pub(super) fn kbd_chip(label: &str) -> Vec<Span<'static>> {
    let bg = theme::BG_ACTIVE;
    let fg = theme::FG;
    vec![
        Span::styled("\u{E0B6}", Style::default().fg(bg)),
        Span::styled(
            label.to_string(),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{E0B4}", Style::default().fg(bg)),
    ]
}

/// Two-cell chip: emphatic key on the left, light label on the right, both
/// painted on the same accent background.
pub(super) fn action_chip(key: &str, label: &str, color: Color) -> Vec<Span<'static>> {
    let dark = Color::Rgb(0x0a, 0x0a, 0x0f);
    vec![
        Span::styled("\u{E0B6}", Style::default().fg(color)),
        Span::styled(
            format!(" {} ", key),
            Style::default()
                .fg(dark)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ", label),
            Style::default()
                .fg(dark)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{E0B4}", Style::default().fg(color)),
    ]
}

pub(super) fn titled_block(title: impl Into<String>, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            title.into(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(color))
}

/// Clamped to `area`: a popup asking for more than the terminal has would
/// otherwise return a rect running past the buffer, and ratatui panics on the
/// first cell outside it.
pub(super) fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let width = w.min(area.width);
    let height = h.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Clear, draw a titled rounded frame, return the inner content rect.
pub(super) fn open_popup(
    f: &mut Frame,
    area: Rect,
    w: u16,
    h: u16,
    title: &str,
    color: Color,
) -> Rect {
    let popup = centered_rect(area, w, h);
    f.render_widget(Clear, popup);
    let block = titled_block(title.to_string(), color);
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    inner
}

/// `$HOME/foo/bar/baz/qux` → `~/foo/…/baz/qux`. Anything ≤ 5 segments is
/// returned unchanged.
pub(super) fn collapse_cwd(p: &str) -> String {
    let home = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned());
    let mut s = match &home {
        Some(h) if p.starts_with(h.as_str()) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    };
    let segs: Vec<&str> = s.split('/').collect();
    if segs.len() > 5 {
        let head = segs[..2].join("/");
        let tail = segs[segs.len() - 2..].join("/");
        s = format!("{}/…/{}", head, tail);
    }
    s
}

/// Truncate to `max` chars with a leading ellipsis (keeps the tail visible —
/// useful for paths where the suffix carries the identifying info).
pub(super) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let n = s.chars().count();
    let keep = max.saturating_sub(1);
    let skip = n - keep;
    let tail: String = s.chars().skip(skip).collect();
    format!("…{}", tail)
}

pub(super) fn pad_right(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (width - n));
    out.push_str(s);
    for _ in 0..(width - n) {
        out.push(' ');
    }
    out
}

/// How long the start of a scrolling name is held still before it slides, so
/// the beginning stays readable on every pass.
pub(super) const MARQUEE_HOLD_MS: u64 = 1_200;

/// How long each column of the slide takes. The event loop beats at least this
/// often while a name is scrolling.
pub const MARQUEE_STEP_MS: u64 = 160;

/// Sits between the end of a scrolling name and the start of it coming round
/// again.
const MARQUEE_GAP: &str = " \u{b7} ";

/// The `width` characters of `name` to show at `now_ms`. A name that fits is
/// returned unchanged. A longer one cycles: held at the start for
/// [`MARQUEE_HOLD_MS`], then one column per [`MARQUEE_STEP_MS`] until it comes
/// back round through the gap.
///
/// Driven by the wall clock rather than a frame counter, so the speed does not
/// follow how fast a session happens to be producing output.
pub(super) fn marquee(name: &str, width: usize, now_ms: u64) -> String {
    let chars: Vec<char> = name.chars().collect();
    if width == 0 || chars.len() <= width {
        return name.to_string();
    }
    let cycle: Vec<char> = chars.into_iter().chain(MARQUEE_GAP.chars()).collect();
    let span = cycle.len();
    let pass_ms = MARQUEE_HOLD_MS + span as u64 * MARQUEE_STEP_MS;
    let into = now_ms % pass_ms;
    let offset = if into < MARQUEE_HOLD_MS {
        0
    } else {
        (((into - MARQUEE_HOLD_MS) / MARQUEE_STEP_MS) as usize) % span
    };
    (0..width).map(|i| cycle[(offset + i) % span]).collect()
}

#[cfg(test)]
#[path = "../tests/widgets.rs"]
mod tests;
