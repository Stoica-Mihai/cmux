mod app;
mod client;
mod keys;
mod persist;
mod session;
mod term_render;
mod theme;
mod transcripts;
mod ui;
#[macro_use]
mod util;

use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{App, Mode, PickerState, RenameState, SpawnState};

fn main() -> Result<()> {
    // Phase 3: --connect probes the daemon connection. Real attach/render
    // loop lands in phase 4. For now this just verifies the socket.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--connect") {
        let path = client::socket_path()
            .ok_or_else(|| anyhow::anyhow!("no $XDG_RUNTIME_DIR/$HOME for socket"))?;
        let mut c = client::Client::connect(&path)?;
        c.send(&cmux_proto::Request::ListSessions)?;
        match c.recv()? {
            cmux_proto::Event::SessionList { sessions } => {
                eprintln!("cmux: {} sessions on daemon", sessions.len());
                for s in sessions {
                    eprintln!("  [{}] {} ({})", s.id, s.label, s.cwd.display());
                }
            }
            other => eprintln!("cmux: unexpected event: {other:?}"),
        }
        return Ok(());
    }

    install_panic_hook();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    // Minimal mouse capture: ?1000h (button press/release) + ?1002h
    // (button-event tracking for drag) + ?1006h (SGR 1006 encoding).
    // Skip ?1003h (any-motion) — Windows Terminal amplifies wheel
    // events when 1003 is on, fanning a single detent into many
    // ScrollUp/Down events.
    {
        use std::io::Write;
        let mut out = stdout();
        let _ = out.write_all(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h");
        let _ = out.flush();
    }
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal);

    {
        use std::io::Write;
        let mut out = stdout();
        let _ = out.write_all(b"\x1b[?1006l\x1b[?1002l\x1b[?1000l");
        let _ = out.flush();
    }
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    res
}

