use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, Mode};
use crate::session::{ClaudeStatus, Session};
use crate::term_render::TermWidget;
use crate::theme;

pub type TileSizes = Vec<(usize, u16, u16)>;

pub fn draw(f: &mut Frame, app: &mut App, tile_sizes: &mut TileSizes) {
    tile_sizes.clear();
    let area = f.area();
    app.last_tile_area = None;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let titlebar = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    draw_titlebar(f, app, titlebar);
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

    if let (Some(tile), Some(toast)) = (app.last_tile_area, &app.toast) {
        draw_toast(f, tile, &toast.text);
    }

    if app.daemon_lost {
        draw_daemon_lost(f, area);
    }
}

fn draw_daemon_lost(f: &mut Frame, area: Rect) {
    // Dim the entire screen behind the modal with a wash of BG_ACTIVE so the
    // frozen tile + chrome read as "disabled".
    let dim_style = Style::default()
        .fg(theme::FG_DIM)
        .bg(Color::Rgb(0x11, 0x11, 0x18))
        .add_modifier(Modifier::DIM);
    f.render_widget(Block::default().style(dim_style), area);

    let w = area.width.saturating_sub(8).clamp(52, 72);
    let h: u16 = 11;
    let popup = centered_rect(area, w, h);

    // Drop-shadow effect: a 1-cell-offset darker block behind the popup.
    let shadow = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width,
        height: popup.height,
    };
    let shadow_clip_x = shadow.x.min(area.x + area.width.saturating_sub(1));
    let shadow_clip_w = (area.x + area.width).saturating_sub(shadow_clip_x);
    let shadow_clip = Rect {
        x: shadow_clip_x,
        y: shadow.y,
        width: shadow_clip_w.min(shadow.width),
        height: shadow.height.min(area.height.saturating_sub(shadow.y - area.y)),
    };
    f.render_widget(Clear, shadow_clip);
    f.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(0x05, 0x05, 0x09))),
        shadow_clip,
    );

    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(
            Style::default()
                .fg(theme::BORDER_DEAD)
                .add_modifier(Modifier::BOLD),
        )
        .title(Span::styled(
            "  Daemon disconnected  ",
            Style::default()
                .fg(theme::BORDER_DEAD)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Rgb(0x18, 0x14, 0x18)));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let center = ratatui::layout::Alignment::Center;
    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "✕  cmuxd is no longer responding",
            Style::default()
                .fg(theme::FG)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(center),
        Line::from(""),
        Line::from(Span::styled(
            "Sessions are retained on disk.",
            Style::default().fg(theme::FG_MUTED),
        ))
        .alignment(center),
        Line::from(Span::styled(
            "Restart cmuxd and reconnect to resume them.",
            Style::default().fg(theme::FG_MUTED),
        ))
        .alignment(center),
        Line::from(""),
        Line::from({
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.extend(kbd_chip("any key"));
            spans.push(Span::styled(
                "  to dismiss",
                Style::default().fg(theme::FG_DIM),
            ));
            spans
        })
        .alignment(center),
    ];
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(Color::Rgb(0x18, 0x14, 0x18))),
        inner,
    );
}

