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

pub(super) fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
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

#[cfg(test)]
#[path = "../tests/widgets.rs"]
mod tests;