fn run(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    let size = terminal.size()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut app = App::new(cwd, (size.height, size.width));

    let saved = persist::load();
    app.show_sidebar = saved.show_sidebar;
    for ps in saved.sessions {
        let res = if let Some(id) = ps.resume_id.clone() {
            app.spawn_resume(ps.cwd.clone(), ps.dangerous, id)
        } else {
            app.spawn_session(ps.cwd.clone(), ps.dangerous)
        };
        if res.is_ok()
            && let Some(s) = app.sessions.last_mut()
        {
            if !ps.label.is_empty() {
                s.label = ps.label;
            }
            s.manually_renamed = ps.manually_renamed;
        }
    }
    flush_persist(&app);

    let debug = util::debug_enabled();
    let mut tile_sizes: ui::TileSizes = Vec::new();
    let mut last_draw_ms: u64 = 0;
    let mut last_persist_ms: u64 = util::now_ms();
    const HEARTBEAT_MS: u64 = 250;
    const PERSIST_DEBOUNCE_MS: u64 = 2_000;
    loop {
        app.reap_dead();
        let now = util::now_ms();
        if app.persist_dirty && now.saturating_sub(last_persist_ms) >= PERSIST_DEBOUNCE_MS {
            flush_persist(&app);
            app.persist_dirty = false;
            last_persist_ms = now;
        }

        let any_session_dirty = app
            .sessions
            .iter()
            .any(|s| s.dirty.swap(false, std::sync::atomic::Ordering::Relaxed));
        let elapsed = now.saturating_sub(last_draw_ms);
        if let Some(t) = &app.toast
            && now >= t.expires_at_ms
        {
            app.toast = None;
            app.needs_redraw = true;
        }
        if app.needs_redraw || any_session_dirty || elapsed >= HEARTBEAT_MS {
            app.render_tick = app.render_tick.wrapping_add(1);
            terminal.draw(|f| ui::draw(f, &mut app, &mut tile_sizes))?;
            for (idx, rows, cols) in tile_sizes.drain(..) {
                if let Some(s) = app.sessions.get_mut(idx) {
                    let _ = s.resize(rows.max(2), cols.max(4));
                }
            }
            app.needs_redraw = false;
            last_draw_ms = now;
        }

        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) => {
                    if debug {
                        log_key(&key, app.prefix_pending);
                    }
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    handle_key(&mut app, key)?;
                    app.needs_redraw = true;
                }
                Event::Resize(cols, rows) => {
                    app.term_size = (rows, cols);
                    resize_all(&mut app);
                    app.needs_redraw = true;
                }
                Event::Mouse(me) => {
                    handle_mouse(&mut app, me);
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    if app.persist_dirty {
        flush_persist(&app);
    }
    Ok(())
}

fn apply_scroll_lines(app: &mut App, delta: i32) {
    use alacritty_terminal::grid::Scroll;
    let Mode::Scrollback(id) = app.mode else { return };
    let Some(s) = app.sessions.iter_mut().find(|s| s.id == id) else { return };
    let Some(sb) = s.scrollback.as_mut() else { return };
    sb.scroll(Scroll::Delta(delta));
    if sb.display_offset() == 0 && delta < 0 {
        s.scrollback = None;
        s.selection = None;
        app.mode = Mode::Dashboard;
    }
    app.needs_redraw = true;
}

fn handle_mouse(app: &mut App, me: MouseEvent) {
    let Some(tile) = app.last_tile_area else { return };
    let inside = me.column >= tile.x
        && me.column < tile.x + tile.width
        && me.row >= tile.y
        && me.row < tile.y + tile.height;

    match me.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(s) = app.sessions.get_mut(app.focus) {
                let had_selection = s.selection.is_some();
                s.selection = None;
                if inside {
                    let row = me.row - tile.y;
                    let col = me.column - tile.x;
                    s.mouse_down_at = Some((row, col));
                } else {
                    s.mouse_down_at = None;
                }
                if had_selection {
                    app.needs_redraw = true;
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if inside => {
            if let Some(s) = app.sessions.get_mut(app.focus)
                && let Some(anchor) = s.mouse_down_at
            {
                let row = me.row - tile.y;
                let col = me.column - tile.x;
                let sel = s.selection.get_or_insert_with(|| {
                    term_render::TileSelection::new(anchor.0, anchor.1)
                });
                sel.anchor = anchor;
                sel.tip = (row, col);
                app.needs_redraw = true;
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if inside => {
            use alacritty_terminal::term::TermMode;
            const WHEEL_REPEAT: usize = 1;
            let up = matches!(me.kind, MouseEventKind::ScrollUp);
            if matches!(app.mode, Mode::Scrollback(_)) {
                apply_scroll_lines(app, if up { 1 } else { -1 });
            } else if let Some(s) = app.sessions.get_mut(app.focus) {
                let mode = s.parser.lock().ok().map(|p| *p.term.mode());
                let col = me.column.saturating_sub(tile.x) + 1;
                let row = me.row.saturating_sub(tile.y) + 1;
                let one: Vec<u8> = match mode {
                    Some(m) if m.intersects(TermMode::SGR_MOUSE) => {
                        let btn = if up { 64 } else { 65 };
                        format!("\x1b[<{};{};{}M", btn, col, row).into_bytes()
                    }
                    Some(m)
                        if m.intersects(
                            TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION,
                        ) =>
                    {
                        let btn = if up { 64u8 } else { 65u8 };
                        vec![
                            0x1b,
                            b'[',
                            b'M',
                            btn + 32,
                            (col as u8).saturating_add(32),
                            (row as u8).saturating_add(32),
                        ]
                    }
                    Some(m)
                        if m.contains(TermMode::ALT_SCREEN)
                            && m.contains(TermMode::ALTERNATE_SCROLL) =>
                    {
                        if up { b"\x1b[A".to_vec() } else { b"\x1b[B".to_vec() }
                    }
                    _ => {
                        if up { b"\x1b[5~".to_vec() } else { b"\x1b[6~".to_vec() }
                    }
                };
                let mut buf: Vec<u8> = Vec::with_capacity(one.len() * WHEEL_REPEAT);
                for _ in 0..WHEEL_REPEAT {
                    buf.extend_from_slice(&one);
                }
                let _ = s.write(&buf);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some(s) = app.sessions.get_mut(app.focus) {
                s.mouse_down_at = None;
            }
            let in_scrollback = matches!(app.mode, Mode::Scrollback(_));
            let Some(s) = app.sessions.get(app.focus) else { return };
            let Some(sel) = s.selection else { return };
            let text = if in_scrollback
                && let Some(sb) = &s.scrollback
            {
                term_render::extract_selection(&sb.term, sel)
            } else if let Ok(p) = s.parser.lock() {
                term_render::extract_selection(&p.term, sel)
            } else {
                return;
            };
            if !text.trim().is_empty() {
                let count = text.chars().count();
                emit_osc52(&text);
                app.toast = Some(app::Toast {
                    text: format!("copied ✓ {} chars", count),
                    expires_at_ms: util::now_ms() + 1400,
                });
                app.needs_redraw = true;
            }
        }
        _ => {}
    }
}

fn emit_osc52(text: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let encoded = base64_encode(text.as_bytes());
    let _ = write!(stdout, "\x1b]52;c;{}\x07", encoded);
    let _ = stdout.flush();
}

fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let b0 = input[i];
        let b1 = input[i + 1];
        let b2 = input[i + 2];
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[((b0 & 0x03) << 4 | (b1 >> 4)) as usize] as char);
        out.push(ALPHABET[((b1 & 0x0f) << 2 | (b2 >> 6)) as usize] as char);
        out.push(ALPHABET[(b2 & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let b0 = input[i];
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[((b0 & 0x03) << 4) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b0 = input[i];
        let b1 = input[i + 1];
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[((b0 & 0x03) << 4 | (b1 >> 4)) as usize] as char);
        out.push(ALPHABET[((b1 & 0x0f) << 2) as usize] as char);
        out.push('=');
    }
    out
}

fn log_key(key: &KeyEvent, prefix_pending: bool) {
    debug_log!(
        "/tmp/cmux-keys.log",
        "code={:?} mods={:?} kind={:?} prefix_pending={}",
        key.code,
        key.modifiers,
        key.kind,
        prefix_pending
    );
}

fn resize_all(app: &mut App) {
    let (term_rows, term_cols) = app.term_size;
    let sidebar_w: u16 = if app.show_sidebar { 32 } else { 0 };
    let main_cols = term_cols.saturating_sub(sidebar_w).saturating_sub(2).max(10);
    let main_rows = term_rows.saturating_sub(3).max(4);
    if let Some(s) = app.sessions.get_mut(app.focus) {
        let _ = s.resize(main_rows, main_cols);
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('q')) {
        app.should_quit = true;
        return Ok(());
    }

    let is_prefix_key = (key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('a') | KeyCode::Char('A')))
        || matches!(key.code, KeyCode::Char('\u{01}'));

    if app.prefix_pending {
        app.prefix_pending = false;
        return handle_prefix_chord(app, key);
    }

    if is_prefix_key {
        app.prefix_pending = true;
        return Ok(());
    }

    let mode_taken = std::mem::replace(&mut app.mode, Mode::Dashboard);
    match mode_taken {
        Mode::Dashboard => handle_dashboard(app, key)?,
        Mode::Spawn(state) => handle_spawn(app, state, key)?,
        Mode::Rename(state) => handle_rename(app, state, key)?,
        Mode::Picker(state) => handle_picker(app, state, key)?,
        Mode::ConfirmDetach(id) => handle_confirm_detach(app, id, key)?,
        Mode::Scrollback(id) => handle_scrollback(app, id, key)?,
        Mode::Help => {
            app.mode = Mode::Dashboard;
        }
        Mode::Reorder => handle_reorder(app, key)?,
    }
    Ok(())
}

fn handle_reorder(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            move_focused(app, -1);
            app.persist_dirty = true;
            app.mode = Mode::Reorder;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            move_focused(app, 1);
            app.persist_dirty = true;
            app.mode = Mode::Reorder;
        }
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
            app.mode = Mode::Dashboard;
        }
        _ => {
            app.mode = Mode::Reorder;
        }
    }
    Ok(())
}

fn handle_scrollback(app: &mut App, id: u64, key: KeyEvent) -> Result<()> {
    use alacritty_terminal::grid::Scroll;
    let Some(s) = app.sessions.iter_mut().find(|s| s.id == id) else {
        app.mode = Mode::Dashboard;
        return Ok(());
    };

    let exit = matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q'));
    if exit {
        s.scrollback = None;
        s.selection = None;
        app.mode = Mode::Dashboard;
        return Ok(());
    }

    let scroll = match key.code {
        KeyCode::Up | KeyCode::Char('k') => Scroll::Delta(1),
        KeyCode::Down | KeyCode::Char('j') => Scroll::Delta(-1),
        KeyCode::PageUp | KeyCode::Char('b') => Scroll::PageUp,
        KeyCode::PageDown | KeyCode::Char('f') | KeyCode::Char(' ') => Scroll::PageDown,
        KeyCode::Home | KeyCode::Char('g') => Scroll::Top,
        KeyCode::End | KeyCode::Char('G') => Scroll::Bottom,
        _ => {
            app.mode = Mode::Scrollback(id);
            return Ok(());
        }
    };

    if let Some(sb) = s.scrollback.as_mut() {
        sb.scroll(scroll);
    }
    app.mode = Mode::Scrollback(id);
    Ok(())
}

fn handle_confirm_detach(app: &mut App, id: u64, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            if let Some(idx) = app.sessions.iter().position(|s| s.id == id) {
                app.focus = idx;
                app.detach_focused();
                app.persist_dirty = true;
            }
            app.mode = Mode::Dashboard;
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.mode = Mode::Dashboard;
        }
        _ => {
            app.mode = Mode::ConfirmDetach(id);
        }
    }
    Ok(())
}

