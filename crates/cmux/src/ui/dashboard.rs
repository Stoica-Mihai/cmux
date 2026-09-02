//! The main alt-screen body: focused tile + optional left sidebar.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::session::{Session, SessionStatus};
use crate::term_render::TermWidget;
use crate::theme;

use super::widgets::{collapse_cwd, selection_bg, titled_block, truncate};

pub(super) fn draw_dashboard(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    tile_sizes: &mut super::TileSizes,
) {
    let (sidebar, main) = if app.show_sidebar {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(32), Constraint::Min(20)])
            .split(area);
        (Some(split[0]), split[1])
    } else {
        (None, area)
    };

    if let Some(sidebar) = sidebar {
        draw_sidebar(f, app, sidebar);
    }

    if app.sessions.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No sessions yet.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Ctrl+A then n to spawn claude in a folder.",
                Style::default().fg(Color::Gray),
            )),
        ])
        .block(titled_block(" preview ", Color::DarkGray));
        f.render_widget(empty, main);
        return;
    }

    let tick = app.render_tick;
    if let Some(session) = app.sessions.get(app.focus) {
        let inner = draw_tile(f, session, main, true, false, app.focus + 1, tick);
        tile_sizes.push((app.focus, inner.height, inner.width));
        app.last_tile_area = Some(inner);
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let block = titled_block(" sessions ", theme::ACCENT_GREEN);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.sessions.is_empty() {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  (empty)",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        f.render_widget(hint, inner);
        return;
    }

    const ROW_HEIGHT: u16 = 3;
    let mut y = inner.y;
    for (i, s) in app.sessions.iter().enumerate() {
        if y + ROW_HEIGHT > inner.y + inner.height {
            break;
        }
        let row_area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: ROW_HEIGHT,
        };
        y += ROW_HEIGHT;
        draw_sidebar_row(f, row_area, i, s, i == app.focus, app.render_tick);
    }
}

fn draw_sidebar_row(
    f: &mut Frame,
    row_area: Rect,
    idx: usize,
    s: &Session,
    focused: bool,
    render_tick: u64,
) {
    let alive = s.alive.load(std::sync::atomic::Ordering::SeqCst);
    let age_ms = s.activity_age_ms();
    let (badge_glyph, badge_color) = sidebar_badge(s, alive, age_ms, render_tick);

    if focused {
        selection_bg(f, row_area);
    }

    let text_area = Rect {
        x: row_area.x + 2,
        y: row_area.y,
        width: row_area.width.saturating_sub(2),
        height: row_area.height,
    };
    let avail = text_area.width as usize;

    let cwd_str = collapse_cwd(&s.cwd.display().to_string());
    let lines: Vec<Line> = vec![
        Line::from(sidebar_header_spans(
            s,
            idx,
            &badge_glyph,
            badge_color,
            alive,
            focused,
        )),
        Line::from(Span::styled(
            format!("    {}", truncate(&cwd_str, avail.saturating_sub(4))),
            Style::default().fg(theme::FG_DIM),
        )),
        Line::from(Span::styled(
            format!("    {}", sidebar_meta(s, age_ms, avail.saturating_sub(4))),
            Style::default().fg(theme::FG_MUTED),
        )),
    ];
    f.render_widget(Paragraph::new(lines), text_area);
}

/// Build the top line of a sidebar row: badge, index, optional resume/danger
/// glyphs, and the session label. Style is gated by alive/focused so callers
/// don't have to recompute it.
fn sidebar_header_spans(
    s: &Session,
    idx: usize,
    badge_glyph: &str,
    badge_color: Color,
    alive: bool,
    focused: bool,
) -> Vec<Span<'static>> {
    let label_style = if !alive {
        Style::default()
            .fg(theme::ACCENT_RED)
            .add_modifier(Modifier::DIM)
    } else if focused {
        Style::default()
            .fg(theme::BORDER_FOCUS)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::FG)
    };
    let state_suffix = match (alive, s.exit_status()) {
        (true, _) => String::new(),
        (false, Some(status)) => format!(" ({status})"),
        (false, None) => " (exited)".to_string(),
    };
    vec![
        Span::styled(
            format!("{} ", badge_glyph),
            Style::default()
                .fg(badge_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[{}]", idx + 1),
            Style::default().fg(theme::FG_MUTED),
        ),
        Span::raw(" "),
        Span::styled(
            if s.resume_id.is_some() {
                theme::glyph::RESUMED
            } else {
                " "
            }
            .to_string(),
            Style::default().fg(theme::ACCENT_CYAN),
        ),
        Span::styled(
            if s.dangerous {
                theme::glyph::DANGER
            } else {
                " "
            }
            .to_string(),
            Style::default().fg(theme::ACCENT_RED),
        ),
        Span::raw(" "),
        Span::styled(format!("{}{}", s.label, state_suffix), label_style),
    ]
}

