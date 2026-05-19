//! `Ctrl+A r` — rename the focused session.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, RenameState};
use crate::keys;

use crate::ui::widgets::open_popup;

pub(in crate::ui) fn draw(f: &mut Frame, area: Rect, state: &RenameState, app: &App) {
    let w = area.width.clamp(30, 60);
    let h: u16 = 7;
    let inner = open_popup(f, area, w, h, " Rename session ", Color::Cyan);

    let id_label = app
        .sessions
        .iter()
        .position(|s| s.id == state.session_id)
        .map(|i| format!("[{}]", i + 1))
        .unwrap_or_default();

    let lines = vec![
        Line::from(Span::styled(
            format!("  Session {}:", id_label),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(format!("  > {}_", state.buf)),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "  {} save  ·  {} cancel",
                keys::RENAME_SAVE.label,
                keys::RENAME_CANCEL.label,
            ),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}
