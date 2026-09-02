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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::popups::harness::{
        app_with, assert_inside, assert_legible, painted_bounds, render, text, try_render,
    };

    const FULL: Rect = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };

    #[test]
    fn it_names_the_session_it_is_about_to_kill() {
        let app = app_with(&["alpha", "beta"]);
        let buf = render(80, 24, |f| draw(f, FULL, 2, &app));
        let out = text(&buf);

        assert!(out.contains("Detach session?"), "no popup title:\n{out}");
        assert!(
            out.contains("Terminate session [2] 'beta' ?"),
            "the wrong session is named:\n{out}"
        );
        assert!(
            out.contains("Running claude process will be killed."),
            "the irreversible part is not spelled out:\n{out}"
        );
    }

    #[test]
    fn both_answers_carry_the_key_that_picks_them() {
        let app = app_with(&["alpha"]);
        let buf = render(80, 24, |f| draw(f, FULL, 1, &app));
        let out = text(&buf);

        for needle in [
            keys::CONFIRM_YES.label,
            "detach",
            keys::CONFIRM_NO.label,
            "cancel",
        ] {
            assert!(
                out.contains(needle),
                "the answer row lacks {needle:?}:\n{out}"
            );
        }
        assert_legible(&buf, "confirm_detach");
    }

    #[test]
    fn an_unknown_session_id_falls_back_to_a_placeholder() {
        let app = app_with(&["alpha"]);
        let buf = render(80, 24, |f| draw(f, FULL, 99, &app));
        let out = text(&buf);
        assert!(
            out.contains("Terminate session [0] '?' ?"),
            "a missing session should read as [0] '?':\n{out}"
        );
    }

    #[test]
    fn a_long_label_is_cut_instead_of_widening_the_popup() {
        let long = "z".repeat(200);
        let short = app_with(&["alpha"]);
        let wide = app_with(&[long.as_str()]);

        let short_buf = render(80, 24, |f| draw(f, FULL, 1, &short));
        let wide_buf = render(80, 24, |f| draw(f, FULL, 1, &wide));

        assert_eq!(
            painted_bounds(&short_buf),
            painted_bounds(&wide_buf),
            "a 200-char label changed the popup's footprint"
        );
        assert!(
            !text(&wide_buf).contains(&long),
            "the full 200-char label was drawn, so it overflowed the popup"
        );
    }

    #[test]
    fn it_stays_inside_the_rect_it_is_handed() {
        let app = app_with(&["alpha"]);
        let area = Rect::new(10, 4, 60, 14);
        let buf = render(80, 24, |f| draw(f, area, 1, &app));
        assert_inside(&buf, area, "the detach confirm");
    }

    #[test]
    fn it_survives_a_terminal_smaller_than_the_popup() {
        let app = app_with(&["alpha"]);

        let small = try_render(20, 5, |f| draw(f, Rect::new(0, 0, 20, 5), 1, &app))
            .unwrap_or_else(|e| panic!("the detach confirm dies in a 20x5 terminal: {e}"));
        assert!(
            text(&small).contains("Detach"),
            "at 20x5 the popup drew nothing readable:\n{}",
            text(&small)
        );

        let tiny = try_render(1, 1, |f| draw(f, Rect::new(0, 0, 1, 1), 1, &app))
            .unwrap_or_else(|e| panic!("the detach confirm dies in a 1x1 terminal: {e}"));
        assert!(
            painted_bounds(&tiny).is_some(),
            "at 1x1 the popup drew nothing at all"
        );
    }
}
