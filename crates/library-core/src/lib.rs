use std::path::Path;

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Track {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: f64,
}

const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "ogg", "m4a", "aac"];

pub fn scan_paths(paths: &[std::path::PathBuf]) -> Vec<Track> {
    let mut tracks = Vec::new();
    for path in paths {
        if path.is_dir() {
            tracks.extend(scan_folder(path));
        } else if let Some(track) = read_track(path) {
            tracks.push(track);
        }
    }
    tracks
}

pub fn scan_folder(folder: &Path) -> Vec<Track> {
    walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(ext)))
                .unwrap_or(false)
        })
        .filter_map(|e| read_track(e.path()))
        .collect()
}

fn read_track(path: &Path) -> Option<Track> {
    let tagged_file = lofty::read_from_path(path).ok()?;
    let duration_secs = tagged_file.properties().duration().as_secs_f64();
    let tag = tagged_file.primary_tag();

    let fallback_title = || {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string()
    };

    Some(Track {
        path: path.to_string_lossy().to_string(),
        title: tag.and_then(|t| t.title()).map(|s| s.to_string()).unwrap_or_else(fallback_title),
        artist: tag.and_then(|t| t.artist()).map(|s| s.to_string()).unwrap_or_else(|| "Unknown Artist".into()),
        album: tag.and_then(|t| t.album()).map(|s| s.to_string()).unwrap_or_else(|| "Unknown Album".into()),
        duration_secs,
    })
}