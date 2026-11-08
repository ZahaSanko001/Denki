use player_core::PlayerHandle;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tauri::Manager;
mod state;
mod mpris;

#[tauri::command]
async fn library_scan_paths(paths: Vec<String>) -> Result<Vec<library_core::Track>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths: Vec<std::path::PathBuf> = paths.into_iter().map(std::path::PathBuf::from).collect();
        library_core::scan_paths(&paths)
    })
    .await
    .map_err(|error| format!("Library scan task failed: {error}"))
}

#[tauri::command]
async fn download_audio(url: String, app: tauri::AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_audio_blocking(url, app))
        .await
        .map_err(|error| format!("Audio download task failed: {error}"))?
}

fn download_audio_blocking(url: String, app: tauri::AppHandle) -> Result<String, String> {
    let url = url.trim();
    let parsed = url::Url::parse(url).map_err(|_| "Enter a valid http or https URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http and https URLs are supported".into());
    }

    let music_dir = dirs::audio_dir().or_else(dirs::home_dir)
        .ok_or_else(|| "Could not find a music folder".to_string())?;
    let download_dir = music_dir.join("Denki Downloads");
    std::fs::create_dir_all(&download_dir).map_err(|e| format!("Could not create download folder: {e}"))?;
    let output_template = download_dir.join("%(title).200B [%(id)s].%(ext)s");
    let ytdlp = ensure_ytdlp(&app)?;

    let output = Command::new(ytdlp)
        .args(["--no-playlist", "--no-progress", "--no-continue", "--force-overwrites", "--format", "bestaudio[ext=m4a][protocol=http_dash_segments][acodec^=mp4a]/bestaudio[ext=m4a][acodec^=mp4a]/bestaudio[ext=mp3]/bestaudio[ext=flac]/bestaudio[ext=wav]/bestaudio[ext=ogg][acodec^=vorbis]/bestaudio[ext=aac]", "--print", "after_move:filepath", "--output"])
        .arg(output_template)
        .arg("--")
        .arg(url)
        .output()
        .map_err(|e| format!("Could not start the downloaded yt-dlp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = stderr.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(if details.is_empty() { format!("yt-dlp failed with {}", output.status) } else { details });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let path = stdout
        .lines()
        .last()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "yt-dlp finished but did not report the downloaded file".to_string())?
        .to_string();
    let size = fs::metadata(&path)
        .map_err(|e| format!("yt-dlp reported a file that could not be read: {e}"))?
        .len();
    if size == 0 {
        return Err("YouTube returned an empty audio stream. Try downloading again; Denki will retry from a fresh file.".into());
    }
    Ok(path)
}

fn ytdlp_asset_url() -> Result<&'static str, String> {
    #[cfg(target_os = "windows")]
    {
        #[cfg(target_arch = "x86_64")]
        return Ok("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe");
        #[cfg(target_arch = "aarch64")]
        return Ok("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_arm64.exe");
        #[allow(unreachable_code)]
        return Err("Automatic yt-dlp setup is not available for this Windows architecture".into());
    }
    #[cfg(target_os = "macos")]
    {
        return Ok("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos");
    }
    #[cfg(target_os = "linux")]
    {
        #[cfg(target_arch = "x86_64")]
        return Ok("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux");
        #[cfg(target_arch = "aarch64")]
        return Ok("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64");
        #[allow(unreachable_code)]
        return Err("Automatic yt-dlp setup is not available for this Linux architecture".into());
    }
    #[allow(unreachable_code)]
    Err("Automatic yt-dlp setup is not supported on this operating system".into())
}

fn ensure_ytdlp(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let tool_dir = app.path().app_data_dir()
        .map_err(|e| format!("Could not locate Denki's application data folder: {e}"))?
        .join("tools");
    fs::create_dir_all(&tool_dir).map_err(|e| format!("Could not create tool folder: {e}"))?;
    #[cfg(target_os = "windows")]
    let binary = tool_dir.join("yt-dlp.exe");
    #[cfg(not(target_os = "windows"))]
    let binary = tool_dir.join("yt-dlp");

    if binary.is_file() {
        return Ok(binary);
    }

    let url = ytdlp_asset_url()?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("Denki/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Could not prepare yt-dlp download: {e}"))?;
    let response = client.get(url).send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("Could not download yt-dlp. Check your internet connection and try again: {e}"))?;

    const MAX_TOOL_SIZE: u64 = 100 * 1024 * 1024;
    if response.content_length().is_some_and(|size| size > MAX_TOOL_SIZE) {
        return Err("The yt-dlp download was larger than expected".into());
    }
    let temporary = tool_dir.join("yt-dlp.download");
    let mut file = File::create(&temporary).map_err(|e| format!("Could not create yt-dlp file: {e}"))?;
    let copied = io::copy(&mut response.take(MAX_TOOL_SIZE + 1), &mut file)
        .map_err(|e| format!("Could not save yt-dlp: {e}"))?;
    if copied > MAX_TOOL_SIZE {
        let _ = fs::remove_file(&temporary);
        return Err("The yt-dlp download was larger than expected".into());
    }
    if copied == 0 {
        let _ = fs::remove_file(&temporary);
        return Err("The yt-dlp download was empty".into());
    }
    drop(file);
    fs::rename(&temporary, &binary).map_err(|e| format!("Could not install yt-dlp: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&binary).map_err(|e| format!("Could not inspect yt-dlp: {e}"))?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).map_err(|e| format!("Could not make yt-dlp executable: {e}"))?;
    }
    Ok(binary)
}

