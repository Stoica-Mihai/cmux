//! `Ctrl+A l` — open past `~/.claude/projects` sessions with live preview.
//! A green dot marks a conversation claude is running in the background, which
//! `Enter` attaches to; a dimmed dot marks one that is not running, which
//! `Enter` resumes.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::PickerState;
use crate::keys;
use crate::theme;

use crate::ui::popups::dangerous::draw_dangerous_panel;
use crate::ui::widgets::{
    collapse_cwd, pad_right, selection_bg, titled_block, truncate, viewport_window,
};

/// The picker's keys and what each does. The popup and the status bar both
/// list them, so the pairs and their wording live here once.
pub(in crate::ui) fn hint_text() -> String {
    [
        ("↑/↓", "select"),
        ("type", "filter"),
        (keys::PICKER_FILTER_CLEAR.label, "clear"),
        (keys::PICKER_PICK.label, "open"),
        (keys::PICKER_TOGGLE_DANGER.label, "danger"),
        (keys::PICKER_CANCEL.label, "cancel"),
    ]
    .iter()
    .map(|(key, action)| format!("{key}={action}"))
    .collect::<Vec<_>>()
    .join("  ")
}

pub(in crate::ui) fn draw(f: &mut Frame, area: Rect, state: &PickerState) {
    let popup = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    f.render_widget(Clear, popup);
    let count = if state.scanning {
        "scanning...".to_string()
    } else {
        format!("{} found", state.items.len())
    };
    let block = titled_block(format!(" Resume past session ({count}) "), Color::Magenta);
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // filter
            Constraint::Min(3),    // list + preview
            Constraint::Length(1), // gap
            Constraint::Length(3), // dangerous panel
            Constraint::Length(1), // hint
        ])
        .split(inner);

    draw_filter_line(f, vertical[0], state);
    draw_list_and_preview(f, vertical[1], state);
    // vertical[2] is an intentionally empty 1-row gap for breathing space.
    draw_dangerous_panel(
        f,
        vertical[3],
        state.dangerous,
        keys::PICKER_TOGGLE_DANGER.label,
    );
    let hint = Style::default().fg(Color::DarkGray);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {}  ", hint_text()), hint),
            Span::styled(
                theme::glyph::CONNECTION,
                Style::default().fg(theme::ACCENT_GREEN),
            ),
            Span::styled("=running", hint),
        ])),
        vertical[4],
    );
}

