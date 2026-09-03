use anyhow::Result;
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use crate::connect_mode::{DaemonHandle, SpawnMailbox};
use crate::session::Session;

pub enum Mode {
    Dashboard,
    Spawn(SpawnState),
    Rename(RenameState),
    Picker(Box<PickerState>),
    ConfirmDetach(u64),
    Scrollback(u64),
    Help,
    Reorder,
}

pub struct RenameState {
    pub session_id: u64,
    pub buf: String,
}

/// Transcript lines shown in the preview pane.
const PREVIEW_LINES: usize = 40;

/// What the picker's background thread collects in one pass.
struct Scan {
    transcripts: Vec<crate::transcripts::Transcript>,
    background: std::collections::HashMap<String, crate::claude_sessions::Background>,
}

pub struct PickerState {
    pub all: Vec<crate::transcripts::Transcript>,
    pub items: Vec<usize>,
    pub selected: usize,
    pub dangerous: bool,
    pub filter: String,
    pub previews: std::collections::HashMap<String, String>,
    /// True from construction until the directory scan lands.
    pub scanning: bool,
    /// Live background sessions, keyed by session id.
    background: std::collections::HashMap<String, crate::claude_sessions::Background>,
    scan_rx: Receiver<Scan>,
    preview_tx: Sender<(String, PathBuf)>,
    preview_rx: Receiver<(String, String)>,
    requested: std::collections::HashSet<String>,
}

impl PickerState {
    /// Starts the directory scan and the preview reader on their own threads.
    /// Both end when the returned state is dropped.
    pub fn new() -> Self {
        let (scan_tx, scan_rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("picker-scan".into())
            .spawn(move || {
                let _ = scan_tx.send(Scan {
                    transcripts: crate::transcripts::scan(),
                    background: crate::claude_sessions::live_background(),
                });
            });

        let (preview_tx, req_rx) = mpsc::channel::<(String, PathBuf)>();
        let (res_tx, preview_rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("picker-preview".into())
            .spawn(move || {
                while let Ok((id, path)) = req_rx.recv() {
                    let text = crate::transcripts::load_preview(&path, PREVIEW_LINES);
                    if res_tx.send((id, text)).is_err() {
                        break;
                    }
                }
            });

        Self {
            all: Vec::new(),
            items: Vec::new(),
            selected: 0,
            dangerous: false,
            filter: String::new(),
            previews: std::collections::HashMap::new(),
            background: std::collections::HashMap::new(),
            scanning: true,
            scan_rx,
            preview_tx,
            preview_rx,
            requested: std::collections::HashSet::new(),
        }
    }

    /// Takes the scan result and any finished previews. Reports whether
    /// anything arrived.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if self.scanning {
            match self.scan_rx.try_recv() {
                Ok(scan) => {
                    self.all = scan.transcripts;
                    self.background = scan.background;
                    self.scanning = false;
                    self.apply_filter();
                    changed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scanning = false;
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        while let Ok((id, text)) = self.preview_rx.try_recv() {
            self.previews.insert(id, text);
            changed = true;
        }
        changed
    }

    pub fn current(&self) -> Option<&crate::transcripts::Transcript> {
        self.items.get(self.selected).and_then(|i| self.all.get(*i))
    }

    /// The short id claude is running this conversation under, for a
    /// conversation it is running in the background.
    pub fn running_as(&self, t: &crate::transcripts::Transcript) -> Option<&str> {
        self.background
            .get(&t.session_id)
            .map(|bg| bg.job_id.as_str())
    }

    /// The name to show for a conversation: claude's own, while it runs the
    /// session in the background, otherwise the transcript's `--name` title.
    /// A running fork carries its origin in claude's name, which the
    /// transcript title predates.
    pub fn display_name<'a>(&'a self, t: &'a crate::transcripts::Transcript) -> Option<&'a str> {
        self.background
            .get(&t.session_id)
            .and_then(|bg| bg.name.as_deref())
            .or(t.custom_title.as_deref())
    }

    /// The session a conversation was forked from. A transcript records its own
    /// origin; a background session's sits in claude's job state instead, and
    /// only one of the two is ever present.
    pub fn fork_origin<'a>(&'a self, t: &'a crate::transcripts::Transcript) -> Option<&'a str> {
        t.forked_from.as_deref().or_else(|| {
            self.background
                .get(&t.session_id)
                .and_then(|bg| bg.forked_from.as_deref())
        })
    }

    /// The origin of a forked conversation, named the way its own row is
    /// named, falling back to the leading digits of its session id.
    pub fn forked_from(&self, t: &crate::transcripts::Transcript) -> Option<String> {
        let parent_id = self.fork_origin(t)?;
        let parent = self.all.iter().find(|p| p.session_id == parent_id);
        let named = parent
            .and_then(|p| self.display_name(p))
            .map(str::to_string);
        Some(named.unwrap_or_else(|| crate::transcripts::short_id(parent_id).to_string()))
    }
    pub fn move_sel(&mut self, delta: i32) {
        self.selected = crate::util::wrap_index(self.selected, self.items.len(), delta);
        self.request_preview();
    }

    /// Queues the selected transcript's preview, once per session.
    pub fn request_preview(&mut self) {
        let Some((id, path)) = self
            .current()
            .map(|t| (t.session_id.clone(), t.path.clone()))
        else {
            return;
        };
        if self.previews.contains_key(&id) || !self.requested.insert(id.clone()) {
            return;
        }
        if self.preview_tx.send((id.clone(), path)).is_err() {
            self.previews
                .insert(id, "(preview unavailable)".to_string());
        }
    }
    pub fn apply_filter(&mut self) {
        let q = self.filter.to_lowercase();
        if q.is_empty() {
            self.items = (0..self.all.len()).collect();
        } else {
            self.items = self
                .all
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    if t.cwd.display().to_string().to_lowercase().contains(&q) {
                        return true;
                    }
                    if let Some(name) = self.display_name(t)
                        && name.to_lowercase().contains(&q)
                    {
                        return true;
                    }
                    false
                })
                .map(|(i, _)| i)
                .collect();
        }
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
        self.request_preview();
    }
}

/// The directories of one folder, read off the main thread.
struct Listing {
    cwd: PathBuf,
    entries: Vec<PathBuf>,
    /// Directory to select once the listing lands, for a step back up.
    select: Option<std::ffi::OsString>,
}

/// Visible sub-directories of `cwd`, sorted, dotfiles omitted.
fn read_dirs(cwd: &PathBuf) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(cwd)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    entries.sort();
    entries
}

