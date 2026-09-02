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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::popups::harness::{
        assert_inside, assert_legible, painted_bounds, render, text, try_render,
    };
    use ratatui::buffer::Buffer;

    const MODAL_BG: Color = Color::Rgb(0x18, 0x14, 0x18);
    const SHADOW_BG: Color = Color::Rgb(0x05, 0x05, 0x09);
    const WASH_BG: Color = Color::Rgb(0x11, 0x11, 0x18);

    fn cells_with_bg(buf: &Buffer, bg: Color) -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if buf[(x, y)].bg == bg {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn it_names_what_died_and_what_survives() {
        let buf = render(80, 24, |f| draw(f, Rect::new(0, 0, 80, 24)));
        let out = text(&buf);

        for needle in [
            "Daemon disconnected",
            "cmuxd",
            "is no longer responding",
            "Sessions are retained on disk.",
            "Restart cmuxd and reconnect to resume them.",
            "any key",
            "to dismiss",
        ] {
            assert!(out.contains(needle), "the modal lacks {needle:?}:\n{out}");
        }
        assert_legible(&buf, "daemon_lost");
    }

    #[test]
    fn the_screen_behind_the_modal_is_washed_out() {
        let buf = render(80, 24, |f| draw(f, Rect::new(0, 0, 80, 24)));
        let corner = &buf[(0u16, 0u16)];

        assert_eq!(
            corner.bg, WASH_BG,
            "the chrome behind the modal is not dimmed, so the frozen session reads as live"
        );
        assert!(
            corner.modifier.contains(Modifier::DIM),
            "the wash carries no DIM modifier: {:?}",
            corner.modifier
        );
    }

    #[test]
    fn the_shadow_sits_one_cell_below_and_right_of_the_modal() {
        let buf = render(80, 24, |f| draw(f, Rect::new(0, 0, 80, 24)));
        let modal = cells_with_bg(&buf, MODAL_BG);
        let shadow = cells_with_bg(&buf, SHADOW_BG);

        assert!(!modal.is_empty(), "the modal never drew");
        assert!(!shadow.is_empty(), "the drop shadow never drew");

        let right = |cells: &[(u16, u16)]| cells.iter().map(|(x, _)| *x).max().unwrap();
        let bottom = |cells: &[(u16, u16)]| cells.iter().map(|(_, y)| *y).max().unwrap();

        assert_eq!(
            right(&shadow),
            right(&modal) + 1,
            "the shadow is not offset one column right of the modal"
        );
        assert_eq!(
            bottom(&shadow),
            bottom(&modal) + 1,
            "the shadow is not offset one row below the modal"
        );
    }

    #[test]
    fn it_stays_inside_the_rect_it_is_handed() {
        let area = Rect::new(10, 5, 80, 20);
        let buf = render(100, 30, |f| draw(f, area));
        assert_inside(&buf, area, "the daemon-lost modal");
    }

    #[test]
    fn it_survives_a_terminal_smaller_than_the_modal() {
        let small = try_render(20, 5, |f| draw(f, Rect::new(0, 0, 20, 5)))
            .unwrap_or_else(|e| panic!("the daemon-lost modal dies in a 20x5 terminal: {e}"));
        assert!(
            text(&small).contains("Daemon"),
            "at 20x5 the modal drew nothing readable:\n{}",
            text(&small)
        );

        let tiny = try_render(1, 1, |f| draw(f, Rect::new(0, 0, 1, 1)))
            .unwrap_or_else(|e| panic!("the daemon-lost modal dies in a 1x1 terminal: {e}"));
        assert!(
            painted_bounds(&tiny).is_some(),
            "at 1x1 the modal drew nothing at all"
        );
    }
}