/// Pure status-glyph picker for sidebar rows. Order matters: exit > prompt >
/// busy > idle > recent > dormant. Lives outside [`draw_sidebar_row`] so it
/// can be exercised without a `Frame`.
fn sidebar_badge(s: &Session, alive: bool, age_ms: u64, render_tick: u64) -> (String, Color) {
    if !alive {
        return (theme::glyph::EXITED.into(), theme::ACCENT_RED);
    }
    if s.attention {
        return (theme::glyph::PERMISSION.into(), theme::ACCENT_RED);
    }
    let busy = s.status == SessionStatus::Busy || age_ms < 1500;
    if busy {
        return (
            theme::spinner_frame(render_tick).to_string(),
            theme::ACCENT_GREEN,
        );
    }
    if s.status == SessionStatus::Idle {
        return (theme::glyph::IDLE.into(), theme::ACCENT_CYAN);
    }
    if age_ms < 30_000 {
        return (theme::glyph::IDLE.into(), theme::ACCENT_YELLOW);
    }
    // Dormant: the idle glyph, dimmed. Keeps the badge column filled so rows
    // do not shift, and reads as "idle, for longer".
    (theme::glyph::IDLE.into(), theme::FG_DIM)
}

fn sidebar_meta(s: &Session, age_ms: u64, max_width: usize) -> String {
    let age = crate::util::format_duration_secs(age_ms / 1000, "");
    let status = match s.status {
        SessionStatus::Busy => "busy",
        SessionStatus::Idle => "idle",
        SessionStatus::Unknown => "-",
    };
    let raw = format!("⏱ {}  {}", age, status);
    if raw.chars().count() > max_width {
        raw.chars().take(max_width).collect()
    } else {
        raw
    }
}

fn draw_tile(
    f: &mut Frame,
    session: &Session,
    area: Rect,
    focused: bool,
    zoomed: bool,
    display_num: usize,
    render_tick: u64,
) -> Rect {
    let alive = session.alive.load(std::sync::atomic::Ordering::SeqCst);
    let border_color = tile_border_color(session, alive, focused, zoomed, render_tick);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            tile_title(session, alive, zoomed, display_num),
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let content_area = Rect {
        x: inner.x.saturating_add(1),
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    };

    let cursor_bg = tile_cursor_bg(session);
    if let Some(sb) = &session.scrollback {
        let widget = TermWidget::new(&sb.term)
            .with_selection(session.selection)
            .with_cursor_bg(cursor_bg);
        f.render_widget(widget, content_area);
    } else if let Ok(parser) = session.parser.lock() {
        let widget = TermWidget::new(&parser.term)
            .with_selection(session.selection)
            .with_cursor_bg(cursor_bg);
        f.render_widget(widget, content_area);
    }
    content_area
}

/// Border color encodes state priority: exit > permission-pending pulse >
/// zoom > focus > idle. The pulse is wall-clock-driven (one toggle per
/// `PULSE_PERIOD_MS`) so it stays ~1 Hz regardless of how often the frame
/// is redrawn — render_tick advanced on every dirty byte, which made the
/// old `render_tick % 2` pulse flicker at the PTY rate instead of pulsing
/// like a heartbeat.
fn tile_border_color(
    session: &Session,
    alive: bool,
    focused: bool,
    zoomed: bool,
    _render_tick: u64,
) -> Color {
    const PULSE_PERIOD_MS: u64 = 900;
    if !alive {
        return theme::BORDER_DEAD;
    }
    if session.attention {
        let phase = crate::util::now_ms() / PULSE_PERIOD_MS;
        return if phase.is_multiple_of(2) {
            theme::BORDER_DEAD
        } else {
            theme::ACCENT_RED_DIM
        };
    }
    if zoomed {
        return theme::ACCENT_MAGENTA;
    }
    if focused {
        theme::BORDER_FOCUS
    } else {
        theme::BORDER_IDLE
    }
}

fn tile_cursor_bg(session: &Session) -> Color {
    if session.attention {
        return theme::ACCENT_RED;
    }
    match session.status {
        SessionStatus::Busy => theme::ACCENT_GREEN,
        SessionStatus::Idle => theme::ACCENT_CYAN,
        SessionStatus::Unknown => theme::FG,
    }
}