pub struct SpawnState {
    pub cwd: PathBuf,
    pub entries: Vec<PathBuf>,
    pub selected: usize,
    pub dangerous: bool,
    /// True until the listing for `cwd` lands.
    pub reading: bool,
    req_tx: Sender<Listing>,
    res_rx: Receiver<Listing>,
}

impl SpawnState {
    /// Reads the starting folder on a worker thread. `read_dir` on the main
    /// thread stalled the whole TUI for as long as the folder took to list,
    /// which on a network mount or a huge directory is visible.
    pub fn new(start: PathBuf) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Listing>();
        let (res_tx, res_rx) = mpsc::channel::<Listing>();
        let _ = std::thread::Builder::new()
            .name("spawn-browser".into())
            .spawn(move || {
                while let Ok(mut job) = req_rx.recv() {
                    job.entries = read_dirs(&job.cwd);
                    if res_tx.send(job).is_err() {
                        break;
                    }
                }
            });

        let mut s = Self {
            cwd: start,
            entries: Vec::new(),
            selected: 0,
            dangerous: false,
            reading: false,
            req_tx,
            res_rx,
        };
        s.request(s.cwd.clone(), None);
        s
    }

    /// Queue a folder for the worker. A dead worker falls back to reading it
    /// here, so the browser still works rather than showing nothing.
    fn request(&mut self, cwd: PathBuf, select: Option<std::ffi::OsString>) {
        let job = Listing {
            cwd: cwd.clone(),
            entries: Vec::new(),
            select: select.clone(),
        };
        if self.req_tx.send(job).is_err() {
            self.entries = read_dirs(&cwd);
            self.settle(select);
            return;
        }
        self.entries.clear();
        self.selected = 0;
        self.reading = true;
    }

    /// Take a finished listing. Reports whether anything arrived.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(job) = self.res_rx.try_recv() {
            // A listing for a folder already stepped away from is stale.
            if job.cwd != self.cwd {
                continue;
            }
            self.entries = job.entries;
            self.reading = false;
            self.settle(job.select);
            changed = true;
        }
        changed
    }

    /// Put the cursor on the folder just stepped out of, or the first row.
    fn settle(&mut self, select: Option<std::ffi::OsString>) {
        self.reading = false;
        self.selected = select
            .and_then(|name| {
                self.entries
                    .iter()
                    .position(|p| p.file_name() == Some(&name))
            })
            .unwrap_or(0);
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    pub fn move_sel(&mut self, delta: i32) {
        self.selected = crate::util::wrap_index(self.selected, self.entries.len(), delta);
    }

    pub fn descend(&mut self) {
        if let Some(target) = self.entries.get(self.selected).cloned() {
            self.cwd = target.clone();
            self.request(target, None);
        }
    }

    pub fn ascend(&mut self) {
        let came_from = self.cwd.file_name().map(|s| s.to_os_string());
        if let Some(p) = self.cwd.parent().map(|p| p.to_path_buf()) {
            self.cwd = p.clone();
            self.request(p, came_from);
        }
    }

    pub fn pick(&self) -> PathBuf {
        self.entries
            .get(self.selected)
            .cloned()
            .unwrap_or_else(|| self.cwd.clone())
    }
}

