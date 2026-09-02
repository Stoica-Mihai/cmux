//! `Ctrl+A n` — directory picker that spawns a fresh claude in the chosen folder.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::SpawnState;
use crate::keys;
use crate::theme;

use crate::ui::popups::dangerous::draw_dangerous_panel;
use crate::ui::widgets::{collapse_cwd, kbd_chip, open_popup, selection_bg, viewport_window};

pub(in crate::ui) fn draw(f: &mut Frame, area: Rect, spawn: &SpawnState) {
    let w = area.width.saturating_sub(8).clamp(50, 90);
    let h = area.height.saturating_sub(4).clamp(14, 28);
    let inner = open_popup(
        f,
        area,
        w,
        h,
        " Spawn claude in a folder ",
        theme::ACCENT_CYAN,
    );

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // cwd
            Constraint::Length(1), // separator
            Constraint::Min(3),    // list
            Constraint::Length(1), // gap
            Constraint::Length(3), // dangerous toggle (3 rows w/ vertical centering)
            Constraint::Length(1), // gap
            Constraint::Length(1), // hint
        ])
        .split(inner);

    let cwd_str = collapse_cwd(&spawn.cwd.display().to_string());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" cwd  ", Style::default().fg(theme::FG_DIM)),
            Span::styled(
                cwd_str,
                Style::default()
                    .fg(theme::ACCENT_YELLOW)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        layout[0],
    );

    draw_dir_list(f, layout[2], spawn);
    draw_dangerous_panel(
        f,
        layout[4],
        spawn.dangerous,
        keys::SPAWN_TOGGLE_DANGER.label,
    );
    draw_hint_row(f, layout[6]);
}

fn draw_dir_list(f: &mut Frame, list_area: Rect, spawn: &SpawnState) {
    if spawn.entries.is_empty() {
        let msg = if spawn.reading {
            "  reading..."
        } else {
            "  (no subdirectories. Enter spawns here, ← goes up)"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(theme::FG_DIM),
            ))),
            list_area,
        );
        return;
    }

    let (start, end) = viewport_window(
        spawn.selected,
        spawn.entries.len(),
        list_area.height as usize,
    );

    for (offset, i) in (start..end).enumerate() {
        let row_y = list_area.y + offset as u16;
        if row_y >= list_area.y + list_area.height {
            break;
        }
        let row_rect = Rect {
            x: list_area.x,
            y: row_y,
            width: list_area.width,
            height: 1,
        };
        let name = spawn.entries[i]
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        draw_dir_row(f, row_rect, &name, i == spawn.selected);
    }
}

fn draw_dir_row(f: &mut Frame, row_rect: Rect, name: &str, is_sel: bool) {
    if is_sel {
        selection_bg(f, row_rect);
    }
    let text_rect = Rect {
        x: row_rect.x + 2,
        y: row_rect.y,
        width: row_rect.width.saturating_sub(2),
        height: 1,
    };
    let style = if is_sel {
        Style::default()
            .fg(theme::ACCENT_CYAN)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::FG)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("{}/", name), style))),
        text_rect,
    );
}

fn draw_hint_row(f: &mut Frame, area: Rect) {
    let pairs = [
        (keys::SPAWN_UP.label, "select"),
        (keys::SPAWN_DESCEND.label, "descend"),
        (keys::SPAWN_ASCEND.label, "ascend"),
        (keys::SPAWN_TOGGLE_DANGER.label, "danger"),
        (keys::SPAWN_PICK.label, "pick"),
        (keys::SPAWN_CANCEL.label, "cancel"),
    ];
    // pre-compute total content width to evenly distribute remaining space as gaps
    let content_width: usize = pairs
        .iter()
        .map(|(k, l)| k.chars().count() + 2 + 1 + l.chars().count())
        .sum::<usize>();
    let avail = area.width as usize;
    let gap_count = pairs.len() + 1;
    let gap = avail
        .saturating_sub(content_width)
        .checked_div(gap_count)
        .unwrap_or(1)
        .max(1);
    let gap_str = " ".repeat(gap);

    let mut hint: Vec<Span<'static>> = Vec::new();
    for (key, label) in pairs.iter() {
        hint.push(Span::raw(gap_str.clone()));
        hint.extend(kbd_chip(key));
        hint.push(Span::styled(
            format!(" {}", label),
            Style::default().fg(theme::FG_MUTED),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(hint)), area);
}