fn draw_toast(f: &mut Frame, tile: Rect, text: &str) {
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

fn draw_titlebar(f: &mut Frame, app: &App, area: Rect) {
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
    let mut left_spans = vec![
        Span::styled(
            " ◆ cmux ",
            Style::default().fg(brand_color).add_modifier(Modifier::BOLD),
        ),
    ];
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
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            title.into(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
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


fn draw_help_popup(f: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(4).clamp(64, 80);
    let h = area.height.saturating_sub(2).clamp(22, 30);
    let inner = open_popup(f, area, w, h, " ⌘ cmux — cheat sheet ", theme::ACCENT_YELLOW);

    let header = |s: &'static str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default()
                .fg(theme::ACCENT_YELLOW)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |chord: &str, desc: &str| {
        let chord_chip = kbd_chip(chord);
        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        spans.extend(chord_chip);
        let chord_width: usize = chord.chars().count() + 4;
        let pad = 12usize.saturating_sub(chord_width);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(
            format!(" {}", desc),
            Style::default().fg(theme::FG),
        ));
        Line::from(spans)
    };
    let note = |s: &str| {
        Line::from(Span::styled(
            format!("  {}", s),
            Style::default().fg(theme::FG_DIM),
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
        row("↑↓", "cycle focused session"),
        row("1-9", "jump to session N"),
        row("z", "toggle sidebar"),
        row("a", "send literal Ctrl+A to focused claude"),
        row("?", "this help"),
        row("q", "quit (kills all sessions)"),
        Line::from(""),
        header(" Global"),
        row("Ctrl+Q", "hard quit from anywhere"),
        Line::from(""),
        header(" Mouse"),
        row("drag", "copy selection via OSC 52"),
        row("⇧+drag", "bypass cmux, outer terminal selection"),
        Line::from(""),
        header(" Sidebar badges"),
        note("⠋ green  busy (claude working)"),
        note("○ cyan   idle (claude waiting for input)"),
        note("● red    permission prompt waiting"),
        note("· gray   dormant"),
        note("✕ red    session exited"),
        note("↺ cyan   resumed session"),
        Line::from(""),
        note("press any key to close"),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm_detach(f: &mut Frame, area: Rect, id: u64, app: &App) {
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
        .alignment(ratatui::layout::Alignment::Center),
        Line::from(Span::styled(
            "Running claude process will be killed.",
            Style::default().fg(theme::FG_DIM),
        ))
        .alignment(ratatui::layout::Alignment::Center),
        Line::from(""),
        Line::from({
            let mut spans = action_chip("y", "detach", theme::ACCENT_GREEN);
            spans.push(Span::raw("   "));
            spans.extend(action_chip("n", "cancel", theme::FG_DIM));
            spans
        })
        .alignment(ratatui::layout::Alignment::Center),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn action_chip(key: &str, label: &str, color: Color) -> Vec<Span<'static>> {
    let dark = Color::Rgb(0x0a, 0x0a, 0x0f);
    vec![
        Span::styled("\u{E0B6}", Style::default().fg(color)),
        Span::styled(
            format!(" {} ", key),
            Style::default()
                .fg(dark)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} ", label),
            Style::default()
                .fg(dark)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{E0B4}", Style::default().fg(color)),
    ]
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
            Constraint::Length(1),  // filter
            Constraint::Min(3),     // list + preview
            Constraint::Length(1),  // gap
            Constraint::Length(3),  // dangerous panel
            Constraint::Length(1),  // hint
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
    let mut row_y = list_area.y;
    for i in start..end {
        if row_y >= list_area.y + list_area.height {
            break;
        }
        let Some(t) = state.items.get(i).and_then(|idx| state.all.get(*idx)) else { continue };
        let is_sel = i == state.selected;
        let row_rect = Rect {
            x: list_area.x,
            y: row_y,
            width: list_area.width,
            height: 1,
        };
        row_y += 1;

        if is_sel {
            let bg = Block::default().style(Style::default().bg(theme::BG_ACTIVE));
            f.render_widget(bg, row_rect);
            let strip = Rect { width: 1, ..row_rect };
            let strip_style = Style::default()
                .fg(theme::ACCENT_MAGENTA)
                .bg(theme::BG_ACTIVE);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("▎", strip_style))),
                strip,
            );
        }

        let text_rect = Rect {
            x: row_rect.x + 2,
            y: row_rect.y,
            width: row_rect.width.saturating_sub(2),
            height: 1,
        };
        let age = crate::transcripts::humanize_age(t.mtime);
        let cwd_str = collapse_cwd(&t.cwd.display().to_string());
        let size_kb = t.file_size / 1024;
        let cwd_cell = pad_right(&truncate(&cwd_str, cwd_w), cwd_w);
        let label = format!(
            "{}  {:>8}  {:>7}KB  {}",
            cwd_cell,
            age,
            size_kb,
            &t.session_id[..8.min(t.session_id.len())],
        );
        let fg = if is_sel {
            theme::BORDER_FOCUS
        } else {
            theme::FG
        };
        let style = if is_sel {
            Style::default()
                .fg(fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg)
        };
        f.render_widget(Paragraph::new(Line::from(Span::styled(label, style))), text_rect);
    }
    let _ = lines;

    f.render_widget(
        Paragraph::new(Span::styled(
            " ─────────────────────",
            Style::default().fg(Color::DarkGray),
        )),
        vertical[2],
    );

    draw_dangerous_panel(f, vertical[3], state.dangerous);
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
            Line::from(Span::styled("  (empty)", Style::default().fg(Color::DarkGray))),
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

        let alive = s.alive.load(std::sync::atomic::Ordering::SeqCst);
        let age_ms = s.activity_age_ms();
        let focused = i == app.focus;

        let busy = s.claude_status == ClaudeStatus::Busy || age_ms < 1500;
        let (badge_glyph, badge_color): (String, Color) = if !alive {
            ("✕".into(), theme::ACCENT_RED)
        } else if s.permission_pending {
            ("⚠".into(), theme::ACCENT_RED)
        } else if busy {
            (
                theme::spinner_frame(app.render_tick).to_string(),
                theme::ACCENT_GREEN,
            )
        } else if s.claude_status == ClaudeStatus::Idle {
            ("○".into(), theme::ACCENT_CYAN)
        } else if age_ms < 30_000 {
            ("○".into(), theme::ACCENT_YELLOW)
        } else {
            ("·".into(), theme::FG_DIM)
        };

        let danger = if s.dangerous { "⚠" } else { " " };
        let state_suffix = if !alive { " (exited)" } else { "" };
        let resume_tag = if s.resume_id.is_some() { "↺" } else { " " };

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
        let num_style = Style::default().fg(theme::FG_MUTED);

        if focused {
            let bg = Block::default().style(Style::default().bg(theme::BG_ACTIVE));
            f.render_widget(bg, row_area);
            let strip_area = Rect {
                x: row_area.x,
                y: row_area.y,
                width: 1,
                height: row_area.height,
            };
            let strip_style = Style::default()
                .fg(theme::BORDER_FOCUS)
                .bg(theme::BG_ACTIVE);
            let strip_lines: Vec<Line> = (0..row_area.height)
                .map(|_| Line::from(Span::styled("▎", strip_style)))
                .collect();
            f.render_widget(Paragraph::new(strip_lines), strip_area);
        }

        let text_area = Rect {
            x: row_area.x + 2,
            y: row_area.y,
            width: row_area.width.saturating_sub(2),
            height: row_area.height,
        };
        let avail_width = text_area.width as usize;

        let mut header: Vec<Span> = vec![
            Span::styled(
                format!("{} ", badge_glyph),
                Style::default()
                    .fg(badge_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("[{}]", i + 1), num_style),
        ];
        if s.permission_pending {
            header.push(Span::styled(
                "●",
                Style::default()
                    .fg(theme::ACCENT_RED)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            header.push(Span::raw(" "));
        }
        header.push(Span::styled(
            resume_tag.to_string(),
            Style::default().fg(theme::ACCENT_CYAN),
        ));
        header.push(Span::styled(
            danger.to_string(),
            Style::default().fg(theme::ACCENT_RED),
        ));
        header.push(Span::raw(" "));
        header.push(Span::styled(
            format!("{}{}", s.label, state_suffix),
            label_style,
        ));

        let cwd_str = collapse_cwd(&s.cwd.display().to_string());
        let lines: Vec<Line> = vec![
            Line::from(header),
            Line::from(Span::styled(
                format!("    {}", truncate(&cwd_str, avail_width.saturating_sub(4))),
                Style::default().fg(theme::FG_DIM),
            )),
            Line::from(Span::styled(
                format!(
                    "    {}",
                    sidebar_meta(s, age_ms, avail_width.saturating_sub(4))
                ),
                Style::default().fg(theme::FG_MUTED),
            )),
        ];
        f.render_widget(Paragraph::new(lines), text_area);
    }
}

fn collapse_cwd(p: &str) -> String {
    let home = std::env::var_os("HOME").map(|h| h.to_string_lossy().into_owned());
    let mut s = match &home {
        Some(h) if p.starts_with(h.as_str()) => format!("~{}", &p[h.len()..]),
        _ => p.to_string(),
    };
    // collapse middle segments if path is deep
    let segs: Vec<&str> = s.split('/').collect();
    if segs.len() > 5 {
        let head = segs[..2].join("/");
        let tail = segs[segs.len() - 2..].join("/");
        s = format!("{}/…/{}", head, tail);
    }
    s
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
    render_tick: u64,
) -> Rect {
    let alive = session.alive.load(std::sync::atomic::Ordering::SeqCst);
    let pulse_on = render_tick.is_multiple_of(2);
    let border_color = if !alive {
        theme::BORDER_DEAD
    } else if session.permission_pending {
        if pulse_on {
            theme::BORDER_DEAD
        } else {
            theme::ACCENT_RED_DIM
        }
    } else if zoomed {
        theme::ACCENT_MAGENTA
    } else if focused {
        theme::BORDER_FOCUS
    } else {
        theme::BORDER_IDLE
    };
    let title_color = border_color;
    let danger = if session.dangerous { " ⚠ " } else { " " };
    let zoom_marker = if zoomed { "↕ " } else { "" };
    let state = if !alive { " EXITED" } else { "" };
    let title = format!(
        " {}[{}]{}{}{} ",
        zoom_marker, display_num, danger, session.label, state
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(title_color)
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

    let cursor_bg = if session.permission_pending {
        theme::ACCENT_RED
    } else {
        match session.claude_status {
            ClaudeStatus::Busy => theme::ACCENT_GREEN,
            ClaudeStatus::Idle => theme::ACCENT_CYAN,
            ClaudeStatus::Unknown => theme::FG,
        }
    };

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

fn draw_spawn_popup(f: &mut Frame, area: Rect, spawn: &crate::app::SpawnState) {
    let w = area.width.saturating_sub(8).clamp(50, 90);
    let h = area.height.saturating_sub(4).clamp(14, 28);
    let inner = open_popup(f, area, w, h, " Spawn claude — pick a folder ", theme::ACCENT_CYAN);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // cwd
            Constraint::Length(1),  // separator
            Constraint::Min(3),     // list
            Constraint::Length(1),  // gap
            Constraint::Length(3),  // dangerous toggle (3 rows w/ vertical centering)
            Constraint::Length(1),  // gap
            Constraint::Length(1),  // hint
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

    let list_area = layout[2];
    let visible = list_area.height as usize;
    let total = spawn.entries.len();
    let start = if spawn.selected >= visible {
        spawn.selected + 1 - visible
    } else {
        0
    };
    let end = (start + visible).min(total);

    if spawn.entries.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  (no subdirectories — press Enter to spawn here, or ← to go up)",
                Style::default().fg(theme::FG_DIM),
            ))),
            list_area,
        );
    }
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
        let is_sel = i == spawn.selected;

        if is_sel {
            let bg = Block::default().style(Style::default().bg(theme::BG_ACTIVE));
            f.render_widget(bg, row_rect);
            let strip = Rect { width: 1, ..row_rect };
            let strip_style = Style::default()
                .fg(theme::ACCENT_CYAN)
                .bg(theme::BG_ACTIVE);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("▎", strip_style))),
                strip,
            );
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

    draw_dangerous_panel(f, layout[4], spawn.dangerous);

    let pairs = [
        ("↑↓", "select"),
        ("→", "descend"),
        ("←", "ascend"),
        ("Space", "danger"),
        ("Enter", "pick"),
        ("Esc", "cancel"),
    ];
    // pre-compute total content width to evenly distribute remaining space as gaps
    let content_width: usize = pairs
        .iter()
        .map(|(k, l)| k.chars().count() + 2 + 1 + l.chars().count())
        .sum::<usize>();
    let avail = layout[4].width as usize;
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
    f.render_widget(Paragraph::new(Line::from(hint)), layout[6]);
}

