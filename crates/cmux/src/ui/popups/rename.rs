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
                "  {} save  {} cancel",
                keys::RENAME_SAVE.label,
                keys::RENAME_CANCEL.label,
            ),
            Style::default().fg(Color::DarkGray),
        )),
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

    fn state(session_id: u64, buf: &str) -> RenameState {
        RenameState {
            session_id,
            buf: buf.to_string(),
        }
    }

    #[test]
    fn it_shows_the_session_number_and_the_name_being_typed() {
        let app = app_with(&["alpha", "beta"]);
        let buf = render(80, 24, |f| draw(f, FULL, &state(2, "renamed"), &app));
        let out = text(&buf);

        assert!(out.contains("Rename session"), "no popup title:\n{out}");
        assert!(
            out.contains("Session [2]:"),
            "the wrong session is named:\n{out}"
        );
        assert!(
            out.contains("> renamed_"),
            "the typed name and its cursor are missing:\n{out}"
        );
    }

    #[test]
    fn an_unknown_session_id_leaves_the_number_blank() {
        let app = app_with(&["alpha"]);
        let buf = render(80, 24, |f| draw(f, FULL, &state(99, ""), &app));
        let out = text(&buf);
        assert!(
            out.contains("Session :"),
            "a missing session should leave the number off:\n{out}"
        );
    }

    #[test]
    fn it_names_the_keys_that_save_and_cancel() {
        let app = app_with(&["alpha"]);
        let buf = render(80, 24, |f| draw(f, FULL, &state(1, "x"), &app));
        let out = text(&buf);

        for needle in [
            keys::RENAME_SAVE.label,
            "save",
            keys::RENAME_CANCEL.label,
            "cancel",
        ] {
            assert!(
                out.contains(needle),
                "the hint row lacks {needle:?}:\n{out}"
            );
        }
        assert_legible(&buf, "rename");
    }

    #[test]
    fn a_long_name_is_cut_instead_of_widening_the_popup() {
        let app = app_with(&["alpha"]);
        let long = "z".repeat(200);

        let short_buf = render(80, 24, |f| draw(f, FULL, &state(1, "x"), &app));
        let long_buf = render(80, 24, |f| draw(f, FULL, &state(1, &long), &app));

        assert_eq!(
            painted_bounds(&short_buf),
            painted_bounds(&long_buf),
            "a 200-char name changed the popup's footprint"
        );
        assert!(
            !text(&long_buf).contains(&long),
            "the full 200-char name was drawn, so it overflowed the popup"
        );
    }

    #[test]
    fn it_stays_inside_the_rect_it_is_handed() {
        let app = app_with(&["alpha"]);
        let area = Rect::new(10, 4, 60, 14);
        let buf = render(80, 24, |f| draw(f, area, &state(1, "x"), &app));
        assert_inside(&buf, area, "the rename popup");
    }

    #[test]
    fn it_survives_a_terminal_smaller_than_the_popup() {
        let app = app_with(&["alpha"]);

        let small = try_render(20, 5, |f| {
            draw(f, Rect::new(0, 0, 20, 5), &state(1, "x"), &app)
        })
        .unwrap_or_else(|e| panic!("the rename popup dies in a 20x5 terminal: {e}"));
        assert!(
            text(&small).contains("Rename"),
            "at 20x5 the popup drew nothing readable:\n{}",
            text(&small)
        );

        let tiny = try_render(1, 1, |f| {
            draw(f, Rect::new(0, 0, 1, 1), &state(1, "x"), &app)
        })
        .unwrap_or_else(|e| panic!("the rename popup dies in a 1x1 terminal: {e}"));
        assert!(
            painted_bounds(&tiny).is_some(),
            "at 1x1 the popup drew nothing at all"
        );
    }
}
