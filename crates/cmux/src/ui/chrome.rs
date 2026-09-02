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
        Style::default()
            .fg(brand_color)
            .add_modifier(Modifier::BOLD),
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
        Span::styled("  ", dim),
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
    let rect = Rect {
        x,
        y,
        width: w,
        height: 1,
    };
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

/// The prefix is down, so every chord below is live for the next keypress.
/// This is the moment to list them; the dashboard, where none of them do
/// anything yet, only says how to get here.
fn prefix_footer() -> Line<'static> {
    let mut spans = chip(" PREFIX ", theme::ACCENT_YELLOW);
    spans.push(Span::styled(
        format!(
            "  {}=new  {}=load  ↑↓=cycle  1-9=jump  {}=rename  {}=detach  {}=sidebar  {}=more  {}=quit",
            keys::PREFIX_SPAWN.label,
            keys::PREFIX_PICKER.label,
            keys::PREFIX_RENAME.label,
            keys::PREFIX_DETACH.label,
            keys::PREFIX_TOGGLE_SIDEBAR.label,
            keys::PREFIX_HELP.label,
            keys::PREFIX_QUIT.label,
        ),
        Style::default().fg(theme::FG),
    ));
    Line::from(spans)
}

/// Idle: none of the chords are live, so listing them here is noise. Say how
/// to reach them and where the full list lives.
fn dashboard_footer(status: &str) -> Line<'static> {
    let mut spans = chip(" DASHBOARD ", theme::ACCENT_GREEN);
    spans.push(Span::raw("  "));
    spans.extend(kbd_chip(keys::PREFIX.label));
    spans.push(Span::styled(
        "  then a key, or  ",
        Style::default().fg(theme::FG_MUTED),
    ));
    spans.extend(kbd_chip(keys::PREFIX_HELP.label));
    spans.push(Span::styled(
        "  for all of them",
        Style::default().fg(theme::FG_MUTED),
    ));
    if !status.is_empty() {
        spans.push(Span::styled(
            format!("  {}", status),
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
                "  {} pick  {} cancel  {} danger  {} select  {} / {} descend/ascend",
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
                "  type new name  {} save  {} cancel",
                keys::RENAME_SAVE.label,
                keys::RENAME_CANCEL.label,
            ),
            theme::ACCENT_YELLOW,
        ),
        Mode::Picker(_) => (
            " RESUME ",
            format!(
                "  ↑↓ select  type to filter  {} resume  {} toggle danger  {} cancel",
                keys::PICKER_PICK.label,
                keys::PICKER_TOGGLE_DANGER.label,
                keys::PICKER_CANCEL.label,
            ),
            theme::ACCENT_MAGENTA,
        ),
        Mode::ConfirmDetach(_) => (
            " CONFIRM ",
            format!(
                "  {} detach  {} cancel",
                keys::CONFIRM_YES.label,
                keys::CONFIRM_NO.label,
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
                "  {} move focused session  {} exit",
                keys::REORDER_UP.label,
                keys::REORDER_EXIT.label,
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
                    "  offset={}  {} line  {} / {} page  {} top  {} bottom  {} exit",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Chords are listed where they are live. At the dashboard none of them do
    /// anything until the prefix is down, so listing them there is noise; once
    /// it is down they are one keypress away and the full list belongs there.
    #[test]
    fn the_chord_list_lives_in_the_prefix_row_not_the_idle_one() {
        let idle = text(&dashboard_footer(""));
        let prefix = text(&prefix_footer());

        for chord in ["=new", "=load", "=rename", "=detach", "=sidebar", "=quit"] {
            assert!(
                prefix.contains(chord),
                "the prefix row should list {chord}: {prefix}"
            );
            assert!(
                !idle.contains(chord),
                "the idle row still lists {chord}, where it does nothing: {idle}"
            );
        }

        // The idle row still has to say how to reach them.
        assert!(idle.contains(keys::PREFIX.label), "{idle}");
        assert!(idle.contains(keys::PREFIX_HELP.label), "{idle}");
        assert!(
            idle.chars().count() < prefix.chars().count(),
            "the idle row should be the shorter of the two"
        );
    }

    #[test]
    fn a_status_message_is_appended_to_the_idle_row() {
        let plain = text(&dashboard_footer(""));
        let with_status = text(&dashboard_footer("spawned session [2]"));
        assert!(with_status.contains("spawned session [2]"));
        assert!(with_status.chars().count() > plain.chars().count());
    }

    /// The row said "Ctrl+A" twice: once as its own hint, once inside a status
    /// message that carried a second copy of the chord list.
    #[test]
    fn the_prefix_is_named_once_even_with_a_status() {
        for status in ["", "spawned session [2]", "resumed session [7]"] {
            let line = text(&dashboard_footer(status));
            let named = line.matches(keys::PREFIX.label).count();
            assert_eq!(named, 1, "the prefix is named {named} times: {line}");
        }
    }

    use crate::session::Session;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::path::PathBuf;

    fn render(w: u16, h: u16, body: impl FnOnce(&mut Frame)) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("backend");
        term.draw(body).expect("draw");
        term.backend().buffer().clone()
    }

    fn buffer_text(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app_with(n: u64) -> App {
        let mut app = App::new(PathBuf::from("/tmp"), (24, 80));
        for i in 1..=n {
            let (tx, _rx) = std::sync::mpsc::channel();
            app.sessions.push(
                Session::new_daemon(
                    i,
                    format!("s{i}"),
                    PathBuf::from("/tmp"),
                    false,
                    None,
                    24,
                    80,
                    None,
                    i,
                    tx,
                )
                .0,
            );
        }
        app
    }

    #[test]
    fn the_titlebar_counts_the_sessions_and_the_focused_one() {
        let empty = buffer_text(&render(80, 1, |f| {
            draw_titlebar(f, &app_with(0), Rect::new(0, 0, 80, 1))
        }));
        assert!(empty.contains("0/0"), "{empty}");

        let mut app = app_with(3);
        app.focus = 1;
        let some = buffer_text(&render(80, 1, |f| {
            draw_titlebar(f, &app, Rect::new(0, 0, 80, 1))
        }));
        assert!(some.contains("2/3"), "focus is 1-indexed for display: {some}");
    }

    /// Local mode and daemon mode look different on purpose: in local mode the
    /// sessions die with the process, and nothing else can see them.
    #[test]
    fn the_titlebar_says_which_mode_it_is_in() {
        let local = buffer_text(&render(80, 1, |f| {
            draw_titlebar(f, &app_with(1), Rect::new(0, 0, 80, 1))
        }));
        assert!(local.contains("local"), "{local}");
        assert!(!local.contains("cmuxd"), "{local}");
    }

    #[test]
    fn the_titlebar_fits_a_narrow_terminal_without_panicking() {
        for w in [1u16, 4, 12, 20] {
            let buf = render(w, 1, |f| {
                draw_titlebar(f, &app_with(2), Rect::new(0, 0, w, 1))
            });
            assert_eq!(buf.area.width, w, "it drew outside a {w}-wide area");
        }
    }

    #[test]
    fn a_toast_lands_inside_the_tile_it_is_given() {
        let tile = Rect::new(0, 0, 40, 10);
        let buf = render(40, 10, |f| draw_toast(f, tile, "copied"));
        let text = buffer_text(&buf);
        assert!(text.contains("copied"), "{text}");
        let row = text.lines().nth(8).unwrap_or("");
        assert!(row.contains("copied"), "the toast is not on the second-last row: {text}");
    }

    /// A tile too small to hold the chip must skip it rather than draw at a
    /// negative offset, which would land the toast on someone else's rows.
    #[test]
    fn a_toast_is_skipped_when_the_tile_cannot_hold_it() {
        for (w, h) in [(1u16, 1u16), (4, 2), (6, 1)] {
            let buf = render(40, 10, |f| draw_toast(f, Rect::new(0, 0, w, h), "copied"));
            assert_eq!(buf.area.width, 40, "it resized the frame at {w}x{h}");
        }
    }

    /// Every mode has to name itself in the footer, or a modal is on screen
    /// with nothing saying which keys are live.
    #[test]
    fn every_mode_gets_its_own_footer_tag() {
        let mut seen: Vec<String> = Vec::new();
        for mode in [
            Mode::Spawn(crate::app::SpawnState::new(PathBuf::from("/tmp"))),
            Mode::Rename(crate::app::RenameState {
                session_id: 1,
                buf: String::new(),
            }),
            Mode::ConfirmDetach(1),
            Mode::Scrollback(1),
            Mode::Help,
            Mode::Reorder,
        ] {
            let mut app = app_with(1);
            app.mode = mode;
            let line = text(&footer_for(&app));
            assert!(!line.trim().is_empty(), "a mode drew an empty footer");
            seen.push(line);
        }
        for (i, a) in seen.iter().enumerate() {
            for b in seen.iter().skip(i + 1) {
                assert_ne!(a, b, "two modes share a footer: {a}");
            }
        }
    }

    /// The prefix row wins over the mode row: once the prefix is down, the
    /// chords it lists are the live ones whatever mode is underneath.
    #[test]
    fn the_prefix_row_replaces_the_mode_row_while_the_prefix_is_down() {
        let mut app = app_with(1);
        app.mode = Mode::Help;
        let without = text(&footer_for(&app));
        app.prefix_pending = true;
        let with = text(&footer_for(&app));
        assert!(with.contains("PREFIX"), "{with}");
        assert_ne!(with, without);
    }

    #[test]
    fn every_footer_key_label_comes_from_the_chord_table() {
        let prefix = text(&prefix_footer());
        for c in [
            &keys::PREFIX_SPAWN,
            &keys::PREFIX_PICKER,
            &keys::PREFIX_RENAME,
            &keys::PREFIX_DETACH,
            &keys::PREFIX_TOGGLE_SIDEBAR,
            &keys::PREFIX_HELP,
            &keys::PREFIX_QUIT,
        ] {
            assert!(
                prefix.contains(c.label),
                "the prefix row does not name {:?}: {prefix}",
                c.label
            );
        }
    }
}
