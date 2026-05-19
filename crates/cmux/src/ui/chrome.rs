//! Persistent chrome around the body: titlebar, footer hint, transient toast.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, Mode};
use crate::keys;
use crate::theme;

use super::widgets::{chip, kbd_chip};

pub(super) fn draw_titlebar(f: &mut Frame, app: &App, area: Rect) {
    let on_dashboard = matches!(app.mode, Mode::Dashboard);
    let brand_color = if on_dashboard {
        theme::ACCENT_MAGENTA
    } else {
        theme::FG_DIM
    };
    let pos = if app.sessions.is_empty() {
        String::from("0/0")
    } else {
        format!("{}/{}", app.focus + 1, app.sessions.len())
    };
    let dim = Style::default().fg(theme::FG_DIM);
    let mut left_spans = vec![Span::styled(
        " ◆ cmux ",
        Style::default().fg(brand_color).add_modifier(Modifier::BOLD),
    )];
    if app.daemon.is_some() {
        left_spans.push(Span::raw(" "));
        left_spans.extend(chip(" cmuxd ", theme::ACCENT_GREEN));
    } else {
        left_spans.push(Span::raw(" "));
        left_spans.push(Span::styled(
            "local",
            Style::default()
                .fg(theme::FG_DIM)
                .add_modifier(Modifier::DIM),
        ));
    }
    left_spans.extend(vec![
        Span::styled(" · ", dim),
        Span::styled(
            pos,
            Style::default()
                .fg(theme::ACCENT_YELLOW)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let clock = current_clock_string();
    let clock_chars = clock.chars().count() as u16;
    let left_chars: usize = left_spans.iter().map(|s| s.content.chars().count()).sum();
    let mid_width = (area.width as usize)
        .saturating_sub(left_chars)
        .saturating_sub(clock_chars as usize)
        .saturating_sub(1);
    if mid_width > 0 {
        left_spans.push(Span::raw(" ".repeat(mid_width)));
    }
    left_spans.push(Span::styled(clock, Style::default().fg(theme::ACCENT_CYAN)));
    left_spans.push(Span::raw(" "));

    f.render_widget(Paragraph::new(Line::from(left_spans)), area);
}

fn current_clock_string() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// Bottom-right floating chip that drops on copy / status events.
pub(super) fn draw_toast(f: &mut Frame, tile: Rect, text: &str) {
    let label = format!(" {} ", text);
    let w = label.chars().count() as u16 + 2;
    let x = tile.x + tile.width.saturating_sub(w + 1);
    let y = tile.y + tile.height.saturating_sub(2);
    if x < tile.x || y < tile.y {
        return;
    }
    let rect = Rect { x, y, width: w, height: 1 };
    let spans = vec![
        Span::styled("\u{E0B6}", Style::default().fg(theme::BG_ACTIVE)),
        Span::styled(
            label,
            Style::default()
                .fg(theme::ACCENT_GREEN)
                .bg(theme::BG_ACTIVE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{E0B4}", Style::default().fg(theme::BG_ACTIVE)),
    ];
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(Line::from(spans)), rect);
}

/// Mode-dependent hint strip rendered at the bottom of the frame. All key
/// labels come from [`crate::keys`] so a chord rebind updates the hint
/// automatically.
pub(super) fn footer_for(app: &App) -> Line<'static> {
    if app.prefix_pending {
        return prefix_footer();
    }
    if matches!(app.mode, Mode::Dashboard) {
        return dashboard_footer(&app.status);
    }
    let (tag, rest, bg) = mode_footer(&app.mode, app);
    let mut spans = chip(tag, bg);
    spans.push(Span::styled(rest, Style::default().fg(theme::FG_MUTED)));
    Line::from(spans)
}

fn prefix_footer() -> Line<'static> {
    let mut spans = chip(" PREFIX ", theme::ACCENT_YELLOW);
    spans.push(Span::styled(
        format!(
            "  {}=new · ↑↓=cycle · {}=detach · {}=load · {} more ",
            keys::PREFIX_SPAWN.label,
            keys::PREFIX_DETACH.label,
            keys::PREFIX_PICKER.label,
            keys::PREFIX_HELP.label,
        ),
        Style::default().fg(theme::FG),
    ));
    Line::from(spans)
}

fn dashboard_footer(status: &str) -> Line<'static> {
    let mut spans = chip(" DASHBOARD ", theme::ACCENT_GREEN);
    spans.push(Span::raw("  "));
    spans.extend(kbd_chip(keys::PREFIX.label));
    spans.push(Span::styled(
        format!(
            "  then  {}=new · {}=load · ↑↓=cycle · 1-9=jump · {}=rename · {}=detach · {}=sidebar · {}=quit",
            keys::PREFIX_SPAWN.label,
            keys::PREFIX_PICKER.label,
            keys::PREFIX_RENAME.label,
            keys::PREFIX_DETACH.label,
            keys::PREFIX_TOGGLE_SIDEBAR.label,
            keys::PREFIX_QUIT.label,
        ),
        Style::default().fg(theme::FG_MUTED),
    ));
    if !status.is_empty() {
        spans.push(Span::styled(
            format!("  ·  {}", status),
            Style::default().fg(theme::ACCENT_YELLOW),
        ));
    }
    Line::from(spans)
}

/// `(chip-label, hint-text, accent-color)` for every non-Dashboard mode.
/// Dashboard is handled inline because it needs a second kbd_chip for the
/// prefix; everything else fits this triple.
fn mode_footer(mode: &Mode, app: &App) -> (&'static str, String, ratatui::style::Color) {
    match mode {
        Mode::Dashboard => unreachable!("handled by dashboard_footer"),
        Mode::Spawn(_) => (
            " SPAWN ",
            format!(
                "  {} pick · {} cancel · {} danger · {} select · {} / {} descend/ascend",
                keys::SPAWN_PICK.label,
                keys::SPAWN_CANCEL.label,
                keys::SPAWN_TOGGLE_DANGER.label,
                keys::SPAWN_UP.label,
                keys::SPAWN_DESCEND.label,
                keys::SPAWN_ASCEND.label,
            ),
            theme::ACCENT_CYAN,
        ),
        Mode::Rename(_) => (
            " RENAME ",
            format!(
                "  type new name · {} save · {} cancel",
                keys::RENAME_SAVE.label, keys::RENAME_CANCEL.label,
            ),
            theme::ACCENT_YELLOW,
        ),
        Mode::Picker(_) => (
            " RESUME ",
            format!(
                "  ↑↓ select · type to filter · {} resume · {} toggle danger · {} cancel",
                keys::PICKER_PICK.label,
                keys::PICKER_TOGGLE_DANGER.label,
                keys::PICKER_CANCEL.label,
            ),
            theme::ACCENT_MAGENTA,
        ),
        Mode::ConfirmDetach(_) => (
            " CONFIRM ",
            format!(
                "  {} detach · {} cancel",
                keys::CONFIRM_YES.label, keys::CONFIRM_NO.label,
            ),
            theme::ACCENT_RED,
        ),
        Mode::Help => (
            " HELP ",
            "  press any key to close".to_string(),
            theme::ACCENT_YELLOW,
        ),
        Mode::Reorder => (
            " REORDER ",
            format!(
                "  {} move focused session · {} exit",
                keys::REORDER_UP.label, keys::REORDER_EXIT.label,
            ),
            theme::ACCENT_MAGENTA,
        ),
        Mode::Scrollback(id) => {
            let offset = app
                .sessions
                .iter()
                .find(|s| s.id == *id)
                .and_then(|s| s.parser.lock().ok().map(|p| p.display_offset()))
                .unwrap_or(0);
            (
                " SCROLLBACK ",
                format!(
                    "  offset={} · {} line · {} / {} page · {} top · {} bottom · {} exit",
                    offset,
                    keys::SCROLLBACK_UP.label,
                    keys::SCROLLBACK_PAGE_UP.label,
                    keys::SCROLLBACK_PAGE_DOWN.label,
                    keys::SCROLLBACK_TOP.label,
                    keys::SCROLLBACK_BOTTOM.label,
                    keys::SCROLLBACK_EXIT.label,
                ),
                theme::ACCENT_PEACH,
            )
        }
    }
}

