use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Mode};
use crate::session::Session;
use crate::term_render::TermWidget;

pub type TileSizes = Vec<(usize, u16, u16)>;

pub fn draw(f: &mut Frame, app: &mut App, tile_sizes: &mut TileSizes) {
    tile_sizes.clear();
    let area = f.area();
    app.last_tile_area = None;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let body = chunks[0];
    let footer = chunks[1];

    draw_dashboard(f, app, body, tile_sizes);

    let footer_text = footer_for(app);
    f.render_widget(Paragraph::new(footer_text), footer);

    match &app.mode {
        Mode::Spawn(s) => draw_spawn_popup(f, area, s),
        Mode::Rename(s) => draw_rename_popup(f, area, s, app),
        Mode::Picker(s) => draw_picker_popup(f, area, s),
        Mode::ConfirmDetach(id) => draw_confirm_detach(f, area, *id, app),
        Mode::Help => draw_help_popup(f, area),
        Mode::Dashboard | Mode::Scrollback(_) | Mode::Reorder => {}
    }
}

fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

fn titled_block(title: impl Into<String>, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title.into())
        .border_style(Style::default().fg(color))
}

fn open_popup(f: &mut Frame, area: Rect, w: u16, h: u16, title: &str, color: Color) -> Rect {
    let popup = centered_rect(area, w, h);
    f.render_widget(Clear, popup);
    let block = titled_block(title.to_string(), color);
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    inner
}

