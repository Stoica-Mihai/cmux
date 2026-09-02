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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcripts::Transcript;
    use crate::ui::popups::harness::{
        assert_inside, assert_legible, painted_bounds, render, row, text, try_render,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    const FULL: Rect = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 30,
    };
    const CWD: &str = "/tmp/proj";

    fn transcript(id: &str, cwd: &str, title: Option<&str>) -> Transcript {
        Transcript {
            session_id: id.to_string(),
            cwd: PathBuf::from(cwd),
            mtime: SystemTime::now() - Duration::from_secs(90),
            file_size: 4096,
            custom_title: title.map(str::to_string),
        }
    }

    /// A picker state built field by field, so no test reads the user's real
    /// `~/.claude/projects` the way `PickerState::new` would.
    fn state(all: Vec<Transcript>, selected: usize) -> PickerState {
        PickerState {
            items: (0..all.len()).collect(),
            all,
            selected,
            dangerous: false,
            filter: String::new(),
            previews: HashMap::new(),
        }
    }

    fn one_row(t: Transcript, is_sel: bool) -> ratatui::buffer::Buffer {
        render(60, 1, |f| {
            draw_row(f, Rect::new(0, 0, 60, 1), &t, is_sel, false, 14, 20)
        })
    }

    #[test]
    fn the_title_counts_the_transcripts_it_found() {
        let s = state(
            vec![
                transcript("aaaaaaaa1", "/tmp/one", None),
                transcript("bbbbbbbb2", "/tmp/two", None),
                transcript("cccccccc3", "/tmp/three", None),
            ],
            0,
        );
        let buf = render(100, 30, |f| draw(f, FULL, &s));
        let out = text(&buf);

        assert!(
            out.contains("Resume past session (3 found)"),
            "the title does not count what it listed:\n{out}"
        );
        assert_legible(&buf, "picker");
    }

    #[test]
    fn an_empty_filter_invites_typing() {
        let s = state(vec![transcript("aaaaaaaa1", "/tmp/one", None)], 0);
        let buf = render(60, 1, |f| draw_filter_line(f, Rect::new(0, 0, 60, 1), &s));
        let out = text(&buf);

        assert!(out.contains("filter:"), "no filter prompt:\n{out}");
        assert!(
            out.contains("(type to search by cwd or --name)"),
            "an empty filter does not say what it searches:\n{out}"
        );
    }

    #[test]
    fn a_typed_filter_shows_what_it_matched() {
        let mut s = state(
            vec![
                transcript("aaaaaaaa1", "/tmp/one", None),
                transcript("bbbbbbbb2", "/tmp/two", None),
                transcript("cccccccc3", "/tmp/three", None),
            ],
            0,
        );
        s.filter = "tw".to_string();
        s.items = vec![1];

        let buf = render(60, 1, |f| draw_filter_line(f, Rect::new(0, 0, 60, 1), &s));
        let out = text(&buf);

        assert!(
            out.contains("filter: tw"),
            "the typed filter is missing:\n{out}"
        );
        assert!(
            out.contains("(1/3)"),
            "the matched-of-total count is missing:\n{out}"
        );
    }

    #[test]
    fn an_empty_list_says_where_it_looked() {
        let s = state(Vec::new(), 0);
        let buf = render(60, 5, |f| draw_rows(f, Rect::new(0, 0, 60, 5), &s));
        let out = text(&buf);
        assert!(
            out.contains("(no past sessions found in ~/.claude/projects)"),
            "an empty picker leaves the user guessing:\n{out}"
        );
    }

    #[test]
    fn a_selected_row_is_accented_on_the_selection_background() {
        let sel = one_row(transcript("aaaaaaaa1", CWD, None), true);
        let plain = one_row(transcript("aaaaaaaa1", CWD, None), false);

        assert!(
            text(&sel).contains(&collapse_cwd(CWD)),
            "the selected row lost its cwd"
        );
        assert!(
            text(&plain).contains(&collapse_cwd(CWD)),
            "the plain row lost its cwd"
        );

        assert_eq!(
            sel[(0u16, 0u16)].bg,
            theme::BG_ACTIVE,
            "the selected row has no selection background"
        );
        assert_eq!(
            plain[(0u16, 0u16)].bg,
            Color::Reset,
            "the unselected row is painted with a selection background"
        );
        assert_eq!(
            sel[(2u16, 0u16)].fg,
            theme::BORDER_FOCUS,
            "the selected row is not accented"
        );
        assert!(
            sel[(2u16, 0u16)].modifier.contains(Modifier::BOLD),
            "the selected row is not bold"
        );
        assert_eq!(
            plain[(2u16, 0u16)].fg,
            theme::FG,
            "the unselected row is not plain foreground"
        );
        assert!(
            !plain[(2u16, 0u16)].modifier.contains(Modifier::BOLD),
            "the unselected row is bold, so selection is invisible"
        );
    }

    #[test]
    fn a_long_cwd_is_cut_to_its_column_with_a_leading_ellipsis() {
        let long = format!("/{}", "d".repeat(60));
        let buf = render(60, 1, |f| {
            draw_row(
                f,
                Rect::new(0, 0, 60, 1),
                &transcript("aaaaaaaa1", &long, None),
                false,
                false,
                14,
                15,
            )
        });
        let line = row(&buf, 0);

        assert!(line.contains('…'), "a long cwd was not truncated: {line:?}");
        assert!(
            !line.contains(&long),
            "the full 61-char cwd was drawn: {line:?}"
        );
        assert_eq!(
            line.chars().count(),
            60,
            "the row no longer matches the width it was given: {line:?}"
        );
    }

    #[test]
    fn the_name_column_appears_only_when_a_transcript_carries_one() {
        let named = state(vec![transcript("aaaaaaaa1", CWD, Some("mytitle"))], 0);
        let plain = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);

        let with = render(80, 1, |f| draw_rows(f, Rect::new(0, 0, 80, 1), &named));
        let without = render(80, 1, |f| draw_rows(f, Rect::new(0, 0, 80, 1), &plain));

        assert!(
            row(&with, 0).starts_with("  mytitle"),
            "the name column is missing: {:?}",
            row(&with, 0)
        );
        assert!(
            row(&without, 0).starts_with(&format!("  {}", collapse_cwd(CWD))),
            "an empty name column is still reserved: {:?}",
            row(&without, 0)
        );
    }

    #[test]
    fn the_preview_pane_shows_the_selected_transcript() {
        let mut s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
        s.previews.insert(
            "aaaaaaaa1".to_string(),
            "hello from the transcript".to_string(),
        );

        let loaded = text(&render(100, 30, |f| draw(f, FULL, &s)));
        assert!(
            loaded.contains(" preview "),
            "the preview pane has no heading:\n{loaded}"
        );
        assert!(
            loaded.contains("hello from the transcript"),
            "the loaded preview is not shown:\n{loaded}"
        );

        let pending = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
        let pending = text(&render(100, 30, |f| draw(f, FULL, &pending)));
        assert!(
            pending.contains("(loading...)"),
            "a transcript with no preview yet shows nothing at all:\n{pending}"
        );
    }

    #[test]
    fn the_hint_row_names_every_key_it_offers() {
        let s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
        let out = text(&render(100, 30, |f| draw(f, FULL, &s)));

        for key in [
            keys::PICKER_FILTER_CLEAR.label,
            keys::PICKER_PICK.label,
            keys::PICKER_TOGGLE_DANGER.label,
            keys::PICKER_CANCEL.label,
        ] {
            assert!(out.contains(key), "the hint row lacks {key:?}:\n{out}");
        }
        for label in ["select", "type to filter", "clear", "resume", "cancel"] {
            assert!(out.contains(label), "the hint row lacks {label:?}:\n{out}");
        }
    }

    #[test]
    fn it_stays_inside_the_rect_it_is_handed() {
        let s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);
        let area = Rect::new(5, 2, 80, 24);
        let buf = render(100, 30, |f| draw(f, area, &s));
        assert_inside(&buf, area, "the resume picker");
    }

    #[test]
    fn it_survives_a_terminal_smaller_than_the_popup() {
        let s = state(vec![transcript("aaaaaaaa1", CWD, None)], 0);

        let small = try_render(20, 5, |f| draw(f, Rect::new(0, 0, 20, 5), &s))
            .unwrap_or_else(|e| panic!("the resume picker dies in a 20x5 terminal: {e}"));
        assert!(
            text(&small).contains("Resume"),
            "at 20x5 the popup drew nothing readable:\n{}",
            text(&small)
        );

        let tiny = try_render(1, 1, |f| draw(f, Rect::new(0, 0, 1, 1), &s))
            .unwrap_or_else(|e| panic!("the resume picker dies in a 1x1 terminal: {e}"));
        assert!(
            painted_bounds(&tiny).is_none(),
            "at 1x1 the picker has no room left after its 1-cell margin, so it should draw nothing"
        );
    }
}
