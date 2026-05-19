//! The main alt-screen body: focused tile + optional left sidebar.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::session::{ClaudeStatus, Session};
use crate::term_render::TermWidget;
use crate::theme;

use super::widgets::{collapse_cwd, titled_block, truncate};

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
        let bg = Block::default().style(Style::default().bg(theme::BG_ACTIVE));
        f.render_widget(bg, row_area);
        let strip_style = Style::default()
            .fg(theme::BORDER_FOCUS)
            .bg(theme::BG_ACTIVE);
        let strip_lines: Vec<Line> = (0..row_area.height)
            .map(|_| Line::from(Span::styled("▎", strip_style)))
            .collect();
        f.render_widget(
            Paragraph::new(strip_lines),
            Rect { x: row_area.x, y: row_area.y, width: 1, height: row_area.height },
        );
    }

    let text_area = Rect {
        x: row_area.x + 2,
        y: row_area.y,
        width: row_area.width.saturating_sub(2),
        height: row_area.height,
    };
    let avail = text_area.width as usize;

    let label_style = if !alive {
        Style::default().fg(theme::ACCENT_RED).add_modifier(Modifier::DIM)
    } else if focused {
        Style::default().fg(theme::BORDER_FOCUS).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::FG)
    };
    let state_suffix = if !alive { " (exited)" } else { "" };

    let header: Vec<Span> = vec![
        Span::styled(
            format!("{} ", badge_glyph),
            Style::default().fg(badge_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("[{}]", idx + 1), Style::default().fg(theme::FG_MUTED)),
        Span::raw(" "),
        Span::styled(
            if s.resume_id.is_some() { "↺" } else { " " }.to_string(),
            Style::default().fg(theme::ACCENT_CYAN),
        ),
        Span::styled(
            if s.dangerous { "⚠" } else { " " }.to_string(),
            Style::default().fg(theme::ACCENT_RED),
        ),
        Span::raw(" "),
        Span::styled(format!("{}{}", s.label, state_suffix), label_style),
    ];

    let cwd_str = collapse_cwd(&s.cwd.display().to_string());
    let lines: Vec<Line> = vec![
        Line::from(header),
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

/// Pure status-glyph picker for sidebar rows. Order matters: exit > prompt >
/// busy > idle > recent > dormant. Lives outside [`draw_sidebar_row`] so it
/// can be exercised without a `Frame`.
fn sidebar_badge(s: &Session, alive: bool, age_ms: u64, render_tick: u64) -> (String, Color) {
    if !alive {
        return ("✕".into(), theme::ACCENT_RED);
    }
    if s.permission_pending {
        return ("⚠".into(), theme::ACCENT_RED);
    }
    let busy = s.claude_status == ClaudeStatus::Busy || age_ms < 1500;
    if busy {
        return (
            theme::spinner_frame(render_tick).to_string(),
            theme::ACCENT_GREEN,
        );
    }
    if s.claude_status == ClaudeStatus::Idle {
        return ("○".into(), theme::ACCENT_CYAN);
    }
    if age_ms < 30_000 {
        return ("○".into(), theme::ACCENT_YELLOW);
    }
    ("·".into(), theme::FG_DIM)
}

fn sidebar_meta(s: &Session, age_ms: u64, max_width: usize) -> String {
    let age = humanize_short(age_ms);
    let status = match s.claude_status {
        ClaudeStatus::Busy => "busy",
        ClaudeStatus::Idle => "idle",
        ClaudeStatus::Unknown => "—",
    };
    let raw = format!("⏱ {}  ·  {}", age, status);
    if raw.chars().count() > max_width {
        raw.chars().take(max_width).collect()
    } else {
        raw
    }
}

fn humanize_short(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
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
            Style::default().fg(border_color).add_modifier(Modifier::BOLD),
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
    if session.permission_pending {
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
    if session.permission_pending {
        return theme::ACCENT_RED;
    }
    match session.claude_status {
        ClaudeStatus::Busy => theme::ACCENT_GREEN,
        ClaudeStatus::Idle => theme::ACCENT_CYAN,
        ClaudeStatus::Unknown => theme::FG,
    }
}

fn tile_title(session: &Session, alive: bool, zoomed: bool, display_num: usize) -> String {
    let danger = if session.dangerous { " ⚠ " } else { " " };
    let zoom_marker = if zoomed { "↕ " } else { "" };
    let state = if !alive { " EXITED" } else { "" };
    format!(
        " {}[{}]{}{}{} ",
        zoom_marker, display_num, danger, session.label, state
    )
}
