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

use super::widgets::{pad_right, selection_bg, titled_block, truncate, viewport_window};

/// Sidebar width. Sized for a session's label and the tail of its cwd; the
/// tile takes everything else.
pub const SIDEBAR_W: u16 = 24;

pub(super) fn draw_dashboard(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    tile_sizes: &mut super::TileSizes,
) {
    let (sidebar, main) = if app.show_sidebar {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_W), Constraint::Min(20)])
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
                format!(
                    "  Press {} then {} to spawn claude in a folder.",
                    crate::keys::PREFIX.label,
                    crate::keys::PREFIX_SPAWN.label
                ),
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

    // One row per session. Three rows each spent most of their cells on
    // padding, and the cwd they carried is what the resume picker is for.
    const ROW_HEIGHT: u16 = 1;
    let height = inner.height as usize;
    let total = app.sessions.len();
    // Keep a row for the overflow note when there is one, or it would be
    // drawn over the last session.
    let rows_fit = if total > height {
        height.saturating_sub(1).max(1)
    } else {
        height.max(1)
    };
    // Scroll to keep the focused session on screen. Drawing from row 0 hid
    // every session past the fold, the focused one included, so jumping to a
    // session below it switched the tile with nothing in the list to show it.
    let (start, end) = viewport_window(app.focus, total, rows_fit);
    let layout = SidebarLayout::new(total, app.sessions.iter().any(|s| s.dangerous), inner.width);

    let rows = app.sessions.iter().enumerate().take(end).skip(start);
    for (y, (i, s)) in (inner.y..inner.y + inner.height).zip(rows) {
        let row_area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: ROW_HEIGHT,
        };
        draw_sidebar_row(f, row_area, i, s, i == app.focus, &layout);
    }

    // Say how many are out of view, or the list looks complete when it is not.
    let hidden = total - (end - start);
    if hidden > 0 && inner.height > 0 {
        let note = Rect {
            x: inner.x,
            y: inner.y + inner.height - 1,
            width: inner.width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" +{hidden} more"),
                Style::default().fg(theme::FG_DIM),
            ))),
            note,
        );
    }
}

/// Column widths one pass over the sidebar shares. The number column grows
/// with the list, and the danger column exists only when something is
/// dangerous, so nothing is spent on a marker no row carries.
struct SidebarLayout {
    num_w: usize,
    danger: bool,
    name_w: usize,
}

impl SidebarLayout {
    /// Age column, as `format_duration_secs` fills it.
    const AGE_W: usize = 4;

    fn new(count: usize, danger: bool, width: u16) -> Self {
        let num_w = count.max(1).to_string().len();
        // gutter, dot, gap, number, gap, [danger, gap], name, gap, age, gutter
        let fixed = 1 + 1 + 1 + num_w + 1 + usize::from(danger) * 2 + 1 + Self::AGE_W + 1;
        let name_w = (width as usize).saturating_sub(fixed).max(4);
        Self {
            num_w,
            danger,
            name_w,
        }
    }

    /// What one row of this layout occupies, for checking the budget adds up.
    #[cfg(test)]
    fn row_width(&self) -> usize {
        1 + 1
            + 1
            + self.num_w
            + 1
            + usize::from(self.danger) * 2
            + self.name_w
            + 1
            + Self::AGE_W
            + 1
    }
}

fn draw_sidebar_row(
    f: &mut Frame,
    row_area: Rect,
    idx: usize,
    s: &Session,
    focused: bool,
    layout: &SidebarLayout,
) {
    let alive = s.alive.load(std::sync::atomic::Ordering::SeqCst);
    let age_ms = s.activity_age_ms();
    let (badge_glyph, badge_color) = sidebar_badge(s, alive, age_ms);

    if focused {
        selection_bg(f, row_area);
    }

    let name_style = if !alive {
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

    let mut spans: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(
            badge_glyph.to_string(),
            Style::default()
                .fg(badge_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>width$}", idx + 1, width = layout.num_w),
            Style::default().fg(theme::FG_MUTED),
        ),
        Span::raw(" "),
    ];
    if layout.danger {
        let cell = if s.dangerous {
            theme::glyph::DANGER
        } else {
            " "
        };
        spans.push(Span::styled(
            format!("{cell} "),
            Style::default().fg(theme::ACCENT_RED),
        ));
    }
    spans.push(Span::styled(
        pad_right(&truncate(&s.label, layout.name_w), layout.name_w),
        name_style,
    ));
    spans.push(Span::styled(
        format!(
            " {:>width$}",
            crate::util::format_duration_secs(age_ms / 1000),
            width = SidebarLayout::AGE_W
        ),
        Style::default().fg(theme::FG_MUTED),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), row_area);
}

/// Pure status-glyph picker for sidebar rows. Order matters: exit > prompt >
/// running > not running. Lives outside [`draw_sidebar_row`] so it can be
/// exercised without a `Frame`.
fn sidebar_badge(s: &Session, alive: bool, age_ms: u64) -> (String, Color) {
    if !alive {
        return (theme::glyph::EXITED.into(), theme::ACCENT_RED);
    }
    if s.attention {
        return (theme::glyph::PERMISSION.into(), theme::ACCENT_RED);
    }
    // One glyph, two colours, the same pair the resume picker uses: green
    // while the session runs, dimmed while it does not. The age line says how
    // long it has been quiet.
    let running = s.status == SessionStatus::Busy || age_ms < 1500;
    let color = if running {
        theme::ACCENT_GREEN
    } else {
        theme::FG_DIM
    };
    (theme::glyph::CONNECTION.into(), color)
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
    // How it ended, not just that it did. The sidebar row has no space for
    // it, and the tile is where a dead session is looked at.
    let state = match (alive, session.exit_status()) {
        (true, _) => String::new(),
        (false, Some(status)) => format!(" {status}"),
        (false, None) => " exited".to_string(),
    };
    format!(
        " {}{}{}{}{} ",
        zoom_marker, display_num, danger, session.label, state
    )
}

#[cfg(test)]
#[path = "../tests/dashboard.rs"]
mod tests;