fn handle_prefix_chord(app: &mut App, key: KeyEvent) -> Result<()> {
    let mode_before = format!("{:?}", std::mem::discriminant(&app.mode));
    if matches!(key.code, KeyCode::Down) {
        app.cycle_focus(1);
        return Ok(());
    }
    if matches!(key.code, KeyCode::Up) {
        app.cycle_focus(-1);
        return Ok(());
    }
    let ch = match key.code {
        KeyCode::Char(c) => c.to_ascii_lowercase(),
        _ => return Ok(()),
    };
    match ch {
        'q' => app.should_quit = true,
        'n' => {
            app.mode = Mode::Spawn(SpawnState::new(app.default_cwd.clone()));
        }
        'd' => {
            if let Some(s) = app.sessions.get(app.focus) {
                app.mode = Mode::ConfirmDetach(s.id);
            }
        }
        'z' => {
            app.show_sidebar = !app.show_sidebar;
            resize_all(app);
            app.persist_dirty = true;
        }
        'r' => {
            if let Some(s) = app.sessions.get(app.focus) {
                app.mode = Mode::Rename(RenameState {
                    session_id: s.id,
                    buf: s.label.clone(),
                });
            }
        }
        'l' => {
            app.mode = Mode::Picker(PickerState::new());
        }
        'a' => {
            if let Some(s) = app.sessions.get_mut(app.focus) {
                let _ = s.write(&[0x01]);
            }
        }
        '[' => {
            if let Some(s) = app.sessions.get_mut(app.focus) {
                let id = s.id;
                let (rows, cols) = s.size;
                let bytes: Vec<u8> = s
                    .byte_ring
                    .lock()
                    .map(|r| r.iter().copied().collect())
                    .unwrap_or_default();
                s.scrollback = Some(session::build_scrollback(rows, cols, &bytes));
                app.mode = Mode::Scrollback(id);
            }
        }
        '?' => {
            app.mode = Mode::Help;
        }
        'm' if !app.sessions.is_empty() => {
            app.mode = Mode::Reorder;
        }
        c if c.is_ascii_digit() => {
            let idx = if c == '0' { 9 } else { (c as u8 - b'1') as usize };
            if idx < app.sessions.len() {
                app.focus = idx;
                resize_all(app);
            }
        }
        _ => {}
    }
    if util::debug_enabled() {
        let mode_after = format!("{:?}", std::mem::discriminant(&app.mode));
        debug_log!(
            "/tmp/cmux-keys.log",
            "  CHORD {} mode_before={} mode_after={} sessions={} focus={}",
            ch,
            mode_before,
            mode_after,
            app.sessions.len(),
            app.focus
        );
    }
    Ok(())
}

