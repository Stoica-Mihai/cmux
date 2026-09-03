mod app;
mod claude_sessions;
mod client;
mod connect_mode;
mod copy_buffer;
mod file_links;
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
        let _ = out.flush();
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
    let (main_rows, main_cols) = app.tile_size();
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
    let debug = util::debug_enabled();
    let mut tile_sizes: ui::TileSizes = Vec::new();

    let result = drive(terminal, &mut app, debug, &mut tile_sizes);

    // Teardown runs whatever ended the loop. An error return used to skip it,
    // so a draw or input failure left daemon sessions unsubscribed and the
    // session list unsaved.
    if app.daemon.is_some() {
        for s in app.sessions.iter_mut() {
            s.detach_keep();
        }
    }
    if app.persist_dirty {
        flush_persist(&app);
    }
    result
}

/// The event loop proper. Any error here still gets [`event_loop`]'s teardown.
fn drive(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    debug: bool,
    tile_sizes: &mut ui::TileSizes,
) -> Result<()> {
    const HEARTBEAT_MS: u64 = 250;
    const PERSIST_DEBOUNCE_MS: u64 = 2_000;
    let mut last_draw_ms: u64 = 0;
    let mut last_persist_ms: u64 = util::now_ms();

    loop {
        app.reap_dead();
        let now = util::now_ms();

        if app.persist_dirty && now.saturating_sub(last_persist_ms) >= PERSIST_DEBOUNCE_MS {
            flush_persist(app);
            app.persist_dirty = false;
            last_persist_ms = now;
        }

        // Both browsers read the filesystem on a worker thread; take whatever
        // has landed.
        let arrived = match &mut app.mode {
            Mode::Picker(p) => p.poll(),
            Mode::Spawn(s) => s.poll(),
            _ => false,
        };
        if arrived {
            app.needs_redraw = true;
        }

        drive_edge_scroll(app, now, tile_area_height(app));
        stitch_scrolls(app, now);

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
        // A scrolling name needs a frame per column; a still sidebar does not.
        let beat = if app.marquee_active {
            ui::MARQUEE_STEP_MS
        } else {
            HEARTBEAT_MS
        };
        if app.needs_redraw || any_session_dirty || now.saturating_sub(last_draw_ms) >= beat {
            project_selection(app);
            app.render_tick = app.render_tick.wrapping_add(1);
            terminal.draw(|f| ui::draw(f, app, tile_sizes))?;
            for (idx, rows, cols) in tile_sizes.drain(..) {
                if let Some(s) = app.sessions.get_mut(idx) {
                    let _ = s.resize(rows.max(2), cols.max(4));
                }
            }
            paint_hyperlinks(app, now);
            app.needs_redraw = false;
            last_draw_ms = now;
        }

        if event::poll(Duration::from_millis(40))? {
            dispatch_event(app, event::read()?, debug)?;
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
    Ok(())
}

/// How tall the focused tile is, for a wheel report's coordinates.
fn tile_area_height(app: &App) -> ratatui::layout::Rect {
    app.last_tile_area.unwrap_or(ratatui::layout::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    })
}

/// Keep scrolling while a drag is held against an edge. A pointer held still
/// sends no further events, so the repeat runs here rather than on motion.
fn drive_edge_scroll(app: &mut App, now: u64, tile: ratatui::layout::Rect) {
    /// One scroll per this many milliseconds while the edge is held.
    const EDGE_SCROLL_MS: u64 = 120;

    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    let Some(up) = s.drag_edge else {
        return;
    };
    if s.stitch.is_some() || now.saturating_sub(s.last_edge_scroll_ms) < EDGE_SCROLL_MS {
        return;
    }
    let Some(seq) = wheel_sequence(s, up, tile) else {
        return;
    };
    if s.write(&seq).is_ok() {
        s.stitch = Some(session::StitchWait {
            sent_ms: now,
            answered: false,
        });
        s.last_edge_scroll_ms = now;
    }
}

/// Take the repaint a scroll asked for. The child redraws asynchronously, so
/// this waits for a frame in which nothing arrived: stitching a half-drawn
/// screen would match against rows that are about to change.
fn stitch_scrolls(app: &mut App, now: u64) {
    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    let Some(wait) = s.stitch else {
        return;
    };
    let arriving = s.dirty.load(std::sync::atomic::Ordering::Relaxed);
    if arriving {
        // The repaint has started. Wait for the frame after it stops.
        s.stitch = Some(session::StitchWait {
            answered: true,
            ..wait
        });
        return;
    }
    if !wait.answered {
        /// A child that ignores the wheel never answers, and the drag must
        /// not wait on it for ever.
        const ANSWER_TIMEOUT_MS: u64 = 400;
        if now.saturating_sub(wait.sent_ms) >= ANSWER_TIMEOUT_MS {
            s.stitch = None;
        }
        return;
    }
    let stitched = match (&mut s.copy, &s.scrollback) {
        (Some(buf), Some(sb)) => buf.stitch_term(&sb.term),
        (Some(buf), None) => match s.parser.lock() {
            Ok(p) => buf.stitch_term(&p.term),
            Err(_) => None,
        },
        _ => None,
    };
    s.stitch = None;
    if let Some(prepended) = stitched {
        if let (Some(buf), Some(drag)) = (&s.copy, &mut s.drag) {
            // Rows added at the front shift every line index the drag holds.
            drag.anchor.0 += prepended;
            drag.tip.0 += prepended;
            // The tip then follows the edge the drag is held against, so the
            // selection keeps growing while the button is down. A trim can
            // have dropped chrome the anchor was sitting on, so both ends
            // stay inside what the buffer actually holds.
            let last = buf.len().saturating_sub(1);
            let rows = s.size.0;
            let below = drag.tip.0 >= drag.anchor.0;
            drag.tip = if below {
                (
                    (buf.top() + rows.saturating_sub(1) as usize).min(last),
                    drag.tip.1,
                )
            } else {
                (buf.top(), drag.tip.1)
            };
            drag.anchor.0 = drag.anchor.0.min(last);
        }
        app.needs_redraw = true;
    }
}

/// Paint the drag onto whatever is on screen now. The selection lives in
/// buffer lines; the highlight needs viewport rows, and the two part company
/// the moment the child scrolls.
fn project_selection(app: &mut App) {
    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    let (Some(buf), Some(drag)) = (&s.copy, &s.drag) else {
        return;
    };
    if drag.anchor == drag.tip {
        s.selection = None;
        return;
    }
    let rows = s.size.0;
    let (lo, hi) = if drag.anchor <= drag.tip {
        (drag.anchor, drag.tip)
    } else {
        (drag.tip, drag.anchor)
    };
    // A line scrolled off the top selects from the first visible row, and one
    // past the bottom to the last: the part on screen is highlighted, and the
    // rest is still in the buffer for the copy.
    let last_line = buf.len().saturating_sub(1);
    let (lo, hi) = ((lo.0.min(last_line), lo.1), (hi.0.min(last_line), hi.1));
    let top_row = buf.viewport_row(lo.0, rows).unwrap_or(0);
    let top_col = if buf.viewport_row(lo.0, rows).is_some() {
        lo.1
    } else {
        0
    };
    let bottom_row = buf
        .viewport_row(hi.0, rows)
        .unwrap_or_else(|| rows.saturating_sub(1));
    let bottom_col = if buf.viewport_row(hi.0, rows).is_some() {
        hi.1
    } else {
        s.size.1.saturating_sub(1)
    };
    s.selection = Some(term_render::TileSelection {
        anchor: (top_row, top_col),
        tip: (bottom_row, bottom_col),
    });
}

/// Wrap the focused tile's OSC 8 links onto the frame just drawn. ratatui
/// cannot carry a hyperlink through its buffer, so the links are painted over
/// the cells it already placed. A popup covering the tile skips the pass.
/// Overdraw the focused tile's links: the ones the program printed as OSC 8,
/// and the `file://` targets cmux synthesises for the file paths on screen.
fn paint_hyperlinks(app: &mut App, now_ms: u64) {
    use std::io::Write;
    let covered = matches!(
        app.mode,
        Mode::Spawn(_) | Mode::Rename(_) | Mode::Picker(_) | Mode::ConfirmDetach(_) | Mode::Help
    ) || app.daemon_lost;
    if covered {
        return;
    }
    let (Some(area), Some(session)) = (app.last_tile_area, app.sessions.get(app.focus)) else {
        return;
    };
    let cache = &mut app.file_links;
    let cwd = session.cwd.clone();
    let mut buf: Vec<u8> = Vec::new();
    let painted = match &session.scrollback {
        Some(sb) => {
            let files = file_links::detect(&cmux_term::grid_rows(&sb.term), &cwd, cache, now_ms);
            term_render::emit_hyperlinks(&sb.term, area, &files, &mut buf)
        }
        None => match session.parser.lock() {
            Ok(p) => {
                let files = file_links::detect(&cmux_term::grid_rows(&p.term), &cwd, cache, now_ms);
                term_render::emit_hyperlinks(&p.term, area, &files, &mut buf)
            }
            Err(_) => return,
        },
    };
    if painted.is_err() || buf.is_empty() {
        return;
    }
    let mut out = std::io::stdout().lock();
    // Save and restore the cursor around the overdraw, so the frame ratatui
    // just positioned is left as it was.
    let _ = out.write_all(b"\x1b7");
    let _ = out.write_all(&buf);
    let _ = out.write_all(b"\x1b8");
    let _ = out.flush();
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
                                match (&s.alive, &s.exit_status) {
                                    (false, Some(st)) => format!("  ({st})"),
                                    (false, None) => "  (exited)".to_string(),
                                    _ if s.attention => "  ⚠".to_string(),
                                    _ => String::new(),
                                }
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
                    cmux_proto::claude_command(dangerous, cmux_proto::Launch::New),
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
    let mut failed: Vec<String> = Vec::new();
    for ps in saved {
        let label = (!ps.label.is_empty()).then(|| ps.label.clone());
        let name = if ps.label.is_empty() {
            ps.cwd.display().to_string()
        } else {
            ps.label.clone()
        };
        match app.restore_session(ps.cwd.clone(), ps.dangerous, ps.resume_id.clone(), label) {
            Ok(()) => {
                if let Some(s) = app.sessions.last_mut() {
                    s.manually_renamed = ps.manually_renamed;
                    if should_pin_label(&ps) {
                        s.set_label(ps.label);
                    }
                }
            }
            // A session that cannot come back leaves the list silently
            // shorter than the one that was saved, so say which and why.
            Err(e) => failed.push(format!("{name}: {e:#}")),
        }
    }
    if !failed.is_empty() {
        app.status = format!(
            "{} session(s) did not restore - {}",
            failed.len(),
            failed.join("; ")
        );
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
        // Not gated on `inside`: a drag that leaves the tile used to stop
        // updating the tip, freezing the selection wherever the pointer
        // crossed the edge instead of extending it to the edge.
        MouseEventKind::Drag(MouseButton::Left) => mouse_drag(app, me, tile),
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
    s.drag = None;
    s.copy = None;
    s.stitch = None;
    s.drag_edge = None;
    s.mouse_down_at = inside.then(|| (me.row - tile.y, me.column - tile.x));

    // Start collecting from what is on screen. A drag that never reaches an
    // edge only ever reads this one screen; one that does gets the rows the
    // child reveals stitched on.
    if let Some((row, col)) = s.mouse_down_at {
        let screen = match &s.scrollback {
            Some(sb) => Some(copy_buffer::CopyBuffer::capture(&sb.term, s.size)),
            None => s
                .parser
                .lock()
                .ok()
                .map(|p| copy_buffer::CopyBuffer::capture(&p.term, s.size)),
        };
        if let Some(buf) = screen
            && let Some(line) = buf.line_at(row)
        {
            s.drag = Some(session::DragRange {
                anchor: (line, col),
                tip: (line, col),
            });
            s.copy = Some(buf);
        }
    }
    if had_selection {
        app.needs_redraw = true;
    }
}

/// A pointer position as a cell inside the tile, clamped to its edges. A
/// pointer dragged past an edge selects up to that edge rather than freezing
/// the selection where it crossed. `None` for a tile with no area.
fn tile_cell(row: u16, col: u16, tile: ratatui::layout::Rect) -> Option<(u16, u16)> {
    if tile.height == 0 || tile.width == 0 {
        return None;
    }
    Some((
        row.clamp(tile.y, tile.y + tile.height - 1) - tile.y,
        col.clamp(tile.x, tile.x + tile.width - 1) - tile.x,
    ))
}

fn mouse_drag(app: &mut App, me: MouseEvent, tile: ratatui::layout::Rect) {
    let Some(s) = app.sessions.get_mut(app.focus) else {
        return;
    };
    if s.mouse_down_at.is_none() {
        return;
    }
    let Some((row, col)) = tile_cell(me.row, me.column, tile) else {
        return;
    };

    // A resize rewraps everything, so the rows collected at the old width no
    // longer say what is on screen.
    if s.copy.as_ref().is_some_and(|b| b.size() != s.size) {
        s.copy = None;
        s.drag = None;
        s.selection = None;
        s.stitch = None;
        return;
    }

    // Past an edge: ask the child to scroll, so the drag keeps going past
    // what one screen holds. The rows it reveals are stitched on the next
    // frame the screen is settled.
    let past_top = me.row < tile.y;
    let past_bottom = me.row >= tile.y + tile.height;
    s.drag_edge = if past_top {
        Some(true)
    } else if past_bottom {
        Some(false)
    } else {
        None
    };

    if let Some(buf) = &s.copy
        && let Some(line) = buf.line_at(row)
        && let Some(drag) = &mut s.drag
    {
        drag.tip = (line, col);
    }
    app.needs_redraw = true;
}

/// A wheel event for the child, in whichever encoding it turned on. `None`
/// when it is not reading the mouse at all, in which case there is nothing to
/// ask it to scroll.
fn wheel_sequence(s: &session::Session, up: bool, tile: ratatui::layout::Rect) -> Option<Vec<u8>> {
    use alacritty_terminal::term::TermMode;
    let mode = s.parser.lock().ok().map(|p| *p.term.mode())?;
    let col = tile.width / 2 + 1;
    let row = tile.height / 2 + 1;
    if mode.intersects(TermMode::SGR_MOUSE) {
        let btn = if up { 64 } else { 65 };
        return Some(format!("\x1b[<{};{};{}M", btn, col, row).into_bytes());
    }
    if mode.intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION) {
        let btn = if up { 64u8 } else { 65u8 };
        return Some(vec![
            0x1b,
            b'[',
            b'M',
            btn + 32,
            (col as u8).saturating_add(32),
            (row as u8).saturating_add(32),
        ]);
    }
    None
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
        // Releasing stops the edge scroll, whatever the pointer does next.
        s.drag_edge = None;
    }
    let Some(s) = app.sessions.get(app.focus) else {
        return;
    };
    // The buffer is the authority: it holds everything the drag scrolled
    // through, not just the screen that happens to be up.
    let text = match (&s.copy, &s.drag) {
        (Some(buf), Some(drag)) if drag.anchor != drag.tip => buf.text_range(drag.anchor, drag.tip),
        _ => return,
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

/// Rows and columns a focused tile gets at the current terminal size. The
/// draw pass reports the real inner size back, so this is what a session is
/// told before its first frame.
fn resize_all(app: &mut App) {
    let (main_rows, main_cols) = app.tile_size();
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
        app.mode = Mode::Picker(Box::new(PickerState::new()));
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

fn handle_picker(app: &mut App, mut state: Box<PickerState>, key: KeyEvent) -> Result<()> {
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
        state.request_preview();
        app.mode = Mode::Picker(state);
    } else if keys::PICKER_END.matches(&key) {
        state.selected = state.items.len().saturating_sub(1);
        state.request_preview();
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
                    // The new row in the sidebar says this already.
                    app.status.clear();
                    resize_all(app);
                    app.persist_dirty = true;
                }
                Err(e) => {
                    app.status = format!("resume failed: {e:#}");
                }
            }
        } else if state.scanning {
            app.mode = Mode::Picker(state);
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
                // The new row in the sidebar says this already.
                app.status.clear();
                resize_all(app);
                app.persist_dirty = true;
            }
            Err(e) => {
                app.status = format!("spawn failed: {e:#}");
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
#[path = "tests/main.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/main_keys.rs"]
mod key_tests;
