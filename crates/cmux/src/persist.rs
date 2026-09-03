use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::Session;

#[derive(Serialize, Deserialize, Clone)]
pub struct PersistedSession {
    pub cwd: PathBuf,
    pub label: String,
    #[serde(default)]
    pub dangerous: bool,
    #[serde(default)]
    pub resume_id: Option<String>,
    #[serde(default)]
    pub manually_renamed: bool,
}

impl From<&Session> for PersistedSession {
    fn from(s: &Session) -> Self {
        Self {
            cwd: s.cwd.clone(),
            label: s.label.clone(),
            dangerous: s.dangerous,
            resume_id: s.resume_id.clone(),
            manually_renamed: s.manually_renamed,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct PersistedState {
    #[serde(default)]
    pub sessions: Vec<PersistedSession>,
    #[serde(default = "default_sidebar")]
    pub show_sidebar: bool,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            show_sidebar: true,
        }
    }
}

fn default_sidebar() -> bool {
    true
}

fn state_path() -> Option<PathBuf> {
    crate::util::config_dir().map(|d| d.join("state.json"))
}

pub fn load() -> PersistedState {
    let Some(path) = state_path() else {
        return PersistedState::default();
    };
    load_from(&path)
}

/// Read one state file. Split from `load` so tests can pass a temp path.
fn load_from(path: &Path) -> PersistedState {
    let Ok(bytes) = std::fs::read(path) else {
        return PersistedState::default();
    };
    let mut state: PersistedState = serde_json::from_slice(&bytes).unwrap_or_default();
    for s in state.sessions.iter_mut() {
        s.label = s
            .label
            .chars()
            .filter(|c| *c != '↺')
            .collect::<String>()
            .trim()
            .to_string();
    }
    state
}

pub fn save(state: &PersistedState) {
    let Some(path) = state_path() else { return };
    save_to(&path, state);
}

/// Write one state file, creating its parent. Split from `save` likewise.
fn save_to(path: &Path, state: &PersistedState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(bytes) = serde_json::to_vec_pretty(state) else {
        return;
    };
    let _ = std::fs::write(path, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;

    static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

    /// A temp directory that deletes itself, so a panicking test leaves nothing.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("cmux-persist-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Build a daemon-backed Session, which needs no pty and no child process.
    fn daemon_session(label: &str) -> Session {
        let (tx, _rx) = mpsc::channel();
        let (s, _slot) = Session::new_daemon(
            1,
            label.to_string(),
            PathBuf::from("/home/u/proj"),
            true,
            Some("resume-abc".to_string()),
            24,
            80,
            None,
            9,
            tx,
            0,
        );
        s
    }

    fn sample_session() -> PersistedSession {
        PersistedSession {
            cwd: PathBuf::from("/home/u/proj"),
            label: "work".to_string(),
            dangerous: true,
            resume_id: Some("abc-123".to_string()),
            manually_renamed: true,
        }
    }

    #[test]
    fn a_fresh_state_has_no_sessions_and_shows_the_sidebar() {
        let d = PersistedState::default();
        assert!(
            d.sessions.is_empty(),
            "a fresh state should list no sessions, got {}",
            d.sessions.len()
        );
        assert!(d.show_sidebar, "a fresh state should show the sidebar");
        assert!(
            default_sidebar(),
            "default_sidebar is the serde fallback for show_sidebar and must agree with Default"
        );
    }

    #[test]
    fn a_state_round_trips_through_json() {
        let state = PersistedState {
            sessions: vec![sample_session()],
            show_sidebar: false,
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let back: PersistedState = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.sessions.len(), 1, "session list lost an entry");
        assert_eq!(back.sessions[0].cwd, state.sessions[0].cwd, "cwd changed");
        assert_eq!(
            back.sessions[0].label, state.sessions[0].label,
            "label changed"
        );
        assert!(back.sessions[0].dangerous, "dangerous flag was dropped");
        assert_eq!(
            back.sessions[0].resume_id,
            Some("abc-123".to_string()),
            "resume_id changed"
        );
        assert!(
            back.sessions[0].manually_renamed,
            "manually_renamed was dropped"
        );
        assert!(
            !back.show_sidebar,
            "show_sidebar=false was not preserved; the serde default silently turned it back on"
        );
    }

    #[test]
    fn absent_json_fields_fall_back_to_their_defaults() {
        let state: PersistedState =
            serde_json::from_str(r#"{"sessions":[{"cwd":"/x","label":"l"}]}"#)
                .expect("a state written by an older build must still load");

        assert!(
            state.show_sidebar,
            "a state file with no show_sidebar key should show the sidebar"
        );
        assert!(
            !state.sessions[0].dangerous,
            "a session with no dangerous key should not be dangerous"
        );
        assert_eq!(
            state.sessions[0].resume_id, None,
            "a session with no resume_id key should have none"
        );
        assert!(
            !state.sessions[0].manually_renamed,
            "a session with no manually_renamed key should not be marked renamed"
        );
    }

    #[test]
    fn an_empty_json_object_loads_as_the_default_state() {
        let state: PersistedState = serde_json::from_str("{}").expect("deserialize");
        assert!(
            state.sessions.is_empty(),
            "sessions should default to empty"
        );
        assert!(state.show_sidebar, "show_sidebar should default to true");
    }

    #[test]
    fn a_session_converts_to_its_persisted_form() {
        let mut s = daemon_session("build");
        s.manually_renamed = true;
        let p = PersistedSession::from(&s);

        assert_eq!(p.cwd, PathBuf::from("/home/u/proj"), "cwd was not carried");
        assert_eq!(p.label, "build", "label was not carried");
        assert!(p.dangerous, "dangerous flag was not carried");
        assert_eq!(
            p.resume_id,
            Some("resume-abc".to_string()),
            "resume_id was not carried"
        );
        assert!(p.manually_renamed, "manually_renamed was not carried");
    }

    #[test]
    fn a_session_that_was_never_renamed_converts_with_the_flag_clear() {
        let s = daemon_session("build");
        let p = PersistedSession::from(&s);
        assert!(
            !p.manually_renamed,
            "manually_renamed must track the session, not be hard-coded"
        );
    }

    #[test]
    fn load_from_returns_the_default_when_the_file_is_absent() {
        let dir = TempDir::new("absent");
        let state = load_from(&dir.join("state.json"));
        assert!(
            state.sessions.is_empty(),
            "a missing state file should load as the default, got {} sessions",
            state.sessions.len()
        );
        assert!(
            state.show_sidebar,
            "a missing state file should load with the sidebar shown"
        );
    }

    #[test]
    fn load_from_returns_the_default_when_the_file_is_empty() {
        let dir = TempDir::new("empty");
        let path = dir.join("state.json");
        std::fs::write(&path, b"").expect("write fixture");

        let state = load_from(&path);
        assert!(
            state.sessions.is_empty(),
            "an empty state file should load as the default, got {} sessions",
            state.sessions.len()
        );
        assert!(
            state.show_sidebar,
            "an empty state file should load with the sidebar shown"
        );
    }

    #[test]
    fn load_from_returns_the_default_when_the_json_is_malformed() {
        let dir = TempDir::new("malformed");
        let path = dir.join("state.json");
        std::fs::write(&path, b"{\"sessions\": [ this is not json").expect("write fixture");

        let state = load_from(&path);
        assert!(
            state.sessions.is_empty(),
            "a truncated state file should load as the default rather than panicking"
        );
        assert!(
            state.show_sidebar,
            "a truncated state file should load with the sidebar shown"
        );
    }

    #[test]
    fn load_from_returns_the_default_when_the_json_has_the_wrong_shape() {
        let dir = TempDir::new("wrongshape");
        let path = dir.join("state.json");
        std::fs::write(&path, b"[1, 2, 3]").expect("write fixture");

        let state = load_from(&path);
        assert!(
            state.sessions.is_empty(),
            "a JSON array where an object was expected should load as the default"
        );
        assert!(
            state.show_sidebar,
            "a wrong-shaped state file should load with the sidebar shown"
        );
    }

    #[test]
    fn load_from_strips_the_resume_marker_and_surrounding_space_from_labels() {
        let dir = TempDir::new("marker");
        let path = dir.join("state.json");
        std::fs::write(
            &path,
            r#"{"sessions":[{"cwd":"/x","label":" ↺ cmux "}],"show_sidebar":true}"#.as_bytes(),
        )
        .expect("write fixture");

        let state = load_from(&path);
        assert_eq!(
            state.sessions[0].label, "cmux",
            "the resume marker and its padding should be stripped from a stored label"
        );
    }

    #[test]
    fn save_to_then_load_from_round_trips() {
        let dir = TempDir::new("roundtrip");
        let path = dir.join("state.json");
        let state = PersistedState {
            sessions: vec![sample_session()],
            show_sidebar: false,
        };
        save_to(&path, &state);

        let back = load_from(&path);
        assert_eq!(back.sessions.len(), 1, "saved session did not come back");
        assert_eq!(back.sessions[0].label, "work", "label did not come back");
        assert_eq!(
            back.sessions[0].cwd,
            PathBuf::from("/home/u/proj"),
            "cwd did not come back"
        );
        assert!(back.sessions[0].dangerous, "dangerous did not come back");
        assert!(
            !back.show_sidebar,
            "a hidden sidebar came back shown; the setting does not survive a restart"
        );
    }

    #[test]
    fn save_to_then_load_from_keeps_a_shown_sidebar_shown() {
        let dir = TempDir::new("sidebaron");
        let path = dir.join("state.json");
        save_to(&path, &PersistedState::default());
        assert!(
            load_from(&path).show_sidebar,
            "a shown sidebar came back hidden"
        );
    }

    #[test]
    fn save_to_creates_the_missing_parent_directory() {
        let dir = TempDir::new("parent");
        let path = dir.join("nested").join("deeper").join("state.json");
        save_to(&path, &PersistedState::default());
        assert!(
            path.exists(),
            "save_to should create {} and everything above it",
            path.display()
        );
    }

    #[test]
    fn save_to_overwrites_an_existing_file() {
        let dir = TempDir::new("overwrite");
        let path = dir.join("state.json");
        save_to(
            &path,
            &PersistedState {
                sessions: vec![sample_session(), sample_session()],
                show_sidebar: true,
            },
        );
        save_to(&path, &PersistedState::default());

        let back = load_from(&path);
        assert!(
            back.sessions.is_empty(),
            "the second save should replace the file, not append to it; got {} sessions",
            back.sessions.len()
        );
    }
}