fn handle_dashboard(app: &mut App, key: KeyEvent) -> Result<()> {
    if let Some(s) = app.sessions.get_mut(app.focus)
        && let Some(bytes) = keys::encode(key)
    {
        let _ = s.write(&bytes);
    }
    Ok(())
}

fn handle_picker(app: &mut App, mut state: PickerState, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Dashboard;
        }
        KeyCode::Up => {
            state.move_sel(-1);
            app.mode = Mode::Picker(state);
        }
        KeyCode::Down => {
            state.move_sel(1);
            app.mode = Mode::Picker(state);
        }
        KeyCode::PageUp => { state.move_sel(-10); app.mode = Mode::Picker(state); }
        KeyCode::PageDown => { state.move_sel(10); app.mode = Mode::Picker(state); }
        KeyCode::Home => { state.selected = 0; state.ensure_preview(); app.mode = Mode::Picker(state); }
        KeyCode::End => {
            state.selected = state.items.len().saturating_sub(1);
            state.ensure_preview();
            app.mode = Mode::Picker(state);
        }
        KeyCode::Tab => {
            state.dangerous = !state.dangerous;
            app.mode = Mode::Picker(state);
        }
        KeyCode::Backspace => {
            state.filter.pop();
            state.apply_filter();
            app.mode = Mode::Picker(state);
        }
        KeyCode::Enter => {
            let chosen = state.current().map(|t| (t.cwd.clone(), t.session_id.clone()));
            let dangerous = state.dangerous;
            if let Some((cwd, session_id)) = chosen {
                app.mode = Mode::Dashboard;
                match app.spawn_resume(cwd, dangerous, session_id) {
                    Ok(()) => {
                        app.status = format!(
                            "resumed session [{}]  ·  {}",
                            app.sessions.len(),
                            util::PREFIX_HINT
                        );
                        resize_all(app);
                        app.persist_dirty = true;
                    }
                    Err(e) => {
                        app.status = format!("resume failed: {}", e);
                    }
                }
            } else {
                app.mode = Mode::Dashboard;
            }
        }
        KeyCode::Char(c) => {
            state.filter.push(c);
            state.apply_filter();
            app.mode = Mode::Picker(state);
        }
        _ => {
            app.mode = Mode::Picker(state);
        }
    }
    Ok(())
}