fn draw_dangerous_panel(f: &mut Frame, area: Rect, active: bool) {
    let (status_color, status_text, label_color, label_mod) = if active {
        (
            theme::ACCENT_RED,
            "● ON",
            theme::ACCENT_RED,
            Modifier::BOLD,
        )
    } else {
        (theme::FG_DIM, "○ OFF", theme::FG, Modifier::empty())
    };

    let panel_bg = if active {
        Color::Rgb(0x33, 0x1a, 0x22)
    } else {
        theme::BG_ACTIVE
    };
    f.render_widget(
        Block::default().style(Style::default().bg(panel_bg)),
        area,
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(12),
            Constraint::Min(20),
            Constraint::Length(20),
        ])
        .split(area);

    let mid_row = area.y + area.height / 2;

    let status_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            status_text.to_string(),
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let label_line = Line::from(Span::styled(
        "--dangerously-skip-permissions".to_string(),
        Style::default().fg(label_color).add_modifier(label_mod),
    ))
    .alignment(ratatui::layout::Alignment::Center);
    let key_line = {
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.extend(kbd_chip("Space"));
        spans.push(Span::styled(
            " toggles ",
            Style::default().fg(theme::FG_DIM),
        ));
        Line::from(spans).alignment(ratatui::layout::Alignment::Right)
    };

    let single = Rect {
        x: cols[0].x,
        y: mid_row,
        width: cols[0].width,
        height: 1,
    };
    f.render_widget(Paragraph::new(status_line).style(Style::default().bg(panel_bg)), single);

    let single2 = Rect {
        x: cols[1].x,
        y: mid_row,
        width: cols[1].width,
        height: 1,
    };
    f.render_widget(Paragraph::new(label_line).style(Style::default().bg(panel_bg)), single2);

    let single3 = Rect {
        x: cols[2].x,
        y: mid_row,
        width: cols[2].width,
        height: 1,
    };
    f.render_widget(Paragraph::new(key_line).style(Style::default().bg(panel_bg)), single3);
}

