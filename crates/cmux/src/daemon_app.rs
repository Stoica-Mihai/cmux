//! Daemon-backed single-session TUI mode (phase 4 MVP).
//!
//! `cmux --connect` enters this mode: it spawns one session on the daemon,
//! subscribes to FrameDelta events, feeds bytes into a local
//! `alacritty_terminal::Term`, and forwards keystrokes/resize back as
//! `Request::Input` / `Request::Resize`. Mouse selection + OSC 52 stay
//! client-side, just like in the in-process path.
//!
//! Multi-session through the daemon and sidebar parity ship in phase 5.

use std::io::stdout;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::vte::ansi::Processor;
use anyhow::{Context, Result};
use cmux_proto::{Event, Request};
use crossterm::event::{
    self, Event as CtEvent, KeyEventKind, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::client::Client;
use crate::term_render::{TermSize, TermWidget, TileSelection};
use crate::theme;

const SCROLLBACK_LINES: usize = 4096;

pub fn run(socket: &std::path::Path, cwd: PathBuf) -> Result<()> {
    // 1. Connect, spawn one session
    let mut c = Client::connect(socket).context("connect to cmuxd")?;
    c.send(&Request::SpawnSession {
        cwd: cwd.clone(),
        dangerous: false,
        resume_id: None,
        label: Some("cmux".into()),
    })?;
    let (session_id, label) = loop {
        match c.recv()? {
            Event::SessionSpawned { id, info } => break (id, info.label),
            Event::Error { message, .. } => anyhow::bail!("daemon: {message}"),
            other => eprintln!("cmux: unexpected pre-spawn event: {other:?}"),
        }
    };

    // 2. Shared term + ring fed by event-reader thread
    let (rows0, cols0) = (24u16, 80u16);
    let term_state = Arc::new(Mutex::new(make_term_state(rows0, cols0)));
    let dirty = Arc::new(AtomicBool::new(true));
    let selection: Arc<Mutex<Option<TileSelection>>> = Arc::new(Mutex::new(None));
    let mouse_down_at: Arc<Mutex<Option<(u16, u16)>>> = Arc::new(Mutex::new(None));
    let should_quit = Arc::new(AtomicBool::new(false));

    // 3. Outbound request channel + writer
    let (req_tx, req_rx) = mpsc::channel::<Request>();
    let req_tx_for_handlers = req_tx.clone();

    // 4. Split into reader + writer threads. Reader blocks on recv() and
    //    pushes events into evt_tx; writer drains req_rx into send().
    let (evt_tx, evt_rx) = mpsc::channel::<Event>();
    let (mut creader, mut cwriter) = c.split().context("split client")?;
    cwriter
        .send(&Request::Subscribe { session_id })
        .context("subscribe")?;
    let dirty_r = dirty.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = creader.recv() {
            dirty_r.store(true, Ordering::Relaxed);
            if evt_tx.send(ev).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        while let Ok(req) = req_rx.recv() {
            if cwriter.send(&req).is_err() {
                break;
            }
        }
    });

    // 5. Set up terminal
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    {
        use std::io::Write;
        let mut out = stdout();
        let _ = out.write_all(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h");
        let _ = out.flush();
    }

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut last_size = (rows0, cols0);
    let mut tile_area: Option<Rect> = None;

    'tui: loop {
        // Drain inbound events (FrameDelta etc.)
        while let Ok(ev) = evt_rx.try_recv() {
            match ev {
                Event::FrameDelta { id, bytes } if id == session_id => {
                    if let Ok(mut t) = term_state.lock() {
                        let TerminalStateLocal { ref mut term, ref mut proc } = *t;
                        proc.advance(term, &bytes);
                    }
                }
                Event::Snapshot { id, .. } if id == session_id => {
                    // Phase 4 MVP: ignore — TUI started with a fresh Term and
                    // catches up via the daemon's replayed ring (delivered as
                    // a FrameDelta right after the Snapshot).
                }
                Event::SessionExited { id, .. } if id == session_id => {
                    break 'tui;
                }
                Event::Resync { id } if id == session_id => {
                    // Phase 4 MVP: clear local term, daemon will refeed bytes.
                    if let Ok(mut t) = term_state.lock() {
                        let (r, cl) = last_size;
                        *t = make_term_state(r, cl);
                    }
                }
                _ => {}
            }
        }

        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
                .split(area);
            let top = chunks[0];
            let body = chunks[1];
            let foot = chunks[2];

            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        " ◆ cmux ",
                        Style::default()
                            .fg(theme::ACCENT_MAGENTA)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("· daemon-backed · ", Style::default().fg(theme::FG_DIM)),
                    Span::styled(label.clone(), Style::default().fg(theme::FG)),
                ])),
                top,
            );

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::BORDER_FOCUS))
                .title(Span::styled(
                    format!(" [{}] {} ", session_id, label),
                    Style::default()
                        .fg(theme::BORDER_FOCUS)
                        .add_modifier(Modifier::BOLD),
                ));
            let inner = block.inner(body);
            f.render_widget(block, body);
            let content = Rect {
                x: inner.x.saturating_add(1),
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: inner.height,
            };
            tile_area = Some(content);

            if let Ok(t) = term_state.lock() {
                let sel = selection.lock().ok().and_then(|s| *s);
                f.render_widget(
                    TermWidget::new(&t.term)
                        .with_selection(sel)
                        .with_cursor_bg(theme::ACCENT_GREEN),
                    content,
                );
            }

            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        " DAEMON ",
                        Style::default()
                            .fg(Color::Rgb(0x0a, 0x0a, 0x0f))
                            .bg(theme::ACCENT_CYAN)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  Ctrl+Q quit", Style::default().fg(theme::FG_MUTED)),
                ])),
                foot,
            );

            // resize PTY when content area changed
            let (rows, cols) = (content.height, content.width);
            if rows >= 2 && cols >= 2 && (rows, cols) != last_size {
                last_size = (rows, cols);
                if let Ok(mut t) = term_state.lock() {
                    t.term.resize(TermSize {
                        lines: rows as usize,
                        cols: cols as usize,
                    });
                }
                let _ = req_tx_for_handlers.send(Request::Resize {
                    session_id,
                    rows,
                    cols,
                });
            }
        })?;

        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                CtEvent::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && matches!(key.code, crossterm::event::KeyCode::Char('q'))
                    {
                        break;
                    }
                    if let Some(bytes) = crate::keys::encode(key) {
                        let _ = req_tx_for_handlers.send(Request::Input {
                            session_id,
                            bytes,
                        });
                    }
                }
                CtEvent::Resize(_, _) => {
                    // body resize handled inside draw closure on next frame
                }
                CtEvent::Mouse(me) => {
                    let Some(tile) = tile_area else { continue };
                    let inside = me.column >= tile.x
                        && me.column < tile.x + tile.width
                        && me.row >= tile.y
                        && me.row < tile.y + tile.height;
                    handle_mouse(
                        me,
                        inside,
                        MouseCtx {
                            tile,
                            session_id,
                            term_state: &term_state,
                            selection: &selection,
                            mouse_down_at: &mouse_down_at,
                            req_tx: &req_tx_for_handlers,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    should_quit.store(true, Ordering::SeqCst);
    {
        use std::io::Write;
        let mut out = stdout();
        let _ = out.write_all(b"\x1b[?1006l\x1b[?1002l\x1b[?1000l");
    }
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    Ok(())
}

struct MouseCtx<'a> {
    tile: Rect,
    session_id: u64,
    term_state: &'a Arc<Mutex<TerminalStateLocal>>,
    selection: &'a Arc<Mutex<Option<TileSelection>>>,
    mouse_down_at: &'a Arc<Mutex<Option<(u16, u16)>>>,
    req_tx: &'a mpsc::Sender<Request>,
}

fn handle_mouse(me: crossterm::event::MouseEvent, inside: bool, ctx: MouseCtx<'_>) {
    let tile = ctx.tile;
    let session_id = ctx.session_id;
    let term_state = ctx.term_state;
    let selection = ctx.selection;
    let mouse_down_at = ctx.mouse_down_at;
    let req_tx = ctx.req_tx;
    use alacritty_terminal::term::TermMode;
    match me.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Ok(mut s) = selection.lock() {
                *s = None;
            }
            if inside {
                let row = me.row - tile.y;
                let col = me.column - tile.x;
                if let Ok(mut a) = mouse_down_at.lock() {
                    *a = Some((row, col));
                }
            } else if let Ok(mut a) = mouse_down_at.lock() {
                *a = None;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if inside => {
            if let Ok(a) = mouse_down_at.lock()
                && let Some(anchor) = *a
                && let Ok(mut s) = selection.lock()
            {
                let row = me.row - tile.y;
                let col = me.column - tile.x;
                let sel = s.get_or_insert_with(|| TileSelection::new(anchor.0, anchor.1));
                sel.anchor = anchor;
                sel.tip = (row, col);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Ok(mut a) = mouse_down_at.lock() {
                *a = None;
            }
            let sel = selection.lock().ok().and_then(|s| *s);
            if let Some(sel) = sel
                && let Ok(t) = term_state.lock()
            {
                let text = crate::term_render::extract_selection(&t.term, sel);
                drop(t);
                if !text.trim().is_empty() {
                    emit_osc52(&text);
                }
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if inside => {
            let up = matches!(me.kind, MouseEventKind::ScrollUp);
            let mode = term_state.lock().ok().map(|t| *t.term.mode());
            let col = me.column.saturating_sub(tile.x) + 1;
            let row = me.row.saturating_sub(tile.y) + 1;
            let bytes: Vec<u8> = match mode {
                Some(m) if m.intersects(TermMode::SGR_MOUSE) => {
                    let btn = if up { 64 } else { 65 };
                    format!("\x1b[<{};{};{}M", btn, col, row).into_bytes()
                }
                _ => {
                    if up { b"\x1b[5~".to_vec() } else { b"\x1b[6~".to_vec() }
                }
            };
            let _ = req_tx.send(Request::Input { session_id, bytes });
        }
        _ => {}
    }
}

fn emit_osc52(text: &str) {
    use std::io::Write;
    let encoded = base64_encode(text.as_bytes());
    let mut out = stdout().lock();
    let _ = write!(out, "\x1b]52;c;{}\x07", encoded);
    let _ = out.flush();
}

fn base64_encode(input: &[u8]) -> String {
    const A: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let b0 = input[i];
        let b1 = input[i + 1];
        let b2 = input[i + 2];
        out.push(A[(b0 >> 2) as usize] as char);
        out.push(A[((b0 & 0x03) << 4 | (b1 >> 4)) as usize] as char);
        out.push(A[((b1 & 0x0f) << 2 | (b2 >> 6)) as usize] as char);
        out.push(A[(b2 & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let b0 = input[i];
        out.push(A[(b0 >> 2) as usize] as char);
        out.push(A[((b0 & 0x03) << 4) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b0 = input[i];
        let b1 = input[i + 1];
        out.push(A[(b0 >> 2) as usize] as char);
        out.push(A[((b0 & 0x03) << 4 | (b1 >> 4)) as usize] as char);
        out.push(A[((b1 & 0x0f) << 2) as usize] as char);
        out.push('=');
    }
    out
}

// Local mirror of cmux's term wrapper. Keeps daemon mode self-contained.
pub struct TerminalStateLocal {
    pub term: Term<VoidListener>,
    pub proc: Processor,
}

fn make_term_state(rows: u16, cols: u16) -> TerminalStateLocal {
    let config = TermConfig {
        scrolling_history: SCROLLBACK_LINES,
        ..Default::default()
    };
    let size = TermSize {
        lines: rows.max(1) as usize,
        cols: cols.max(1) as usize,
    };
    let _ = <TermSize as Dimensions>::screen_lines(&size); // silence unused-import
    TerminalStateLocal {
        term: Term::new(config, &size, VoidListener),
        proc: Processor::new(),
    }
}