fn tile_title(session: &Session, alive: bool, zoomed: bool, display_num: usize) -> String {
    let danger = if session.dangerous {
        format!(" {} ", theme::glyph::DANGER)
    } else {
        " ".to_string()
    };
    let zoom_marker = if zoomed { "↕ " } else { "" };
    let state = if !alive { " EXITED" } else { "" };
    format!(
        " {}[{}]{}{}{} ",
        zoom_marker, display_num, danger, session.label, state
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;

    fn session() -> Session {
        let (tx, _rx) = mpsc::channel();
        Session::new_daemon(
            1,
            "s".into(),
            PathBuf::from("/tmp"),
            false,
            None,
            24,
            80,
            None,
            1,
            tx,
        )
        .0
    }

    /// No badge state renders a bare dot. It read as decoration next to the
    /// other glyphs rather than as a status.
    #[test]
    fn no_badge_state_is_a_bare_dot() {
        let mut s = session();
        let states = [
            (false, false, SessionStatus::Unknown, 0u64),
            (true, true, SessionStatus::Unknown, 0),
            (true, false, SessionStatus::Busy, 0),
            (true, false, SessionStatus::Idle, 60_000),
            (true, false, SessionStatus::Unknown, 10_000),
            (true, false, SessionStatus::Unknown, 60_000),
        ];
        for (alive, attention, status, age) in states {
            s.attention = attention;
            s.status = status;
            let (glyph, _) = sidebar_badge(&s, alive, age, 0);
            assert_ne!(glyph, "·", "a badge still renders a bare dot: {glyph:?}");
        }
    }

    /// Dormant is the idle glyph dimmed, not a different shape — but it still
    /// has to be tellable apart, so the colour must differ.
    #[test]
    fn dormant_is_the_idle_glyph_in_a_dimmer_colour() {
        let mut s = session();
        s.attention = false;
        s.status = SessionStatus::Unknown;

        let (recent, recent_color) = sidebar_badge(&s, true, 10_000, 0);
        let (dormant, dormant_color) = sidebar_badge(&s, true, 60_000, 0);

        assert_eq!(recent, theme::glyph::IDLE);
        assert_eq!(dormant, theme::glyph::IDLE);
        assert_ne!(
            recent_color, dormant_color,
            "dormant and recent render the same, so the state is invisible"
        );
    }

    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

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

    fn app_with(labels: &[&str]) -> App {
        let mut app = App::new(PathBuf::from("/tmp"), (24, 100));
        for (i, label) in labels.iter().enumerate() {
            let (tx, _rx) = mpsc::channel();
            let id = i as u64 + 1;
            app.sessions.push(
                Session::new_daemon(
                    id,
                    (*label).into(),
                    PathBuf::from("/tmp"),
                    false,
                    None,
                    24,
                    80,
                    None,
                    id,
                    tx,
                )
                .0,
            );
        }
        app
    }

    /// The badge ranking decides what a row shows when two states are true at
    /// once, so each rank has to beat the one below it.
    #[test]
    fn the_badge_ranking_puts_exit_above_attention_above_busy() {
        let mut s = session();
        s.attention = true;
        s.status = SessionStatus::Busy;

        let (dead, _) = sidebar_badge(&s, false, 0, 0);
        assert_eq!(dead, theme::glyph::EXITED, "exit should outrank everything");

        let (attn, _) = sidebar_badge(&s, true, 0, 0);
        assert_eq!(
            attn,
            theme::glyph::PERMISSION,
            "attention should outrank busy"
        );

        s.attention = false;
        let (busy, _) = sidebar_badge(&s, true, 60_000, 0);
        assert_ne!(busy, theme::glyph::PERMISSION);
    }

    /// Recent output counts as busy even before the probe says so, or a
    /// session that just printed a page looks idle while it is still working.
    #[test]
    fn recent_output_reads_as_busy_without_waiting_for_the_probe() {
        let mut s = session();
        s.status = SessionStatus::Unknown;
        let (fresh, _) = sidebar_badge(&s, true, 100, 0);
        let (stale, _) = sidebar_badge(&s, true, 60_000, 0);
        assert_ne!(fresh, stale, "output age made no difference to the badge");
    }

    #[test]
    fn the_busy_badge_animates_across_ticks() {
        let mut s = session();
        s.status = SessionStatus::Busy;
        let frames: std::collections::HashSet<String> =
            (0..8).map(|t| sidebar_badge(&s, true, 0, t).0).collect();
        assert!(frames.len() > 1, "the spinner never changed: {frames:?}");
    }

    #[test]
    fn the_sidebar_meta_line_is_clipped_to_the_width_it_is_given() {
        let s = session();
        for width in [0usize, 1, 5, 12, 40] {
            let line = sidebar_meta(&s, 90_000, width);
            assert!(
                line.chars().count() <= width,
                "meta is {} chars in a {width}-wide column: {line:?}",
                line.chars().count()
            );
        }
    }

    #[test]
    fn the_sidebar_meta_line_names_the_status() {
        let mut s = session();
        for (status, want) in [
            (SessionStatus::Busy, "busy"),
            (SessionStatus::Idle, "idle"),
            (SessionStatus::Unknown, "-"),
        ] {
            s.status = status;
            let line = sidebar_meta(&s, 5_000, 40);
            assert!(line.contains(want), "{status:?} rendered as {line:?}");
        }
    }

    /// A dead tile has to be tellable from a live one at a glance, and the
    /// focused one from the rest.
    #[test]
    fn the_tile_border_separates_dead_focused_and_idle() {
        let s = session();
        let dead = tile_border_color(&s, false, true, false, 0);
        let focused = tile_border_color(&s, true, true, false, 0);
        let idle = tile_border_color(&s, true, false, false, 0);
        let zoomed = tile_border_color(&s, true, true, true, 0);
        assert_ne!(dead, focused);
        assert_ne!(focused, idle);
        assert_ne!(zoomed, focused);
    }

    #[test]
    fn the_tile_cursor_colour_follows_the_status() {
        let mut s = session();
        s.status = SessionStatus::Busy;
        let busy = tile_cursor_bg(&s);
        s.status = SessionStatus::Idle;
        let idle = tile_cursor_bg(&s);
        assert_ne!(busy, idle);

        s.attention = true;
        assert_eq!(
            tile_cursor_bg(&s),
            theme::ACCENT_RED,
            "attention should override the status colour"
        );
    }

    #[test]
    fn the_tile_title_carries_the_number_the_label_and_the_state() {
        let mut s = session();
        s.label = "worker".into();
        let live = tile_title(&s, true, false, 2);
        assert!(live.contains("[2]"), "{live}");
        assert!(live.contains("worker"), "{live}");
        assert!(!live.contains("EXITED"), "{live}");

        let dead = tile_title(&s, false, false, 2);
        assert!(dead.contains("EXITED"), "a dead tile should say so: {dead}");

        let zoomed = tile_title(&s, true, true, 2);
        assert_ne!(zoomed, live, "a zoomed tile should be marked");
    }

    #[test]
    fn the_dashboard_lists_every_session_in_the_sidebar() {
        let mut app = app_with(&["alpha", "beta", "gamma"]);
        let mut sizes: super::super::TileSizes = Vec::new();
        let buf = render(100, 24, |f| {
            draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut sizes)
        });
        let text = buffer_text(&buf);
        for label in ["alpha", "beta", "gamma"] {
            assert!(text.contains(label), "{label} is missing: {text}");
        }
    }

    #[test]
    fn the_dashboard_draws_with_no_sessions_at_all() {
        let mut app = app_with(&[]);
        let mut sizes: super::super::TileSizes = Vec::new();
        let buf = render(100, 24, |f| {
            draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut sizes)
        });
        assert_eq!(buf.area.width, 100);
    }

    /// Both sides of the toggle, because hiding the sidebar is what gives the
    /// tiles the extra width.
    #[test]
    fn hiding_the_sidebar_gives_the_tile_more_width() {
        let mut app = app_with(&["alpha"]);
        let mut with: super::super::TileSizes = Vec::new();
        app.show_sidebar = true;
        render(100, 24, |f| {
            draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut with)
        });
        let mut without: super::super::TileSizes = Vec::new();
        app.show_sidebar = false;
        render(100, 24, |f| {
            draw_dashboard(f, &mut app, Rect::new(0, 0, 100, 24), &mut without)
        });
        assert!(
            without[0].2 > with[0].2,
            "hiding the sidebar did not widen the tile: {with:?} then {without:?}"
        );
    }

    #[test]
    fn the_dashboard_survives_a_terminal_too_small_for_its_layout() {
        for (w, h) in [(1u16, 1u16), (4, 3), (10, 4), (20, 6)] {
            let mut app = app_with(&["alpha", "beta"]);
            let mut sizes: super::super::TileSizes = Vec::new();
            let buf = render(w, h, |f| {
                draw_dashboard(f, &mut app, Rect::new(0, 0, w, h), &mut sizes)
            });
            assert_eq!(buf.area.width, w, "it drew outside a {w}x{h} frame");
        }
    }
}
