mod app;
mod keys;
mod persist;
mod session;
mod term_render;
mod transcripts;
mod ui;
#[macro_use]
mod util;

use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{App, Mode, PickerState, RenameState, SpawnState};

fn main() -> Result<()> {
    install_panic_hook();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal);

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
    persist_now(&app);

    let debug = std::env::var_os("CMUX_DEBUG").is_some();
    let mut tile_sizes: ui::TileSizes = Vec::new();
    loop {
        app.reap_dead();
        terminal.draw(|f| ui::draw(f, &app, &mut tile_sizes))?;
        for (idx, rows, cols) in tile_sizes.drain(..) {
            if let Some(s) = app.sessions.get_mut(idx) {
                let _ = s.resize(rows.max(2), cols.max(4));
            }
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
                }
                Event::Resize(cols, rows) => {
                    app.term_size = (rows, cols);
                    resize_all(&mut app);
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
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
            persist_now(app);
            app.mode = Mode::Reorder;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            move_focused(app, 1);
            persist_now(app);
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
    let Some(s) = app.sessions.iter().find(|s| s.id == id) else {
        app.mode = Mode::Dashboard;
        return Ok(());
    };

    let exit = matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q'));
    if exit {
        if let Ok(mut p) = s.parser.lock() {
            p.scroll(Scroll::Bottom);
        }
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

    if let Ok(mut p) = s.parser.lock() {
        p.scroll(scroll);
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
                persist_now(app);
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
            persist_now(app);
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
            if let Some(s) = app.sessions.get(app.focus) {
                app.mode = Mode::Scrollback(s.id);
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
                        persist_now(app);
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
            persist_now(app);
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
                    persist_now(app);
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

fn persist_now(app: &App) {
    let sessions = app
        .sessions
        .iter()
        .filter(|s| s.alive.load(std::sync::atomic::Ordering::SeqCst))
        .map(|s| persist::PersistedSession {
            cwd: s.cwd.clone(),
            label: s.label.clone(),
            dangerous: s.dangerous,
            resume_id: s.resume_id.clone(),
            manually_renamed: s.manually_renamed,
        })
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