fn footer_for(app: &App) -> Line<'static> {
    let mode = &app.mode;
    let status = &app.status;
    let prefix_pending = app.prefix_pending;
    if prefix_pending {
        let mut spans = chip(" PREFIX ", theme::ACCENT_YELLOW);
        spans.push(Span::styled(
            "  n=new · ↑↓=cycle · d=detach · l=load · ? more ".to_string(),
            Style::default().fg(theme::FG),
        ));
        return Line::from(spans);
    }
    if matches!(mode, Mode::Dashboard) {
        let mut spans = chip(" DASHBOARD ", theme::ACCENT_GREEN);
        spans.push(Span::raw("  "));
        spans.extend(kbd_chip("Ctrl+A"));
        spans.push(Span::styled(
            "  then  n=new · l=load · ↑↓=cycle · 1-9=jump · r=rename · d=detach · z=sidebar · q=quit".to_string(),
            Style::default().fg(theme::FG_MUTED),
        ));
        if !status.is_empty() {
            spans.push(Span::styled(
                format!("  ·  {}", status),
                Style::default().fg(theme::ACCENT_YELLOW),
            ));
        }
        return Line::from(spans);
    }
    let (tag, rest, bg) = match mode {
        Mode::Dashboard => unreachable!(),
        Mode::Spawn(_) => (
            " SPAWN ",
            "  Enter pick · Esc cancel · Space danger · ↑↓ select · →/← descend/ascend"
                .to_string(),
            theme::ACCENT_CYAN,
        ),
        Mode::Rename(_) => (
            " RENAME ",
            "  type new name · Enter save · Esc cancel".to_string(),
            theme::ACCENT_YELLOW,
        ),
        Mode::Picker(_) => (
            " RESUME ",
            "  ↑↓ select · type to filter · Enter resume · Tab toggle danger · Esc cancel"
                .to_string(),
            theme::ACCENT_MAGENTA,
        ),
        Mode::ConfirmDetach(_) => (
            " CONFIRM ",
            "  y detach · n/Esc cancel".to_string(),
            theme::ACCENT_RED,
        ),
        Mode::Help => (
            " HELP ",
            "  press any key to close".to_string(),
            theme::ACCENT_YELLOW,
        ),
        Mode::Reorder => (
            " REORDER ",
            "  ↑/↓ move focused session · Esc/Enter/q exit".to_string(),
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
                    "  offset={} · ↑/↓ line · PgUp/PgDn page · g top · G bottom · q/Esc exit",
                    offset
                ),
                theme::ACCENT_PEACH,
            )
        }
    };
    let mut spans = chip(tag, bg);
    spans.push(Span::styled(rest, Style::default().fg(theme::FG_MUTED)));
    Line::from(spans)
}

fn chip(label: &str, bg: Color) -> Vec<Span<'static>> {
    let fg = Color::Rgb(0x0a, 0x0a, 0x0f);
    vec![
        Span::styled("\u{E0B6}", Style::default().fg(bg)),
        Span::styled(
            label.trim().to_string(),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{E0B4}", Style::default().fg(bg)),
    ]
}

fn kbd_chip(label: &str) -> Vec<Span<'static>> {
    let bg = theme::BG_ACTIVE;
    let fg = theme::FG;
    vec![
        Span::styled("\u{E0B6}", Style::default().fg(bg)),
        Span::styled(
            label.to_string(),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{E0B4}", Style::default().fg(bg)),
    ]
}