pub struct App {
    pub sessions: Vec<Session>,
    pub focus: usize,
    pub mode: Mode,
    pub next_id: u64,
    pub default_cwd: PathBuf,
    /// Path tokens already resolved against the filesystem, so the link pass
    /// does not re-check the same screen every frame.
    pub file_links: crate::file_links::Cache,
    pub term_size: (u16, u16),
    pub should_quit: bool,
    pub status: String,
    pub prefix_pending: bool,
    pub show_sidebar: bool,
    pub needs_redraw: bool,
    pub persist_dirty: bool,
    pub last_tile_area: Option<Rect>,
    pub render_tick: u64,
    pub toast: Option<Toast>,
    pub daemon: Option<Arc<DaemonHandle>>,
    pub daemon_lost: bool,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub expires_at_ms: u64,
}

impl App {
    pub fn new(default_cwd: PathBuf, term_size: (u16, u16)) -> Self {
        Self {
            sessions: Vec::new(),
            focus: 0,
            mode: Mode::Dashboard,
            next_id: 1,
            default_cwd,
            file_links: crate::file_links::Cache::default(),
            term_size,
            should_quit: false,
            status: String::new(),
            prefix_pending: false,
            show_sidebar: true,
            needs_redraw: true,
            persist_dirty: false,
            last_tile_area: None,
            render_tick: 0,
            toast: None,
            daemon: None,
            daemon_lost: false,
        }
    }

    pub fn spawn_session(&mut self, cwd: PathBuf, dangerous: bool) -> Result<()> {
        self.spawn_session_inner(cwd, dangerous, None, None)
    }

    pub fn spawn_resume(
        &mut self,
        cwd: PathBuf,
        dangerous: bool,
        session_id: String,
    ) -> Result<()> {
        self.spawn_session_inner(cwd, dangerous, Some(session_id), None)
    }

    /// Respawn a saved session, carrying its label in the spawn itself.
    /// Renaming it afterwards would mark it manually renamed on the daemon,
    /// which stops the status probe ever updating the name again — the TUI
    /// would follow the name the child picks while the daemon and the browser
    /// kept the old one.
    pub fn restore_session(
        &mut self,
        cwd: PathBuf,
        dangerous: bool,
        resume: Option<String>,
        label: Option<String>,
    ) -> Result<()> {
        self.spawn_session_inner(cwd, dangerous, resume, label)
    }

    fn spawn_session_inner(
        &mut self,
        cwd: PathBuf,
        dangerous: bool,
        resume: Option<String>,
        label: Option<String>,
    ) -> Result<()> {
        let label = label.filter(|l| !l.is_empty()).unwrap_or_else(|| {
            cwd.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| cwd.display().to_string())
        });
        let (rows, cols) = self.tile_size_for_new();
        let id = self.next_id;
        self.next_id += 1;

