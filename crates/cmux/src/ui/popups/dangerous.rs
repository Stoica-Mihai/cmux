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
        (theme::ACCENT_RED, "● ON", theme::ACCENT_RED, Modifier::BOLD)
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
        Rect {
            x: cols[0].x,
            y: mid_row,
            width: cols[0].width,
            height: 1,
        },
    );
    f.render_widget(
        Paragraph::new(label_line).style(Style::default().bg(panel_bg)),
        Rect {
            x: cols[1].x,
            y: mid_row,
            width: cols[1].width,
            height: 1,
        },
    );
    f.render_widget(
        Paragraph::new(key_line).style(Style::default().bg(panel_bg)),
        Rect {
            x: cols[2].x,
            y: mid_row,
            width: cols[2].width,
            height: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys;
    use crate::ui::popups::harness::{assert_inside, assert_legible, painted_bounds, render, text};
    use ratatui::buffer::Buffer;

    const ROW: Rect = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 3,
    };

    fn panel(active: bool) -> Buffer {
        render(80, 3, |f| {
            draw_dangerous_panel(f, ROW, active, keys::SPAWN_TOGGLE_DANGER.label)
        })
    }

    fn row_uses_fg(buf: &Buffer, y: u16, fg: Color) -> bool {
        (0..buf.area.width).any(|x| buf[(x, y)].fg == fg)
    }

    #[test]
    fn the_active_state_reads_on_and_the_idle_state_reads_off() {
        let on = text(&panel(true));
        let off = text(&panel(false));

        assert!(on.contains("ON"), "the active panel does not say ON:\n{on}");
        assert!(
            !on.contains("OFF"),
            "the active panel still says OFF:\n{on}"
        );
        assert!(
            off.contains("OFF"),
            "the idle panel does not say OFF:\n{off}"
        );

        for out in [&on, &off] {
            assert!(
                out.contains("--dangerously-skip-permissions"),
                "the panel does not name the flag it toggles:\n{out}"
            );
            assert!(
                out.contains(keys::SPAWN_TOGGLE_DANGER.label),
                "the panel does not name the key that toggles it:\n{out}"
            );
        }
    }

    #[test]
    fn the_two_states_do_not_render_alike() {
        let on = panel(true);
        let off = panel(false);
        let mid = ROW.height / 2;

        assert_ne!(text(&on), text(&off), "on and off render the same glyphs");
        assert_ne!(
            on[(0u16, 0u16)].bg,
            off[(0u16, 0u16)].bg,
            "on and off share a panel background"
        );
        assert!(
            row_uses_fg(&on, mid, theme::ACCENT_RED),
            "the active panel does not carry the danger accent"
        );
        assert!(
            !row_uses_fg(&off, mid, theme::ACCENT_RED),
            "the idle panel carries the danger accent, so off looks armed"
        );
    }

    #[test]
    fn both_states_are_legible() {
        assert_legible(&panel(true), "dangerous panel, on");
        assert_legible(&panel(false), "dangerous panel, off");
    }

    #[test]
    fn it_stays_inside_the_rect_it_is_handed() {
        let area = Rect::new(5, 2, 60, 3);
        for active in [true, false] {
            let buf = render(80, 10, |f| {
                draw_dangerous_panel(f, area, active, keys::SPAWN_TOGGLE_DANGER.label)
            });
            assert_inside(&buf, area, &format!("the dangerous panel, active={active}"));
        }
    }

    #[test]
    fn it_survives_a_row_too_narrow_for_its_columns() {
        for (w, h) in [(20u16, 5u16), (1, 1)] {
            for active in [true, false] {
                let buf = render(w, h, |f| {
                    draw_dangerous_panel(
                        f,
                        Rect::new(0, 0, w, h.min(3)),
                        active,
                        keys::SPAWN_TOGGLE_DANGER.label,
                    )
                });
                assert!(
                    painted_bounds(&buf).is_some(),
                    "{w}x{h} active={active} drew nothing at all"
                );
            }
        }
    }
}
