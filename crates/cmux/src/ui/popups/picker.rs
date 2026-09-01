//! `Ctrl+A l` — resume past `~/.claude/projects` sessions with live preview.

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

pub(in crate::ui) fn draw(f: &mut Frame, area: Rect, state: &PickerState) {
    let popup = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    f.render_widget(Clear, popup);
    let block = titled_block(
        format!(" Resume past session ({} found) ", state.items.len()),
        Color::Magenta,
    );
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
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(
                " ↑/↓ select  type to filter  {} clear  {} resume  {} toggle danger  {} cancel",
                keys::PICKER_FILTER_CLEAR.label,
                keys::PICKER_PICK.label,
                keys::PICKER_TOGGLE_DANGER.label,
                keys::PICKER_CANCEL.label,
            ),
            Style::default().fg(Color::DarkGray),
        )),
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
        f.render_widget(
            Paragraph::new(text)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .style(Style::default().fg(Color::Gray)),
            preview_inner,
        );
    }

    draw_rows(f, list_area, state);
}

fn draw_rows(f: &mut Frame, list_area: Rect, state: &PickerState) {
    const NAME_W: usize = 14;
    let has_any_title = state.all.iter().any(|t| t.custom_title.is_some());
    let name_block = if has_any_title { NAME_W + 2 } else { 0 };
    let cwd_w = (list_area.width as usize)
        .saturating_sub(32)
        .saturating_sub(name_block)
        .max(15);

    if state.items.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  (no past sessions found in ~/.claude/projects)",
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
        draw_row(
            f,
            row_rect,
            t,
            i == state.selected,
            has_any_title,
            NAME_W,
            cwd_w,
        );
    }
}

fn draw_row(
    f: &mut Frame,
    row_rect: Rect,
    t: &crate::transcripts::Transcript,
    is_sel: bool,
    has_any_title: bool,
    name_w: usize,
    cwd_w: usize,
) {
    if is_sel {
        selection_bg(f, row_rect);
    }

    let text_rect = Rect {
        x: row_rect.x + 2,
        y: row_rect.y,
        width: row_rect.width.saturating_sub(2),
        height: 1,
    };
    let cwd_str = collapse_cwd(&t.cwd.display().to_string());
    let cwd_cell = pad_right(&truncate(&cwd_str, cwd_w), cwd_w);
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
    if has_any_title {
        let (cell, style) = match &t.custom_title {
            Some(n) => (
                pad_right(&truncate(n, name_w), name_w),
                Style::default()
                    .fg(theme::ACCENT_MAGENTA)
                    .add_modifier(Modifier::ITALIC),
            ),
            None => (" ".repeat(name_w), Style::default().fg(theme::FG_DIM)),
        };
        spans.push(Span::styled(cell, style));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        format!(
            "{}  {:>8}  {:>7}KB  {}",
            cwd_cell,
            crate::transcripts::humanize_age(t.mtime),
            t.file_size / 1024,
            &t.session_id[..8.min(t.session_id.len())],
        ),
        row_style,
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), text_rect);
}
