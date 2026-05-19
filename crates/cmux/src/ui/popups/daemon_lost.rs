//! Full-screen overlay shown when the daemon connection drops in `--connect`
//! mode. Dims the chrome behind the modal so the frozen session reads as
//! disabled; any key dismisses (handled in main).

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::theme;

use crate::ui::widgets::{centered_rect, kbd_chip};

pub(in crate::ui) fn draw(f: &mut Frame, area: Rect) {
    // Dim the entire screen behind the modal with a wash of BG_ACTIVE so the
    // frozen tile + chrome read as "disabled".
    let dim_style = Style::default()
        .fg(theme::FG_DIM)
        .bg(Color::Rgb(0x11, 0x11, 0x18))
        .add_modifier(Modifier::DIM);
    f.render_widget(Block::default().style(dim_style), area);

    let w = area.width.saturating_sub(8).clamp(52, 72);
    let h: u16 = 11;
    let popup = centered_rect(area, w, h);

    // Drop-shadow effect: a 1-cell-offset darker block behind the popup.
    let shadow = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width,
        height: popup.height,
    };
    let shadow_clip_x = shadow.x.min(area.x + area.width.saturating_sub(1));
    let shadow_clip_w = (area.x + area.width).saturating_sub(shadow_clip_x);
    let shadow_clip = Rect {
        x: shadow_clip_x,
        y: shadow.y,
        width: shadow_clip_w.min(shadow.width),
        height: shadow
            .height
            .min(area.height.saturating_sub(shadow.y - area.y)),
    };
    f.render_widget(Clear, shadow_clip);
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(0x05, 0x05, 0x09))),
        shadow_clip,
    );

    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme::BORDER_DEAD)
                .add_modifier(Modifier::BOLD),
        )
        .title(Span::styled(
            "  Daemon disconnected  ",
            Style::default()
                .fg(theme::BORDER_DEAD)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Rgb(0x18, 0x14, 0x18)));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "cmuxd",
                Style::default()
                    .fg(theme::BORDER_DEAD)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " is no longer responding",
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
            ),
        ])
        .alignment(Alignment::Center),
        Line::from(""),
        Line::from(Span::styled(
            "Sessions are retained on disk.",
            Style::default().fg(theme::FG_MUTED),
        ))
        .alignment(Alignment::Center),
        Line::from(Span::styled(
            "Restart cmuxd and reconnect to resume them.",
            Style::default().fg(theme::FG_MUTED),
        ))
        .alignment(Alignment::Center),
        Line::from(""),
        Line::from({
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.extend(kbd_chip("any key"));
            spans.push(Span::styled(
                "  to dismiss",
                Style::default().fg(theme::FG_DIM),
            ));
            spans
        })
        .alignment(Alignment::Center),
    ];
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(Color::Rgb(0x18, 0x14, 0x18))),
        inner,
    );
}