#[tauri::command]
fn player_toggle(handle: tauri::State<PlayerHandle>) -> Result<player_core::Status, String> {
    handle.toggle().map_err(|e| e.to_string())
}

#[tauri::command]
fn player_seek(seconds: f64, handle: tauri::State<PlayerHandle>) -> Result<(), String> {
    handle.seek(Duration::from_secs_f64(seconds)).map_err(|e| e.to_string())
}

#[tauri::command]
fn player_set_volume(volume: f32, handle: tauri::State<PlayerHandle>) {
    handle.set_volume(volume);
}

#[tauri::command]
fn player_status(handle: tauri::State<PlayerHandle>) -> player_core::Status {
    handle.status()
}

#[tauri::command]
fn queue_load(paths: Vec<String>, start_index: usize, autoplay: bool, handle: tauri::State<PlayerHandle>) -> Result<(), String> {
    let paths = paths.into_iter().map(PathBuf::from).collect();
    handle.load_queue(paths, start_index, autoplay).map_err(|e| e.to_string())
}

#[tauri::command]
fn queue_next(handle: tauri::State<PlayerHandle>) -> Result<(), String> {
    handle.next().map_err(|e| e.to_string())
}

#[tauri::command]
fn queue_prev(handle: tauri::State<PlayerHandle>) -> Result<(), String> {
    handle.prev().map_err(|e| e.to_string())
}

#[tauri::command]
fn state_load() -> state::AppState {
    state::load_state()
}

#[tauri::command]
fn state_save(last_path: Option<String>, last_index: Option<usize>, last_position_secs: Option<f64>, volume: Option<f32>) {
    state::save_state(&state::AppState { last_path, last_index, last_position_secs, volume });
}

#[tauri::command]
fn mpris_set_metadata(
    title: String,
    artist: String,
    duration_secs: Option<f64>,
    controls: tauri::State<Option<mpris::SharedControls>>,
) -> Result<(), String> {
    let shared = controls
        .as_ref()
        .ok_or_else(|| "MPRIS controls are unavailable".to_string())?;
    let mut controls = shared
        .lock()
        .map_err(|error| format!("could not lock MPRIS controls: {error}"))?;
    controls
        .set_metadata(souvlaki::MediaMetadata {
            title: Some(&title),
            artist: Some(&artist),
            duration: duration_secs.map(Duration::from_secs_f64),
            ..Default::default()
        })
        .map_err(|error| format!("could not set MPRIS metadata: {error:?}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let player_handle = PlayerHandle::spawn();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(player_handle.clone())
        .setup(move |app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                    width: 480.0,
                    height: 370.0,
                }));
                let _ = window.center();
                let _ = window.show();
            }

            #[cfg(target_os = "windows")]
            let hwnd: Option<*mut std::ffi::c_void> = {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                let window = app.get_webview_window("main").expect("no main window");
                let handle = window.window_handle().expect("failed to get window handle");
                match handle.as_raw() {
                    RawWindowHandle::Win32(h) => Some(isize::from(h.hwnd) as *mut std::ffi::c_void),
                    _ => None,
                }
            };
            #[cfg(not(target_os = "windows"))]
            let hwnd: Option<*mut std::ffi::c_void> = None;

            let mpris_controls = match mpris::init(player_handle.clone(), hwnd) {
                Ok(controls) => Some(controls),
                Err(error) => {
                    eprintln!("MPRIS unavailable: {error}");
                    None
                }
            };
            app.manage(mpris_controls);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            player_toggle,
            player_seek,
            player_set_volume,
            player_status,
            queue_load,
            queue_next,
            queue_prev,
            library_scan_paths,
            download_audio,
            state_load,
            state_save,
            mpris_set_metadata,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
