mod app;
mod client;
mod connect_mode;
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

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{App, Mode, PickerState, RenameState, SpawnState};

fn run_connect_mode(http: Option<&str>) -> Result<()> {
    install_panic_hook();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    {
        use std::io::Write;
        let mut out = stdout();
        let _ = out.write_all(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h");
        let _ = out.flush();
    }
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let res = run_with_daemon(&mut terminal, http);
    {
        use std::io::Write;
        let mut out = stdout();
        let _ = out.write_all(b"\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l");
    }
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    res
}

fn run_with_daemon(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    http: Option<&str>,
) -> Result<()> {
    let path = client::socket_path()
        .ok_or_else(|| anyhow::anyhow!("no $XDG_RUNTIME_DIR/$HOME for socket"))?;
    let (handle, infos) = connect_mode::connect(&path, http)?;
    let size = terminal.size()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut app = App::new(cwd, (size.height, size.width));
    app.daemon = Some(handle.clone());

    // Adopt every existing daemon session into the sidebar.
    let (term_rows, term_cols) = app.term_size;
    let main_cols = term_cols.saturating_sub(32).saturating_sub(2).max(10);
    let main_rows = term_rows.saturating_sub(3).max(4);
    let adopted = !infos.is_empty();
    for info in infos {
        let _ = app.adopt_daemon_session(info, &handle, main_rows, main_cols);
    }

    // A daemon that is holding sessions already is the source of truth. Only
    // when it has none is this a cold start, and the saved sessions are
    // replayed — into the daemon, so they outlive this process and show up in
    // the browser like everything else.
    let saved = persist::load();
    app.show_sidebar = saved.show_sidebar;
    if !adopted {
        restore_saved(&mut app, saved.sessions);
    }

    // Same loop as local mode; teardown branches on app.daemon to detach
    // instead of killing sessions.
    event_loop(terminal, app)
}

/// Drive the TUI until quit. Single loop body shared by local-mode (`run`)
/// and daemon-mode (`run_with_daemon`); branches on `app.daemon` only at
/// teardown.
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    mut app: App,
) -> Result<()> {
    const HEARTBEAT_MS: u64 = 250;
    const PERSIST_DEBOUNCE_MS: u64 = 2_000;

    let debug = util::debug_enabled();
    let mut tile_sizes: ui::TileSizes = Vec::new();
    let mut last_draw_ms: u64 = 0;
    let mut last_persist_ms: u64 = util::now_ms();

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
        if let Some(t) = &app.toast
            && now >= t.expires_at_ms
        {
            app.toast = None;
            app.needs_redraw = true;
        }
        if app.needs_redraw || any_session_dirty || now.saturating_sub(last_draw_ms) >= HEARTBEAT_MS
        {
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
            dispatch_event(&mut app, event::read()?, debug)?;
        }

        if app.should_quit {
            break;
        }

        // Daemon dropped: flip into the daemon-lost modal. Renders next frame
        // via ui::draw; any keypress exits via handle_key's daemon-lost path.
        // No-op in local mode (`app.daemon` is None).
        if !app.daemon_lost
            && let Some(d) = &app.daemon
            && !d.alive.load(std::sync::atomic::Ordering::SeqCst)
        {
            app.daemon_lost = true;
            app.needs_redraw = true;
        }
    }

    if app.daemon.is_some() {
        for s in app.sessions.iter_mut() {
            s.detach_keep();
        }
    }
    if app.persist_dirty {
        flush_persist(&app);
    }
    Ok(())
}

fn dispatch_event(app: &mut App, ev: Event, debug: bool) -> Result<()> {
    match ev {
        Event::Key(key) => {
            if debug {
                log_key(&key, app.prefix_pending);
            }
            if key.kind == KeyEventKind::Release {
                return Ok(());
            }
            handle_key(app, key)?;
            app.needs_redraw = true;
        }
        Event::Resize(cols, rows) => {
            app.term_size = (rows, cols);
            resize_all(app);
            app.needs_redraw = true;
        }
        Event::Mouse(me) => handle_mouse(app, me),
        Event::Paste(text) => {
            handle_paste(app, &text);
            app.needs_redraw = true;
        }
        _ => {}
    }
    Ok(())
}