fn dangerous_line(active: bool) -> Line<'static> {
    if active {
        Line::from(Span::styled(
            " [x] --dangerously-skip-permissions  (Space toggles)",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(Span::styled(
            " [ ] --dangerously-skip-permissions  (Space toggles)",
            Style::default().fg(Color::Gray),
        ))
    }
}

fn draw_help_popup(f: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(4).clamp(60, 76);
    let h = area.height.saturating_sub(2).clamp(20, 28);
    let inner = open_popup(f, area, w, h, " cmux — cheat sheet ", Color::Yellow);

    let header = |s: &'static str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
    };
    let row = |chord: &str, desc: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {:<14}", chord),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(desc.to_string(), Style::default().fg(Color::White)),
        ])
    };
    let note = |s: &str| {
        Line::from(Span::styled(
            format!("  {}", s),
            Style::default().fg(Color::DarkGray),
        ))
    };

    let lines: Vec<Line> = vec![
        header(" Prefix chords  (Ctrl+A then…)"),
        row("n", "spawn new claude in a folder"),
        row("l", "resume picker (past sessions)"),
        row("r", "rename focused session"),
        row("d", "detach focused (with confirm)"),
        row("[", "enter scrollback mode"),
        row("m", "enter reorder mode (move sessions)"),
        row("↑ / ↓", "cycle focused session"),
        row("1 .. 9", "jump to session N"),
        row("z", "toggle sidebar"),
        row("a", "send literal Ctrl+A to focused claude"),
        row("?", "this help"),
        row("q", "quit (kills all sessions)"),
        Line::from(""),
        header(" Global"),
        row("Ctrl+Q", "hard quit from anywhere"),
        Line::from(""),
        header(" Mouse"),
        note("drag inside tile → copies selection via OSC 52"),
        note("Shift+drag → bypass cmux, use outer terminal selection"),
        Line::from(""),
        header(" Sidebar badges"),
        note("● green  busy (claude working)"),
        note("○ cyan   idle (claude waiting for input)"),
        note("⚠ red    permission prompt waiting"),
        note("· gray   dormant"),
        note("✕ red    session exited"),
        note("↺        resumed session"),
        Line::from(""),
        header(" Inside popups"),
        note("Spawn   ↑↓ select  ·  → descend  ·  ← ascend  ·  Space dangerous  ·  Enter pick"),
        note("Resume  ↑↓ select  ·  type to filter  ·  Tab dangerous  ·  Enter resume"),
        note("Scroll  ↑↓ line  ·  PgUp/PgDn page  ·  g top  ·  G bottom  ·  q exit"),
        Line::from(""),
        note("press any key to close"),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm_detach(f: &mut Frame, area: Rect, id: u64, app: &App) {
    let w = area.width.clamp(40, 60);
    let h: u16 = 7;
    let inner = open_popup(f, area, w, h, " Detach session? ", Color::Red);

    let (pos, label) = app
        .sessions
        .iter()
        .enumerate()
        .find(|(_, s)| s.id == id)
        .map(|(i, s)| (i + 1, s.label.clone()))
        .unwrap_or((0, "?".to_string()));

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Terminate session [{}] '{}' ?", pos, label),
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [y]es / Enter to confirm   ·   [n] / Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_picker_popup(f: &mut Frame, area: Rect, state: &crate::app::PickerState) {
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
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let filter_text = if state.filter.is_empty() {
        Line::from(vec![
            Span::styled(" filter: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "(type to search by cwd)",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" filter: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                state.filter.clone(),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("    ({}/{})", state.items.len(), state.all.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    };
    f.render_widget(Paragraph::new(filter_text), vertical[0]);

    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(vertical[1]);
    let list_area = horiz[0];
    let preview_area = horiz[1];

    let preview_block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" preview ");
    let preview_inner = preview_block.inner(preview_area);
    f.render_widget(preview_block, preview_area);
    if let Some(t) = state.current() {
        let text = state.previews.get(&t.session_id).cloned().unwrap_or_else(|| "(loading...)".to_string());
        f.render_widget(
            Paragraph::new(text)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .style(Style::default().fg(Color::Gray)),
            preview_inner,
        );
    }

    let visible = list_area.height as usize;
    let total = state.items.len();
    let start = if state.selected >= visible {
        state.selected + 1 - visible
    } else { 0 };
    let end = (start + visible).min(total);

    let mut lines: Vec<Line> = Vec::with_capacity(end - start);
    if state.items.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no past sessions found in ~/.claude/projects)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let cwd_w = (list_area.width as usize).saturating_sub(32).max(15);
    for i in start..end {
        let Some(t) = state.items.get(i).and_then(|idx| state.all.get(*idx)) else { continue };
        let is_sel = i == state.selected;
        let prefix = if is_sel { "▶ " } else { "  " };
        let age = crate::transcripts::humanize_age(t.mtime);
        let cwd_str = t.cwd.display().to_string();
        let size_kb = t.file_size / 1024;
        let cwd_cell = pad_right(&truncate(&cwd_str, cwd_w), cwd_w);
        let label = format!(
            "{}{}  {:>8}  {:>7}KB  {}",
            prefix,
            cwd_cell,
            age,
            size_kb,
            &t.session_id[..8.min(t.session_id.len())],
        );
        let style = if is_sel {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(label, style)));
    }
    f.render_widget(Paragraph::new(lines), list_area);

    f.render_widget(
        Paragraph::new(Span::styled(
            " ─────────────────────",
            Style::default().fg(Color::DarkGray),
        )),
        vertical[2],
    );

    f.render_widget(Paragraph::new(dangerous_line(state.dangerous)), vertical[3]);
    f.render_widget(
        Paragraph::new(Span::styled(
            " ↑/↓ select  ·  type to filter  ·  Backspace clear  ·  Enter = resume  ·  Tab = toggle danger  ·  Esc cancel",
            Style::default().fg(Color::DarkGray),
        )),
        vertical[4],
    );
}

fn draw_rename_popup(f: &mut Frame, area: Rect, state: &crate::app::RenameState, app: &App) {
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
            "  Enter = save  ·  Esc = cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_dashboard(f: &mut Frame, app: &mut App, area: Rect, tile_sizes: &mut TileSizes) {
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

    if let Some(session) = app.sessions.get(app.focus) {
        let inner = draw_tile(f, session, main, true, false, app.focus + 1);
        tile_sizes.push((app.focus, inner.height, inner.width));
        app.last_tile_area = Some(inner);
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let block = titled_block(" sessions ", Color::Green);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.sessions.is_empty() {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  (empty)", Style::default().fg(Color::DarkGray))),
        ]);
        f.render_widget(hint, inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::with_capacity(app.sessions.len() * 2);
    for (i, s) in app.sessions.iter().enumerate() {
        let alive = s.alive.load(std::sync::atomic::Ordering::SeqCst);
        let age_ms = s.activity_age_ms();
        let (badge, badge_color) = if !alive {
            ("✕", Color::Red)
        } else if s.permission_pending {
            ("⚠", Color::LightRed)
        } else if s.claude_status == crate::session::ClaudeStatus::Busy || age_ms < 1500 {
            ("●", Color::Green)
        } else if s.claude_status == crate::session::ClaudeStatus::Idle {
            ("○", Color::Cyan)
        } else if age_ms < 30_000 {
            ("○", Color::Yellow)
        } else {
            ("·", Color::DarkGray)
        };
        let focused = i == app.focus;
        let marker = if focused { "▶ " } else { "  " };
        let danger = if s.dangerous { "⚠" } else { " " };
        let state = if !alive { " (exited)" } else { "" };
        let resume_tag = if s.resume_id.is_some() { "↺" } else { " " };
        let line_style = if !alive {
            Style::default().fg(Color::Red).add_modifier(Modifier::DIM)
        } else if focused {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{} ", badge), Style::default().fg(badge_color).add_modifier(Modifier::BOLD)),
            Span::styled(format!("[{}]{}{} ", i + 1, resume_tag, danger), line_style),
            Span::styled(format!("{}{}", s.label, state), line_style),
        ]));
        lines.push(Line::from(Span::styled(
            format!("      {}", truncate(&s.cwd.display().to_string(), inner.width.saturating_sub(8) as usize)),
            Style::default().fg(Color::DarkGray),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let n = s.chars().count();
    let keep = max.saturating_sub(1);
    let skip = n - keep;
    let tail: String = s.chars().skip(skip).collect();
    format!("…{}", tail)
}

fn pad_right(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (width - n));
    out.push_str(s);
    for _ in 0..(width - n) {
        out.push(' ');
    }
    out
}

fn draw_tile(
    f: &mut Frame,
    session: &Session,
    area: Rect,
    focused: bool,
    zoomed: bool,
    display_num: usize,
) -> Rect {
    let alive = session.alive.load(std::sync::atomic::Ordering::SeqCst);
    let style = if !alive {
        Style::default().fg(Color::Red).add_modifier(Modifier::DIM)
    } else if zoomed {
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let danger = if session.dangerous { " ⚠ " } else { " " };
    let zoom_marker = if zoomed { "↕ " } else { "" };
    let state = if !alive { " EXITED" } else { "" };
    let title = format!(" {}[{}]{}{}{}  ", zoom_marker, display_num, danger, session.label, state);
    let block = Block::default().borders(Borders::ALL).border_style(style).title(title);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Ok(parser) = session.parser.lock() {
        let widget = TermWidget::new(&parser.term).with_selection(session.selection);
        f.render_widget(widget, inner);
    }
    inner
}

fn draw_spawn_popup(f: &mut Frame, area: Rect, spawn: &crate::app::SpawnState) {
    let w = area.width.saturating_sub(8).clamp(50, 90);
    let h = area.height.saturating_sub(4).clamp(14, 28);
    let inner = open_popup(f, area, w, h, " Spawn claude — pick a folder ", Color::Cyan);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let cwd_str = spawn.cwd.display().to_string();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" cwd: ", Style::default().fg(Color::DarkGray)),
            Span::styled(cwd_str, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ])),
        layout[0],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            " ─────────────────────",
            Style::default().fg(Color::DarkGray),
        )),
        layout[1],
    );

    let list_area = layout[2];
    let visible = list_area.height as usize;
    let total = spawn.entries.len();
    let start = if spawn.selected >= visible {
        spawn.selected + 1 - visible
    } else {
        0
    };
    let end = (start + visible).min(total);

    let mut lines: Vec<Line> = Vec::with_capacity(end - start);
    if spawn.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no subdirectories — press Enter to spawn here, or ← to go up)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for i in start..end {
        let name = spawn.entries[i]
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let is_sel = i == spawn.selected;
        let prefix = if is_sel { "▶ " } else { "  " };
        let style = if is_sel {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(format!("{}{}/", prefix, name), style)));
    }
    f.render_widget(Paragraph::new(lines), list_area);

    f.render_widget(Paragraph::new(dangerous_line(spawn.dangerous)), layout[3]);
    f.render_widget(
        Paragraph::new(Span::styled(
            " ─────────────────────",
            Style::default().fg(Color::DarkGray),
        )),
        layout[4],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            " ↑/↓ select  ·  → descend  ·  ← ascend  ·  Space danger  ·  Enter Pick  ·  Esc cancel",
            Style::default().fg(Color::DarkGray),
        )),
        layout[5],
    );
}

