use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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

#[derive(Serialize, Deserialize)]
pub struct PersistedState {
    #[serde(default)]
    pub sessions: Vec<PersistedSession>,
    #[serde(default = "default_sidebar")]
    pub show_sidebar: bool,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self { sessions: Vec::new(), show_sidebar: true }
    }
}

fn default_sidebar() -> bool { true }

fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("tmux-claude").join("state.json"))
}

pub fn load() -> PersistedState {
    let Some(path) = state_path() else { return PersistedState::default() };
    let Ok(bytes) = std::fs::read(&path) else { return PersistedState::default() };
    let mut state: PersistedState = serde_json::from_slice(&bytes).unwrap_or_default();
    for s in state.sessions.iter_mut() {
        s.label = s.label.chars().filter(|c| *c != '↺').collect::<String>().trim().to_string();
    }
    state
}

pub fn save(state: &PersistedState) {
    let Some(path) = state_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(bytes) = serde_json::to_vec_pretty(state) else { return };
    let _ = std::fs::write(&path, bytes);
}