#[derive(clap::Parser, Debug)]
#[command(
    name = "cmux",
    version,
    about = "tmux-style TUI for managing multiple `claude` CLI sessions"
)]
struct Cli {
    /// Now the default; accepted so existing invocations keep working.
    #[arg(long)]
    connect: bool,

    /// Own the PTYs in this process instead of handing them to cmuxd. Sessions
    /// then die when you quit, and nothing else — no second terminal, no
    /// browser — can see them.
    #[arg(long)]
    local: bool,

    /// Start the daemon's HTTP + WebSocket API too, so the same sessions can
    /// be picked up in a browser. Only applies when this command is the one
    /// that spawns the daemon; a daemon already running keeps its own setting.
    #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:7070")]
    http: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Talk to a running cmuxd daemon over its UNIX socket.
    Ctl {
        #[command(subcommand)]
        cmd: CtlCmd,
    },
}

#[derive(clap::Subcommand, Debug)]
enum CtlCmd {
    /// List sessions the daemon is currently hosting.
    List,
    /// Spawn a new session in the chosen directory. Runs claude unless a
    /// command is given after `--`, e.g. `cmux ctl spawn . -- bash -l`.
    Spawn {
        /// Working directory for the new session.
        #[arg(default_value = ".")]
        cwd: PathBuf,
        /// Pass `--dangerously-skip-permissions` to claude. Claude only.
        #[arg(long)]
        dangerous: bool,
        /// Display name for the session in the sidebar.
        #[arg(long)]
        label: Option<String>,
        /// Command to run instead of claude. Everything after `--` is argv.
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Kill a session by id.
    Kill {
        /// Session id (matches `cmux ctl list`).
        id: u64,
    },
    /// Tell the daemon to exit and kill every session it owns.
    Shutdown,
    /// Print a one-line summary (session count).
    Status,
}

fn run_ctl(cmd: CtlCmd) -> Result<()> {
    let path = client::socket_path()
        .ok_or_else(|| anyhow::anyhow!("no $XDG_RUNTIME_DIR/$HOME for socket"))?;
    let mut c = client::Client::connect(&path).context("connect cmuxd")?;
    match cmd {
        CtlCmd::List => {
            c.send(&cmux_proto::Request::ListSessions)?;
            match c.recv()? {
                cmux_proto::Event::SessionList { sessions } => {
                    if sessions.is_empty() {
                        println!("(no sessions)");
                    } else {
                        for s in sessions {
                            println!(
                                "[{}] {:<20}  {}  {}x{}  {}{}",
                                s.id,
                                s.label,
                                s.cwd.display(),
                                s.rows,
                                s.cols,
                                s.cmd.join(" "),
                                if s.attention { "  ⚠" } else { "" }
                            );
                        }
                    }
                }
                other => anyhow::bail!("unexpected event: {other:?}"),
            }
        }
        CtlCmd::Kill { id } => {
            c.send(&cmux_proto::Request::Detach {
                session_id: id,
                keep_session: false,
            })?;
            println!("kill sent for session {id}");
        }
        CtlCmd::Spawn {
            cwd,
            dangerous,
            label,
            cmd,
        } => {
            let cwd: PathBuf = if cwd.as_os_str() == "." {
                std::env::current_dir().context("getcwd")?
            } else {
                cwd.canonicalize().unwrap_or(cwd)
            };
            if dangerous && !cmd.is_empty() {
                anyhow::bail!(
                    "--dangerous is a claude flag; drop it when passing your own command"
                );
            }
            let (cmd, probe) = if cmd.is_empty() {
                (
                    cmux_proto::claude_command(dangerous, None),
                    cmux_proto::ProbeKind::Claude {
                        dangerous,
                        resume_id: None,
                    },
                )
            } else {
                (cmd, cmux_proto::ProbeKind::None)
            };
            let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            c.send(&cmux_proto::Request::SpawnSession {
                cwd: cwd.clone(),
                cmd,
                probe,
                label,
                rows,
                cols,
            })?;
            match c.recv()? {
                cmux_proto::Event::SessionSpawned { id, info } => {
                    println!("spawned [{}] {} at {}", id, info.label, info.cwd.display());
                }
                cmux_proto::Event::Error { message, .. } => {
                    anyhow::bail!("daemon error: {message}");
                }
                other => anyhow::bail!("unexpected event: {other:?}"),
            }
        }
        CtlCmd::Shutdown => {
            c.send(&cmux_proto::Request::Shutdown)?;
            // best-effort drain
            let _ = c.recv();
            println!("shutdown requested");
        }
        CtlCmd::Status => {
            c.send(&cmux_proto::Request::ListSessions)?;
            match c.recv()? {
                cmux_proto::Event::SessionList { sessions } => {
                    println!("cmuxd: {} sessions", sessions.len());
                }
                other => anyhow::bail!("unexpected event: {other:?}"),
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    use clap::Parser;
    let cli = Cli::parse();
    if let Some(Command::Ctl { cmd }) = cli.command {
        return run_ctl(cmd);
    }
    if cli.connect && cli.local {
        anyhow::bail!("--connect and --local are opposites; pass one or neither");
    }
    // Daemon-backed unless asked otherwise: sessions outlive the TUI, and a
    // browser or a second terminal can attach to the same ones.
    if !cli.local {
        return run_connect_mode(cli.http.as_deref());
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
        let _ = out.write_all(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h");
        let _ = out.flush();
    }
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal);

    {
        use std::io::Write;
        let mut out = stdout();
        let _ = out.write_all(b"\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l");
        let _ = out.flush();
    }
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    res
}

/// Respawn saved sessions. `App::spawn_*` route through the daemon whenever
/// one is attached, so this serves both modes.
/// Whether a saved label should be pinned on the daemon. Only a name the user
/// chose: a label merely carried over from the last run has to stay
/// overridable, or the status probe can never rename the session again and the
/// TUI ends up showing a different name from the browser.
fn should_pin_label(ps: &persist::PersistedSession) -> bool {
    ps.manually_renamed && !ps.label.is_empty()
}

fn restore_saved(app: &mut App, saved: Vec<persist::PersistedSession>) {
    for ps in saved {
        let label = (!ps.label.is_empty()).then(|| ps.label.clone());
        let res = app.restore_session(ps.cwd.clone(), ps.dangerous, ps.resume_id.clone(), label);
        if res.is_ok()
            && let Some(s) = app.sessions.last_mut()
        {
            s.manually_renamed = ps.manually_renamed;
            if should_pin_label(&ps) {
                s.set_label(ps.label);
            }
        }
    }
}

fn run(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
    let size = terminal.size()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut app = App::new(cwd, (size.height, size.width));

    let saved = persist::load();
    app.show_sidebar = saved.show_sidebar;
    restore_saved(&mut app, saved.sessions);
    flush_persist(&app);

    event_loop(terminal, app)
}

fn apply_scroll_lines(app: &mut App, delta: i32) {
    use alacritty_terminal::grid::Scroll;
    let Mode::Scrollback(id) = app.mode else {
        return;
    };
    let Some(s) = app.sessions.iter_mut().find(|s| s.id == id) else {
        return;
    };
    let Some(sb) = s.scrollback.as_mut() else {
        return;
    };
    sb.scroll(Scroll::Delta(delta));
    if sb.display_offset() == 0 && delta < 0 {
        s.scrollback = None;
        s.selection = None;
        app.mode = Mode::Dashboard;
    }
    app.needs_redraw = true;
}

fn handle_mouse(app: &mut App, me: MouseEvent) {
    let Some(tile) = app.last_tile_area else {
        return;
    };
    let inside = me.column >= tile.x
        && me.column < tile.x + tile.width
        && me.row >= tile.y
        && me.row < tile.y + tile.height;

    match me.kind {
        MouseEventKind::Down(MouseButton::Left) => mouse_press(app, me, tile, inside),
        MouseEventKind::Drag(MouseButton::Left) if inside => mouse_drag(app, me, tile),
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if inside => {
            mouse_wheel(app, me, tile)
        }
        MouseEventKind::Up(MouseButton::Left) => mouse_release(app),
        _ => {}
    }
}

fn mouse_press(app: &mut App, me: MouseEvent, tile: ratatui::layout::Rect, inside: bool) {
    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    let had_selection = s.selection.is_some();
    s.selection = None;
    s.mouse_down_at = inside.then(|| (me.row - tile.y, me.column - tile.x));
    if had_selection {
        app.needs_redraw = true;
    }
}

fn mouse_drag(app: &mut App, me: MouseEvent, tile: ratatui::layout::Rect) {
    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    let Some(anchor) = s.mouse_down_at else {
        return;
    };
    let row = me.row - tile.y;
    let col = me.column - tile.x;
    let sel = s
        .selection
        .get_or_insert_with(|| term_render::TileSelection::new(anchor.0, anchor.1));
    sel.anchor = anchor;
    sel.tip = (row, col);
    app.needs_redraw = true;
}

fn mouse_wheel(app: &mut App, me: MouseEvent, tile: ratatui::layout::Rect) {
    use alacritty_terminal::term::TermMode;
    let up = matches!(me.kind, MouseEventKind::ScrollUp);

    if matches!(app.mode, Mode::Scrollback(_)) {
        apply_scroll_lines(app, if up { 1 } else { -1 });
        return;
    }

    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    let mode = s.parser.lock().ok().map(|p| *p.term.mode());
    let col = me.column.saturating_sub(tile.x) + 1;
    let row = me.row.saturating_sub(tile.y) + 1;
    let seq: Vec<u8> = match mode {
        Some(m) if m.intersects(TermMode::SGR_MOUSE) => {
            let btn = if up { 64 } else { 65 };
            format!("\x1b[<{};{};{}M", btn, col, row).into_bytes()
        }
        Some(m) if m.intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION) => {
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
        Some(m) if m.contains(TermMode::ALT_SCREEN) && m.contains(TermMode::ALTERNATE_SCROLL) => {
            if up {
                b"\x1b[A".to_vec()
            } else {
                b"\x1b[B".to_vec()
            }
        }
        _ => {
            if up {
                b"\x1b[5~".to_vec()
            } else {
                b"\x1b[6~".to_vec()
            }
        }
    };
    let _ = s.write(&seq);
}

fn mouse_release(app: &mut App) {
    if let Some(s) = app.sessions.get_mut(app.focus) {
        s.mouse_down_at = None;
    }
    let in_scrollback = matches!(app.mode, Mode::Scrollback(_));
    let Some(s) = app.sessions.get(app.focus) else {
        return;
    };
    let Some(sel) = s.selection else { return };
    let text = if in_scrollback && let Some(sb) = &s.scrollback {
        term_render::extract_selection(&sb.term, sel)
    } else if let Ok(p) = s.parser.lock() {
        term_render::extract_selection(&p.term, sel)
    } else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    let count = text.chars().count();
    emit_osc52(&text);
    app.toast = Some(app::Toast {
        text: format!("copied ✓ {} chars", count),
        expires_at_ms: util::now_ms() + 1400,
    });
    app.needs_redraw = true;
}

fn handle_paste(app: &mut App, text: &str) {
    use alacritty_terminal::term::TermMode;
    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    let bracketed = s
        .parser
        .lock()
        .ok()
        .map(|p| p.term.mode().contains(TermMode::BRACKETED_PASTE))
        .unwrap_or(false);
    if bracketed {
        let mut buf: Vec<u8> = Vec::with_capacity(text.len() + 12);
        buf.extend_from_slice(b"\x1b[200~");
        buf.extend_from_slice(text.as_bytes());
        buf.extend_from_slice(b"\x1b[201~");
        let _ = s.write(&buf);
    } else {
        let cleaned: String = text.chars().filter(|&c| c != '\u{1b}').collect();
        let _ = s.write(cleaned.as_bytes());
    }
}

fn emit_osc52(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\x1b]52;c;{}\x07", encoded);
    let _ = stdout.flush();
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
    let main_cols = term_cols
        .saturating_sub(sidebar_w)
        .saturating_sub(2)
        .max(10);
    let main_rows = term_rows.saturating_sub(3).max(4);
    if let Some(s) = app.sessions.get_mut(app.focus) {
        let _ = s.resize(main_rows, main_cols);
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Daemon-lost modal: swallow all keys, any key dismisses + quits.
    if app.daemon_lost {
        let _ = key;
        app.should_quit = true;
        return Ok(());
    }
    if keys::HARD_QUIT.matches(&key) {
        app.should_quit = true;
        return Ok(());
    }

    let is_prefix_key = keys::PREFIX.matches(&key) || matches!(key.code, KeyCode::Char('\u{01}'));

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
    if keys::REORDER_UP.matches(&key) {
        move_focused(app, -1);
        app.persist_dirty = true;
        app.mode = Mode::Reorder;
    } else if keys::REORDER_DOWN.matches(&key) {
        move_focused(app, 1);
        app.persist_dirty = true;
        app.mode = Mode::Reorder;
    } else if keys::REORDER_EXIT.matches(&key) {
        app.mode = Mode::Dashboard;
    } else {
        app.mode = Mode::Reorder;
    }
    Ok(())
}

fn handle_scrollback(app: &mut App, id: u64, key: KeyEvent) -> Result<()> {
    use alacritty_terminal::grid::Scroll;
    let Some(s) = app.sessions.iter_mut().find(|s| s.id == id) else {
        app.mode = Mode::Dashboard;
        return Ok(());
    };

    if keys::SCROLLBACK_EXIT.matches(&key) {
        s.scrollback = None;
        s.selection = None;
        app.mode = Mode::Dashboard;
        return Ok(());
    }

    let scroll = if keys::SCROLLBACK_UP.matches(&key) {
        Scroll::Delta(1)
    } else if keys::SCROLLBACK_DOWN.matches(&key) {
        Scroll::Delta(-1)
    } else if keys::SCROLLBACK_PAGE_UP.matches(&key) {
        Scroll::PageUp
    } else if keys::SCROLLBACK_PAGE_DOWN.matches(&key) {
        Scroll::PageDown
    } else if keys::SCROLLBACK_TOP.matches(&key) {
        Scroll::Top
    } else if keys::SCROLLBACK_BOTTOM.matches(&key) {
        Scroll::Bottom
    } else {
        app.mode = Mode::Scrollback(id);
        return Ok(());
    };

    if let Some(sb) = s.scrollback.as_mut() {
        sb.scroll(scroll);
    }
    app.mode = Mode::Scrollback(id);
    Ok(())
}

fn handle_confirm_detach(app: &mut App, id: u64, key: KeyEvent) -> Result<()> {
    if keys::CONFIRM_YES.matches(&key) {
        if let Some(idx) = app.sessions.iter().position(|s| s.id == id) {
            app.focus = idx;
            app.detach_focused();
            app.persist_dirty = true;
        }
        app.mode = Mode::Dashboard;
    } else if keys::CONFIRM_NO.matches(&key) {
        app.mode = Mode::Dashboard;
    } else {
        app.mode = Mode::ConfirmDetach(id);
    }
    Ok(())
}

fn handle_prefix_chord(app: &mut App, key: KeyEvent) -> Result<()> {
    let mode_before = format!("{:?}", std::mem::discriminant(&app.mode));

    if keys::PREFIX_FOCUS_NEXT.matches(&key) {
        app.cycle_focus(1);
        return Ok(());
    }
    if keys::PREFIX_FOCUS_PREV.matches(&key) {
        app.cycle_focus(-1);
        return Ok(());
    }
    if keys::PREFIX_QUIT.matches(&key) {
        app.should_quit = true;
    } else if keys::PREFIX_SPAWN.matches(&key) {
        app.mode = Mode::Spawn(SpawnState::new(app.default_cwd.clone()));
    } else if keys::PREFIX_DETACH.matches(&key) {
        if let Some(s) = app.sessions.get(app.focus) {
            app.mode = Mode::ConfirmDetach(s.id);
        }
    } else if keys::PREFIX_TOGGLE_SIDEBAR.matches(&key) {
        app.show_sidebar = !app.show_sidebar;
        resize_all(app);
        app.persist_dirty = true;
    } else if keys::PREFIX_RENAME.matches(&key) {
        if let Some(s) = app.sessions.get(app.focus) {
            app.mode = Mode::Rename(RenameState {
                session_id: s.id,
                buf: s.label.clone(),
            });
        }
    } else if keys::PREFIX_PICKER.matches(&key) {
        app.mode = Mode::Picker(PickerState::new());
    } else if keys::PREFIX_SEND_CTRL_A.matches(&key) {
        if let Some(s) = app.sessions.get_mut(app.focus) {
            let _ = s.write(&[0x01]);
        }
    } else if keys::PREFIX_SCROLLBACK.matches(&key) {
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
    } else if keys::PREFIX_HELP.matches(&key) {
        app.mode = Mode::Help;
    } else if keys::PREFIX_REORDER.matches(&key) && !app.sessions.is_empty() {
        app.mode = Mode::Reorder;
    } else if let KeyCode::Char(c) = key.code
        && c.is_ascii_digit()
    {
        let idx = if c == '0' {
            9
        } else {
            (c as u8 - b'1') as usize
        };
        if idx < app.sessions.len() {
            app.focus = idx;
            resize_all(app);
        }
    }
    if util::debug_enabled() {
        let mode_after = format!("{:?}", std::mem::discriminant(&app.mode));
        debug_log!(
            "/tmp/cmux-keys.log",
            "  CHORD {:?} mode_before={} mode_after={} sessions={} focus={}",
            key.code,
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
    if keys::PICKER_CANCEL.matches(&key) {
        app.mode = Mode::Dashboard;
    } else if keys::PICKER_UP.matches(&key) {
        state.move_sel(-1);
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_DOWN.matches(&key) {
        state.move_sel(1);
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_PGUP.matches(&key) {
        state.move_sel(-10);
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_PGDOWN.matches(&key) {
        state.move_sel(10);
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_HOME.matches(&key) {
        state.selected = 0;
        state.ensure_preview();
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_END.matches(&key) {
        state.selected = state.items.len().saturating_sub(1);
        state.ensure_preview();
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_TOGGLE_DANGER.matches(&key) {
        state.dangerous = !state.dangerous;
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_FILTER_CLEAR.matches(&key) {
        state.filter.pop();
        state.apply_filter();
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_PICK.matches(&key) {
        let chosen = state
            .current()
            .map(|t| (t.cwd.clone(), t.session_id.clone()));
        let dangerous = state.dangerous;
        if let Some((cwd, session_id)) = chosen {
            app.mode = Mode::Dashboard;
            match app.spawn_resume(cwd, dangerous, session_id) {
                Ok(()) => {
                    app.status = format!(
                        "resumed session [{}]  {}",
                        app.sessions.len(),
                        util::prefix_hint()
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
    } else if let KeyCode::Char(c) = key.code {
        state.filter.push(c);
        state.apply_filter();
        app.mode = Mode::Picker(state);
    } else {
        app.mode = Mode::Picker(state);
    }
    Ok(())
}

fn handle_rename(app: &mut App, mut state: RenameState, key: KeyEvent) -> Result<()> {
    if keys::RENAME_CANCEL.matches(&key) {
        app.mode = Mode::Dashboard;
    } else if keys::RENAME_SAVE.matches(&key) {
        let new_label = state.buf.trim().to_string();
        if !new_label.is_empty()
            && let Some(s) = app.sessions.iter_mut().find(|s| s.id == state.session_id)
        {
            s.set_label(new_label);
            s.manually_renamed = true;
        }
        app.mode = Mode::Dashboard;
        app.persist_dirty = true;
    } else if matches!(key.code, KeyCode::Backspace) {
        state.buf.pop();
        app.mode = Mode::Rename(state);
    } else if let KeyCode::Char(c) = key.code {
        state.buf.push(c);
        app.mode = Mode::Rename(state);
    } else {
        app.mode = Mode::Rename(state);
    }
    Ok(())
}

fn handle_spawn(app: &mut App, mut state: SpawnState, key: KeyEvent) -> Result<()> {
    if keys::SPAWN_CANCEL.matches(&key) {
        app.mode = Mode::Dashboard;
    } else if keys::SPAWN_PICK.matches(&key) {
        let chosen = state.pick();
        let dangerous = state.dangerous;
        app.mode = Mode::Dashboard;
        match app.spawn_session(chosen, dangerous) {
            Ok(()) => {
                app.status = format!(
                    "spawned session [{}]  {}",
                    app.sessions.len(),
                    util::prefix_hint()
                );
                resize_all(app);
                app.persist_dirty = true;
            }
            Err(e) => {
                app.status = format!("spawn failed: {}", e);
            }
        }
    } else if keys::SPAWN_TOGGLE_DANGER.matches(&key) {
        state.dangerous = !state.dangerous;
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_UP.matches(&key) {
        state.move_sel(-1);
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_DOWN.matches(&key) {
        state.move_sel(1);
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_PGUP.matches(&key) {
        state.move_sel(-10);
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_PGDOWN.matches(&key) {
        state.move_sel(10);
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_HOME.matches(&key) {
        state.selected = 0;
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_END.matches(&key) {
        state.selected = state.entries.len().saturating_sub(1);
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_DESCEND.matches(&key) {
        state.descend();
        app.mode = Mode::Spawn(state);
    } else if keys::SPAWN_ASCEND.matches(&key) {
        state.ascend();
        app.mode = Mode::Spawn(state);
    } else {
        app.mode = Mode::Spawn(state);
    }
    Ok(())
}

fn move_focused(app: &mut App, delta: i32) {
    if app.sessions.is_empty() {
        return;
    }
    let to = util::wrap_index(app.focus, app.sessions.len(), delta);
    app.sessions.swap(app.focus, to);
    app.focus = to;
}

fn flush_persist(app: &App) {
    // In daemon mode the daemon owns the canonical session list; the TUI
    // must not overwrite ~/.config/cmux/state.json with daemon-adopted
    // sessions (next launch in local mode would then try to spawn them as
    // fresh local PTYs).
    if app.daemon.is_some() {
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(label: &str, manually_renamed: bool) -> persist::PersistedSession {
        persist::PersistedSession {
            cwd: PathBuf::from("/tmp"),
            label: label.to_string(),
            dangerous: false,
            resume_id: None,
            manually_renamed,
        }
    }

    /// Both directions. A name the user typed is pinned, so the probe cannot
    /// undo it. A label merely carried over from the last run is not, or the
    /// probe can never rename the session and the TUI ends up showing a
    /// different name from the browser.
    #[test]
    fn only_a_user_chosen_name_is_pinned_on_the_daemon() {
        assert!(should_pin_label(&saved("mine", true)));
        assert!(!should_pin_label(&saved("saved-dirname", false)));
        assert!(!should_pin_label(&saved("", true)));
        assert!(!should_pin_label(&saved("", false)));
    }
}