fn footer_for(app: &App) -> Line<'static> {
    let mode = &app.mode;
    let status = &app.status;
    let prefix_pending = app.prefix_pending;
    if prefix_pending {
        return Line::from(Span::styled(
            " PREFIX  n=new  ↑↓=cycle  d=detach  l=load  ?=more ".to_string(),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let (tag, rest, bg) = match mode {
        Mode::Dashboard => (" DASHBOARD ", format!(" {} ", status), Color::Green),
        Mode::Spawn(_) => (
            " SPAWN ",
            " Enter=pick  Esc=cancel  Space=toggle-danger  ↑↓ select  →/← descend/ascend ".to_string(),
            Color::Cyan,
        ),
        Mode::Rename(_) => (
            " RENAME ",
            " type new name  ·  Enter = save  ·  Esc = cancel ".to_string(),
            Color::Yellow,
        ),
        Mode::Picker(_) => (
            " RESUME ",
            " ↑↓ select  ·  type to filter  ·  Enter = resume  ·  Space = toggle danger  ·  Esc = cancel ".to_string(),
            Color::Magenta,
        ),
        Mode::ConfirmDetach(_) => (
            " CONFIRM ",
            " y = detach  ·  n/Esc = cancel ".to_string(),
            Color::Red,
        ),
        Mode::Help => (
            " HELP ",
            " press any key to close ".to_string(),
            Color::Yellow,
        ),
        Mode::Reorder => (
            " REORDER ",
            " ↑/↓ move focused session  ·  Esc/Enter/q = exit ".to_string(),
            Color::Magenta,
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
                    " offset={}  ·  ↑/↓ line  ·  PgUp/PgDn page  ·  g top  ·  G bottom  ·  q/Esc exit ",
                    offset
                ),
                Color::Blue,
            )
        }
    };
    Line::from(vec![
        Span::styled(
            tag.to_string(),
            Style::default().fg(Color::Black).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(rest, Style::default().fg(Color::White).bg(Color::DarkGray)),
    ])
}
