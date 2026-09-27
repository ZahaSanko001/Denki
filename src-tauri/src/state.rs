use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct AppState {
    pub last_path: Option<String>,
    pub last_index: Option<usize>,
    pub last_position_secs: Option<f64>,
    pub volume: Option<f32>,
}

fn state_file_path() -> Option<PathBuf> {
    let mut dir = dirs::config_dir()?;
    dir.push("denki");
    std::fs::create_dir_all(&dir).ok()?;
    dir.push("state.toml");
    Some(dir)
}

pub fn load_state() -> AppState {
    let Some(path) = state_file_path() else { return AppState::default(); };
    let Ok(text) = std::fs::read_to_string(&path) else { return AppState::default(); };
    toml::from_str(&text).unwrap_or_default()
}

pub fn save_state(state: &AppState) {
    if let Some(path) = state_file_path() {
        if let Ok(text) = toml::to_string_pretty(state) {
            let _ = std::fs::write(path, text);
        }
    }
}