//! Shared `--dangerously-skip-permissions` toggle row used by both the spawn
//! picker and the resume picker. Pulled out because the row visuals (status
//! pill, label, key chip) are identical; the only thing that varies is which
//! key toggles it.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::theme;

use crate::ui::widgets::kbd_chip;

pub(in crate::ui) fn draw_dangerous_panel(
    f: &mut Frame,
    area: Rect,
    active: bool,
    toggle_key: &'static str,
) {
    let (status_color, status_text, label_color, label_mod) = if active {
        (
            theme::ACCENT_RED,
            "● ON",
            theme::ACCENT_RED,
            Modifier::BOLD,
        )
    } else {
        (theme::FG_DIM, "○ OFF", theme::FG, Modifier::empty())
    };

    let panel_bg = if active {
        Color::Rgb(0x33, 0x1a, 0x22)
    } else {
        theme::BG_ACTIVE
    };
    f.render_widget(Block::default().style(Style::default().bg(panel_bg)), area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(12),
            Constraint::Min(20),
            Constraint::Length(20),
        ])
        .split(area);

    let mid_row = area.y + area.height / 2;

    let status_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            status_text.to_string(),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let label_line = Line::from(Span::styled(
        "--dangerously-skip-permissions".to_string(),
        Style::default().fg(label_color).add_modifier(label_mod),
    ))
    .alignment(Alignment::Center);
    let key_line = {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.extend(kbd_chip(toggle_key));
        spans.push(Span::styled(
            " toggles ",
            Style::default().fg(theme::FG_DIM),
        ));
        Line::from(spans).alignment(Alignment::Right)
    };

    f.render_widget(
        Paragraph::new(status_line).style(Style::default().bg(panel_bg)),
        Rect { x: cols[0].x, y: mid_row, width: cols[0].width, height: 1 },
    );
    f.render_widget(
        Paragraph::new(label_line).style(Style::default().bg(panel_bg)),
        Rect { x: cols[1].x, y: mid_row, width: cols[1].width, height: 1 },
    );
    f.render_widget(
        Paragraph::new(key_line).style(Style::default().bg(panel_bg)),
        Rect { x: cols[2].x, y: mid_row, width: cols[2].width, height: 1 },
    );
}