fn draw_filter_line(f: &mut Frame, area: Rect, state: &PickerState) {
    let line = if state.filter.is_empty() {
        Line::from(vec![
            Span::styled(" filter: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "(type to search by cwd or --name)",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" filter: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                state.filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("    ({}/{})", state.items.len(), state.all.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_list_and_preview(f: &mut Frame, area: Rect, state: &PickerState) {
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    let list_area = horiz[0];
    let preview_area = horiz[1];

    let preview_block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" preview ");
    let preview_inner = preview_block.inner(preview_area);
    f.render_widget(preview_block, preview_area);

    if let Some(t) = state.current() {
        let text = state
            .previews
            .get(&t.session_id)
            .cloned()
            .unwrap_or_else(|| "(loading...)".to_string());
        let mut lines: Vec<Line<'static>> = Vec::new();
        if let Some(origin) = state.forked_from(t) {
            lines.push(Line::from(Span::styled(
                format!("forked from {origin}"),
                Style::default()
                    .fg(theme::ACCENT_CYAN)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::default());
        }
        lines.extend(text.lines().map(|l| Line::from(l.to_string())));
        f.render_widget(
            Paragraph::new(lines)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .style(Style::default().fg(Color::Gray)),
            preview_inner,
        );
    }

    draw_rows(f, list_area, state);
}

/// Age column, as `humanize_age` fills it.
const AGE_W: usize = 4;
/// Size column, as `format_size_bytes` fills it.
const SIZE_W: usize = 7;
/// Short-id column, as `short_id` fills it.
const ID_W: usize = 8;
/// Fork column, wide enough for its one word.
const FORK_W: usize = 4;
/// The row's leading pad, its connection dot and the separator after it, plus
/// the age, size and id columns with the two spaces separating each.
const FIXED_W: usize = 1 + 1 + 1 + 2 + AGE_W + 2 + SIZE_W + 2 + ID_W;

/// Column widths shared by every row of one pass over the list.
struct RowLayout {
    name_w: usize,
    fork_w: usize,
    cwd_w: usize,
}

fn draw_rows(f: &mut Frame, list_area: Rect, state: &PickerState) {
    const NAME_MIN: usize = 12;
    const NAME_MAX: usize = 28;
    let widest_name = state
        .all
        .iter()
        .filter_map(|t| state.display_name(t))
        .map(|n| n.chars().count())
        .max()
        .unwrap_or(0);
    let name_w = widest_name.clamp(NAME_MIN, NAME_MAX);
    let name_block = if widest_name == 0 { 0 } else { name_w + 2 };
    let any_fork = state.all.iter().any(|t| state.fork_origin(t).is_some());
    let fork_block = if any_fork { FORK_W + 2 } else { 0 };
    let cwd_w = (list_area.width as usize)
        .saturating_sub(FIXED_W)
        .saturating_sub(name_block)
        .saturating_sub(fork_block)
        .max(15);
    let layout = RowLayout {
        name_w: name_block.saturating_sub(2),
        fork_w: fork_block.saturating_sub(2),
        cwd_w,
    };

    if state.items.is_empty() {
        let msg = if state.scanning {
            "  scanning ~/.claude/projects..."
        } else {
            "  (no past sessions found in ~/.claude/projects)"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(Color::DarkGray),
            ))),
            list_area,
        );
        return;
    }

    let (start, end) =
        viewport_window(state.selected, state.items.len(), list_area.height as usize);

    let mut row_y = list_area.y;
    for i in start..end {
        if row_y >= list_area.y + list_area.height {
            break;
        }
        let Some(t) = state.items.get(i).and_then(|idx| state.all.get(*idx)) else {
            continue;
        };
        let row_rect = Rect {
            x: list_area.x,
            y: row_y,
            width: list_area.width,
            height: 1,
        };
        row_y += 1;
        draw_row(f, row_rect, state, t, i == state.selected, &layout);
    }
}

fn draw_row(
    f: &mut Frame,
    row_rect: Rect,
    state: &PickerState,
    t: &crate::transcripts::Transcript,
    is_sel: bool,
    layout: &RowLayout,
) {
    if is_sel {
        selection_bg(f, row_rect);
    }

    let text_rect = Rect {
        x: row_rect.x + 1,
        y: row_rect.y,
        width: row_rect.width.saturating_sub(1),
        height: 1,
    };
    let cwd_str = collapse_cwd(&t.cwd.display().to_string());
    let cwd_cell = pad_right(&truncate(&cwd_str, layout.cwd_w), layout.cwd_w);
    let fg = if is_sel {
        theme::BORDER_FOCUS
    } else {
        theme::FG
    };
    let row_style = if is_sel {
        Style::default().fg(fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(fg)
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    let connected = state.running_as(t).is_some();
    spans.push(Span::styled(
        theme::glyph::CONNECTION,
        Style::default().fg(if connected {
            theme::ACCENT_GREEN
        } else {
            theme::FG_DIM
        }),
    ));
    spans.push(Span::raw(" "));
    if layout.name_w > 0 {
        let (cell, style) = match state.display_name(t) {
            Some(n) => (
                pad_right(&truncate(n, layout.name_w), layout.name_w),
                Style::default()
                    .fg(theme::ACCENT_MAGENTA)
                    .add_modifier(Modifier::ITALIC),
            ),
            None => (
                " ".repeat(layout.name_w),
                Style::default().fg(theme::FG_DIM),
            ),
        };
        spans.push(Span::styled(cell, style));
        spans.push(Span::raw("  "));
    }
    if layout.fork_w > 0 {
        let cell = if state.fork_origin(t).is_some() {
            "fork"
        } else {
            ""
        };
        spans.push(Span::styled(
            pad_right(cell, layout.fork_w),
            Style::default().fg(theme::ACCENT_CYAN),
        ));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        format!(
            "{}  {:>age_w$}  {:>size_w$}  {}",
            cwd_cell,
            crate::transcripts::humanize_age(t.mtime),
            crate::util::format_size_bytes(t.file_size),
            crate::transcripts::short_id(&t.session_id),
            age_w = AGE_W,
            size_w = SIZE_W,
        ),
        row_style,
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), text_rect);
}

#[cfg(test)]
#[path = "../../tests/picker.rs"]
mod tests;
