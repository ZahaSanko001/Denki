//! Shared application core used by the desktop and mobile Tauri shells.
//!
//! Platform-specific UI, file pickers, and operating-system media sessions
//! belong in the Tauri layer. Playback, library metadata, and persisted player
//! state are exposed here through one stable crate boundary.

pub use library_core::{Track, read_track_from_file, scan_folder, scan_paths};
pub use player_core::{AudioSource, PlaybackStatus, PlayerError, PlayerHandle, Status};

pub mod state {
    use serde::{Deserialize, Serialize};
    use std::path::Path;

    #[derive(Serialize, Deserialize, Default, Clone)]
    pub struct AppState {
        pub last_path: Option<String>,
        #[serde(default)]
        pub last_paths: Vec<String>,
        pub last_index: Option<usize>,
        pub last_position_secs: Option<f64>,
        pub volume: Option<f32>,
    }

    pub fn load_state(path: &Path) -> AppState {
        let Ok(text) = std::fs::read_to_string(&path) else {
            return AppState::default();
        };
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn save_state(path: &Path, state: &AppState) {
        if let (Some(parent), Ok(text)) = (path.parent(), toml::to_string_pretty(state)) {
            if std::fs::create_dir_all(parent).is_ok() {
                let _ = std::fs::write(path, text);
            }
        }
    }
}
