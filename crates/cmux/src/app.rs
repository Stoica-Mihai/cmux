use anyhow::Result;
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::connect_mode::{DaemonHandle, SpawnMailbox};
use crate::session::Session;

pub enum Mode {
    Dashboard,
    Spawn(SpawnState),
    Rename(RenameState),
    Picker(PickerState),
    ConfirmDetach(u64),
    Scrollback(u64),
    Help,
    Reorder,
}

pub struct RenameState {
    pub session_id: u64,
    pub buf: String,
}

pub struct PickerState {
    pub all: Vec<crate::transcripts::Transcript>,
    pub items: Vec<usize>,
    pub selected: usize,
    pub dangerous: bool,
    pub filter: String,
    pub previews: std::collections::HashMap<String, String>,
}

impl PickerState {
    pub fn new() -> Self {
        let all = crate::transcripts::scan();
        let items = (0..all.len()).collect();
        let mut s = Self {
            all,
            items,
            selected: 0,
            dangerous: false,
            filter: String::new(),
            previews: std::collections::HashMap::new(),
        };
        s.ensure_preview();
        s
    }
    pub fn current(&self) -> Option<&crate::transcripts::Transcript> {
        self.items.get(self.selected).and_then(|i| self.all.get(*i))
    }
    pub fn move_sel(&mut self, delta: i32) {
        self.selected = crate::util::wrap_index(self.selected, self.items.len(), delta);
        self.ensure_preview();
    }
    pub fn ensure_preview(&mut self) {
        let Some(t) = self.current() else { return };
        if self.previews.contains_key(&t.session_id) {
            return;
        }
        let id = t.session_id.clone();
        let path = dirs_path(&t.cwd, &id);
        let text = crate::transcripts::load_preview(&path, 40);
        self.previews.insert(id, text);
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
                    if let Some(name) = &t.custom_title
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
        self.ensure_preview();
    }
}

fn dirs_path(cwd: &Path, session_id: &str) -> PathBuf {
    crate::util::claude_projects_dir()
        .unwrap_or_default()
        .join(crate::transcripts::slug_encode(cwd))
        .join(format!("{}.jsonl", session_id))
}

pub struct SpawnState {
    pub cwd: PathBuf,
    pub entries: Vec<PathBuf>,
    pub selected: usize,
    pub dangerous: bool,
}

impl SpawnState {
    pub fn new(start: PathBuf) -> Self {
        let mut s = Self {
            cwd: start,
            entries: Vec::new(),
            selected: 0,
            dangerous: false,
        };
        s.refresh();
        s
    }