fn handle_rename(app: &mut App, mut state: RenameState, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Dashboard;
        }
        KeyCode::Enter => {
            let new_label = state.buf.trim().to_string();
            if !new_label.is_empty()
                && let Some(s) = app.sessions.iter_mut().find(|s| s.id == state.session_id)
            {
                s.label = new_label;
                s.manually_renamed = true;
            }
            app.mode = Mode::Dashboard;
            app.persist_dirty = true;
        }
        KeyCode::Backspace => {
            state.buf.pop();
            app.mode = Mode::Rename(state);
        }
        KeyCode::Char(c) => {
            state.buf.push(c);
            app.mode = Mode::Rename(state);
        }
        _ => {
            app.mode = Mode::Rename(state);
        }
    }
    Ok(())
}

fn handle_spawn(app: &mut App, mut state: SpawnState, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Dashboard;
        }
        KeyCode::Enter => {
            let chosen = state.pick();
            let dangerous = state.dangerous;
            app.mode = Mode::Dashboard;
            match app.spawn_session(chosen, dangerous) {
                Ok(()) => {
                    app.status = format!(
                        "spawned session [{}]  ·  {}",
                        app.sessions.len(),
                        util::PREFIX_HINT
                    );
                    resize_all(app);
                    app.persist_dirty = true;
                }
                Err(e) => {
                    app.status = format!("spawn failed: {}", e);
                }
            }
            return Ok(());
        }
        KeyCode::Char(' ') => {
            state.dangerous = !state.dangerous;
            app.mode = Mode::Spawn(state);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_sel(-1);
            app.mode = Mode::Spawn(state);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_sel(1);
            app.mode = Mode::Spawn(state);
        }
        KeyCode::PageUp => {
            state.move_sel(-10);
            app.mode = Mode::Spawn(state);
        }
        KeyCode::PageDown => {
            state.move_sel(10);
            app.mode = Mode::Spawn(state);
        }
        KeyCode::Home => {
            state.selected = 0;
            app.mode = Mode::Spawn(state);
        }
        KeyCode::End => {
            state.selected = state.entries.len().saturating_sub(1);
            app.mode = Mode::Spawn(state);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            state.descend();
            app.mode = Mode::Spawn(state);
        }
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
            state.ascend();
            app.mode = Mode::Spawn(state);
        }
        _ => {
            app.mode = Mode::Spawn(state);
        }
    }
    Ok(())
}

fn move_focused(app: &mut App, delta: i32) {
    if app.sessions.is_empty() { return; }
    let to = util::wrap_index(app.focus, app.sessions.len(), delta);
    app.sessions.swap(app.focus, to);
    app.focus = to;
}

fn flush_persist(app: &App) {
    let sessions = app
        .sessions
        .iter()
        .filter(|s| s.alive.load(std::sync::atomic::Ordering::SeqCst))
        .map(persist::PersistedSession::from)
        .collect();
    let state = persist::PersistedState {
        sessions,
        show_sidebar: app.show_sidebar,
    };
    persist::save(&state);
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        original(info);
    }));
}
