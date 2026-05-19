//! `Ctrl+A d` — irreversible-action confirmation. y/Enter detach, n/Esc abort.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::keys;
use crate::theme;

use crate::ui::widgets::{action_chip, open_popup};

pub(in crate::ui) fn draw(f: &mut Frame, area: Rect, id: u64, app: &App) {
    let w = area.width.clamp(44, 56);
    let h: u16 = 6;
    let inner = open_popup(f, area, w, h, " ⚠ Detach session? ", theme::ACCENT_RED);

    let (pos, label) = app
        .sessions
        .iter()
        .enumerate()
        .find(|(_, s)| s.id == id)
        .map(|(i, s)| (i + 1, s.label.clone()))
        .unwrap_or((0, "?".to_string()));

    let lines = vec![
        Line::from(Span::styled(
            format!("Terminate session [{}] '{}' ?", pos, label),
            Style::default().fg(theme::FG),
        ))
        .alignment(Alignment::Center),
        Line::from(Span::styled(
            "Running claude process will be killed.",
            Style::default().fg(theme::FG_DIM),
        ))
        .alignment(Alignment::Center),
        Line::from(""),
        Line::from({
            let mut spans = action_chip(keys::CONFIRM_YES.label, "detach", theme::ACCENT_GREEN);
            spans.push(Span::raw("   "));
            spans.extend(action_chip(keys::CONFIRM_NO.label, "cancel", theme::FG_DIM));
            spans
        })
        .alignment(Alignment::Center),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}