        let session = if let Some(daemon) = self.daemon.clone() {
            // Daemon-backed spawn: queue a mailbox, send Request::SpawnSession,
            // block on the mailbox for the SessionSpawned info.
            let mb = SpawnMailbox::new();
            daemon
                .pending_spawns
                .lock()
                .map_err(|_| anyhow::anyhow!("spawn mailbox lock poisoned"))?
                .push_back(mb.clone());
            daemon.request(cmux_proto::Request::SpawnSession {
                cwd: cwd.clone(),
                cmd: crate::claude_sessions::open_command(dangerous, resume.as_deref()),
                probe: cmux_proto::ProbeKind::Claude {
                    dangerous,
                    resume_id: resume.clone(),
                },
                label: Some(label.clone()),
                rows,
                cols,
            })?;
            let info = mb
                .wait(5_000)
                .ok_or_else(|| anyhow::anyhow!("daemon did not respond to SpawnSession"))?;
            let spawned_dangerous = info.probe.dangerous();
            let spawned_resume = info.probe.resume_id().map(str::to_string);
            let (sess, slot) = Session::new_daemon(
                id,
                info.label,
                info.cwd,
                spawned_dangerous,
                spawned_resume,
                rows,
                cols,
                None,
                info.id,
                daemon.req_tx.clone(),
            );
            daemon.register_slot(info.id, slot);
            // Subscribe so FrameDelta starts flowing for this session. A
            // failure here used to return with the daemon holding a spawned
            // session and this client holding a slot for it, but no row in
            // the sidebar to reach either from.
            if let Err(e) = daemon.request(cmux_proto::Request::Subscribe {
                session_id: info.id,
            }) {
                daemon.forget_slot(info.id);
                return Err(e);
            }
            sess
        } else {
            Session::spawn(id, label, cwd, dangerous, rows, cols, resume)?
        };

        self.sessions.push(session);
        self.focus = self.sessions.len() - 1;
        Ok(())
    }

    /// Construct a daemon-backed Session for an existing daemon session
    /// (returned by `ListSessions` on connect). Subscribes for FrameDelta
    /// streaming and queues an Attach to drain the replay ring.
    pub fn adopt_daemon_session(
        &mut self,
        info: cmux_proto::SessionInfo,
        daemon: &Arc<DaemonHandle>,
        rows: u16,
        cols: u16,
    ) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        let dangerous = info.probe.dangerous();
        let resume_id = info.probe.resume_id().map(str::to_string);
        let (sess, slot) = Session::new_daemon(
            id,
            info.label,
            info.cwd,
            dangerous,
            resume_id,
            rows,
            cols,
            None,
            info.id,
            daemon.req_tx.clone(),
        );
        daemon.register_slot(info.id, slot);
        let wired = daemon
            .request(cmux_proto::Request::Subscribe {
                session_id: info.id,
            })
            .and_then(|()| {
                daemon.request(cmux_proto::Request::Attach {
                    session_id: info.id,
                    want_history: true,
                })
            });
        if let Err(e) = wired {
            daemon.forget_slot(info.id);
            return Err(e);
        }
        self.sessions.push(sess);
        Ok(())
    }

    fn tile_size_for_new(&self) -> (u16, u16) {
        let n = (self.sessions.len() + 1) as u16;
        let cols_grid = (n as f32).sqrt().ceil() as u16;
        let rows_grid = n.div_ceil(cols_grid);
        let (term_rows, term_cols) = self.term_size;
        let body_rows = term_rows.saturating_sub(2);
        let rows = (body_rows / rows_grid.max(1)).saturating_sub(2).max(4);
        let cols = (term_cols / cols_grid.max(1)).saturating_sub(2).max(10);
        (rows, cols)
    }

    pub fn detach_focused(&mut self) {
        if self.focus < self.sessions.len() {
            // End the session, not just this view of it. Dropping the handle
            // kills a local PTY but leaves a daemon-hosted one running, where
            // it stays visible to every other client and to the browser.
            self.sessions[self.focus].kill();
            self.sessions.remove(self.focus);
            if self.focus >= self.sessions.len() && !self.sessions.is_empty() {
                self.focus = self.sessions.len() - 1;
            }
        }
        if self.sessions.is_empty() {
            self.mode = Mode::Dashboard;
        }
    }

    pub fn cycle_focus(&mut self, delta: i32) {
        self.focus = crate::util::wrap_index(self.focus, self.sessions.len(), delta);
    }

    pub fn reap_dead(&mut self) {
        for s in self.sessions.iter_mut() {
            let _ = s.is_alive();
            s.poll_status();
        }
    }
}

#[cfg(test)]
#[path = "tests/app.rs"]
mod tests;