    pub fn refresh(&mut self) {
        self.entries = std::fs::read_dir(&self.cwd)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        self.entries.sort();
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    pub fn move_sel(&mut self, delta: i32) {
        self.selected = crate::util::wrap_index(self.selected, self.entries.len(), delta);
    }

    pub fn descend(&mut self) {
        if let Some(target) = self.entries.get(self.selected).cloned() {
            self.cwd = target;
            self.selected = 0;
            self.refresh();
        }
    }

    pub fn ascend(&mut self) {
        let came_from = self.cwd.file_name().map(|s| s.to_os_string());
        if let Some(p) = self.cwd.parent() {
            self.cwd = p.to_path_buf();
            self.selected = 0;
            self.refresh();
            if let Some(name) = came_from
                && let Some(idx) = self
                    .entries
                    .iter()
                    .position(|p| p.file_name() == Some(&name))
            {
                self.selected = idx;
            }
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
            daemon.pending_spawns.lock().unwrap().push_back(mb.clone());
            daemon.request(cmux_proto::Request::SpawnSession {
                cwd: cwd.clone(),
                cmd: cmux_proto::claude_command(dangerous, resume.as_deref()),
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
            // Subscribe so FrameDelta starts flowing for this session.
            daemon.request(cmux_proto::Request::Subscribe {
                session_id: info.id,
            })?;
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
        daemon.request(cmux_proto::Request::Subscribe {
            session_id: info.id,
        })?;
        daemon.request(cmux_proto::Request::Attach {
            session_id: info.id,
            want_history: true,
        })?;
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
mod tests {
    use super::*;
    use crate::session::Session;
    use std::sync::mpsc;

    fn daemon_session(id: u64, label: &str) -> Session {
        let (tx, _rx) = mpsc::channel();
        Session::new_daemon(
            id,
            label.into(),
            PathBuf::from("/tmp"),
            false,
            None,
            24,
            80,
            None,
            id,
            tx,
        )
        .0
    }

    fn app_with(n: u64) -> App {
        let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
        for i in 1..=n {
            app.sessions.push(daemon_session(i, &format!("s{i}")));
        }
        app
    }

    /// A scratch tree of directories, named so the caller can predict the sort.
    fn dir_tree(tag: &str, names: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cmux-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in names {
            std::fs::create_dir_all(root.join(name)).expect("mkdir");
        }
        root
    }

    #[test]
    fn cycling_focus_wraps_in_both_directions() {
        let mut app = app_with(3);
        app.focus = 0;
        app.cycle_focus(-1);
        assert_eq!(app.focus, 2, "going back from the first should wrap to last");
        app.cycle_focus(1);
        assert_eq!(app.focus, 0, "going on from the last should wrap to first");
    }

    #[test]
    fn cycling_focus_with_no_sessions_stays_put() {
        let mut app = app_with(0);
        app.cycle_focus(1);
        assert_eq!(app.focus, 0, "an empty list has nothing to focus");
    }

    #[test]
    fn detaching_the_last_tile_moves_focus_back_and_leaves_no_gap() {
        let mut app = app_with(3);
        app.focus = 2;
        app.detach_focused();
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.focus, 1, "focus should follow the list in, not dangle");
    }

    #[test]
    fn detaching_the_only_tile_returns_to_the_dashboard() {
        let mut app = app_with(1);
        app.focus = 0;
        app.mode = Mode::Scrollback(1);
        app.detach_focused();
        assert!(app.sessions.is_empty());
        assert!(
            matches!(app.mode, Mode::Dashboard),
            "with nothing left there is no session mode to stay in"
        );
    }

    #[test]
    fn a_new_tile_never_gets_a_degenerate_size() {
        // Small enough that the naive arithmetic would underflow to zero.
        let mut app = App::new(PathBuf::from("/tmp"), (4, 12));
        for n in 0..8 {
            let (rows, cols) = app.tile_size_for_new();
            assert!(rows >= 4, "tile {n} got {rows} rows");
            assert!(cols >= 10, "tile {n} got {cols} cols");
            app.sessions.push(daemon_session(n + 1, "s"));
        }
    }

    #[test]
    fn tiles_shrink_as_the_grid_fills() {
        let mut app = App::new(PathBuf::from("/tmp"), (60, 200));
        let one = app.tile_size_for_new();
        for i in 1..=4 {
            app.sessions.push(daemon_session(i, "s"));
        }
        let five = app.tile_size_for_new();
        assert!(
            five.0 < one.0 && five.1 < one.1,
            "a fifth tile should be smaller than the first: {one:?} then {five:?}"
        );
    }

    #[test]
    fn the_directory_picker_lists_only_visible_directories() {
        let root = dir_tree("visible", &["alpha", "beta", ".hidden"]);
        std::fs::write(root.join("a-file"), b"x").expect("write");
        let state = SpawnState::new(root.clone());
        let names: Vec<String> = state
            .entries
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["alpha", "beta"], "got {names:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn descending_then_ascending_lands_back_on_the_directory_left() {
        let root = dir_tree("updown", &["alpha", "beta", "gamma"]);
        let mut state = SpawnState::new(root.clone());
        state.move_sel(1);
        let target = state.pick();
        state.descend();
        assert_eq!(state.cwd, target, "descend should enter the selection");
        state.ascend();
        assert_eq!(state.cwd, root);
        assert_eq!(
            state.pick(),
            target,
            "ascending should reselect the directory just left"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn picking_in_an_empty_directory_falls_back_to_that_directory() {
        let root = dir_tree("empty", &[]);
        std::fs::create_dir_all(&root).expect("mkdir");
        let state = SpawnState::new(root.clone());
        assert!(state.entries.is_empty());
        assert_eq!(state.pick(), root, "with nothing listed, pick the cwd");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn transcript(cwd: &str, title: Option<&str>, id: &str) -> crate::transcripts::Transcript {
        crate::transcripts::Transcript {
            session_id: id.to_string(),
            cwd: PathBuf::from(cwd),
            mtime: std::time::SystemTime::UNIX_EPOCH,
            file_size: 0,
            custom_title: title.map(str::to_string),
        }
    }

    fn picker_with(items: Vec<crate::transcripts::Transcript>) -> PickerState {
        let n = items.len();
        PickerState {
            all: items,
            items: (0..n).collect(),
            selected: 0,
            dangerous: false,
            filter: String::new(),
            previews: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn the_picker_filter_matches_path_and_title_case_insensitively() {
        let mut p = picker_with(vec![
            transcript("/home/u/Alpha", None, "a"),
            transcript("/home/u/beta", Some("Gamma work"), "b"),
            transcript("/home/u/delta", None, "c"),
        ]);

        p.filter = "ALPHA".into();
        p.apply_filter();
        assert_eq!(p.items, vec![0], "path match should ignore case");

        p.filter = "gamma".into();
        p.apply_filter();
        assert_eq!(p.items, vec![1], "the custom title should match too");

        p.filter = "nothing-here".into();
        p.apply_filter();
        assert!(p.items.is_empty());
    }

    #[test]
    fn clearing_the_picker_filter_restores_every_item() {
        let mut p = picker_with(vec![
            transcript("/a", None, "a"),
            transcript("/b", None, "b"),
        ]);
        p.filter = "a".into();
        p.apply_filter();
        assert_eq!(p.items, vec![0]);
        p.filter.clear();
        p.apply_filter();
        assert_eq!(p.items, vec![0, 1]);
    }

    /// Filtering down to fewer items than the current selection index used to
    /// leave `selected` pointing past the end.
    #[test]
    fn filtering_pulls_the_selection_back_into_range() {
        let mut p = picker_with(vec![
            transcript("/a", None, "a"),
            transcript("/b", None, "b"),
            transcript("/c", None, "c"),
        ]);
        p.selected = 2;
        p.filter = "/a".into();
        p.apply_filter();
        assert!(
            p.selected < p.items.len().max(1),
            "selected {} is past the {} remaining items",
            p.selected,
            p.items.len()
        );
        assert!(p.current().is_some(), "the selection should resolve");
    }

    #[test]
    fn an_empty_picker_has_nothing_current_and_survives_moving() {
        let mut p = picker_with(Vec::new());
        assert!(p.current().is_none());
        p.move_sel(1);
        p.move_sel(-1);
        assert!(p.current().is_none());
    }

    /// Detach must remove the tile *and* end the session. While it only
    /// dropped the handle, a daemon-hosted session stayed alive and kept
    /// showing up in every other client, the browser included.
    #[test]
    fn detaching_ends_the_session_on_the_daemon_too() {
        let (tx, rx) = mpsc::channel();
        let (sess, _slot) = Session::new_daemon(
            1,
            "s".into(),
            PathBuf::from("/tmp"),
            false,
            None,
            24,
            80,
            None,
            5,
            tx,
        );
        let mut app = App::new(PathBuf::from("/tmp"), (40, 120));
        app.sessions.push(sess);
        app.focus = 0;

        app.detach_focused();

        assert!(app.sessions.is_empty(), "the tile should be gone");
        match rx.try_recv() {
            Ok(cmux_proto::Request::Detach {
                session_id,
                keep_session,
            }) => {
                assert_eq!(session_id, 5);
                assert!(!keep_session, "detach must end it, not park it");
            }
            Ok(other) => panic!("expected Detach, got {other:?}"),
            Err(_) => panic!("the daemon was never told; the session leaks"),
        }
    }
}
