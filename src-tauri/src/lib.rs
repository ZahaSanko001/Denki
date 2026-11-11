#[cfg(not(target_os = "android"))]
use denki_core::scan_paths;
#[cfg(target_os = "android")]
use denki_core::AudioSource;
use denki_core::PlayerHandle;
#[cfg(not(target_os = "android"))]
use std::fs::{self, File};
#[cfg(not(target_os = "android"))]
use std::io::{self, Read};
use std::path::PathBuf;
#[cfg(not(target_os = "android"))]
use std::process::Command;
use std::time::Duration;
use tauri::Manager;
#[cfg(not(target_os = "android"))]
mod mpris;
#[cfg(target_os = "android")]
use tauri_plugin_android_fs::{AndroidFsExt, Entry as AndroidEntry};
#[cfg(target_os = "android")]
use tauri_plugin_fs::{FilePath as FsFilePath, FsExt, OpenOptions as FsOpenOptions};

#[tauri::command]
#[cfg(not(target_os = "android"))]
async fn library_scan_paths(paths: Vec<String>) -> Result<Vec<denki_core::Track>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths: Vec<std::path::PathBuf> =
            paths.into_iter().map(std::path::PathBuf::from).collect();
        scan_paths(&paths)
    })
    .await
    .map_err(|error| format!("Library scan task failed: {error}"))
}

#[cfg(target_os = "android")]
#[tauri::command]
async fn library_scan_paths(
    paths: Vec<String>,
    app: tauri::AppHandle,
) -> Result<Vec<denki_core::Track>, String> {
    scan_android_uris(
        paths.into_iter().map(|uri| (uri.clone(), uri)).collect(),
        app,
    )
    .await
}

#[cfg(target_os = "android")]
#[tauri::command]
async fn android_pick_files(app: tauri::AppHandle) -> Result<Vec<denki_core::Track>, String> {
    let android_fs = app.android_fs_async();
    let files = android_fs
        .picker()
        .pick_files(None, &["audio/*"], false)
        .await
        .map_err(|error| format!("Could not open the audio picker: {error}"))?;
    let mut uris = Vec::with_capacity(files.len());
    for uri in files {
        android_fs
            .picker()
            .persist_uri_permission(&uri)
            .await
            .map_err(|error| {
                format!("Could not retain access to a selected audio file: {error}")
            })?;
        let name = match android_fs.get_name(&uri).await {
            Ok(name) => name,
            Err(_) => android_fs.get_name_or_last_path_segment(&uri).await,
        };
        uris.push((uri.uri, name));
    }
    scan_android_uris(uris, app).await
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
async fn android_pick_files() -> Result<Vec<denki_core::Track>, String> {
    Err(
        "Audio picking through the Android Storage Access Framework is only available on Android"
            .into(),
    )
}

#[cfg(target_os = "android")]
#[tauri::command]
async fn android_pick_folder(app: tauri::AppHandle) -> Result<Vec<denki_core::Track>, String> {
    let android_fs = app.android_fs_async();
    let folder = android_fs
        .picker()
        .pick_dir(None, false)
        .await
        .map_err(|error| format!("Could not open the folder picker: {error}"))?;
    let Some(folder) = folder else {
        return Ok(Vec::new());
    };
    android_fs
        .picker()
        .persist_uri_permission(&folder)
        .await
        .map_err(|error| format!("Could not retain access to the selected folder: {error}"))?;

    let mut folders = vec![folder];
    let mut files = Vec::new();
    while let Some(folder) = folders.pop() {
        let entries = android_fs
            .read_dir(&folder)
            .await
            .map_err(|error| format!("Could not scan a selected folder: {error}"))?;
        for entry in entries {
            match entry {
                AndroidEntry::File { uri, name, .. } if is_supported_audio_name(&name) => {
                    files.push((uri.uri.clone(), name));
                }
                AndroidEntry::Dir { uri, .. } => folders.push(uri),
                AndroidEntry::File { .. } => {}
            }
        }
    }

    scan_android_uris(files, app).await
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
async fn android_pick_folder() -> Result<Vec<denki_core::Track>, String> {
    Err(
        "Folder picking through the Android Storage Access Framework is only available on Android"
            .into(),
    )
}

#[cfg(target_os = "android")]
async fn scan_android_uris(
    files: Vec<(String, String)>,
    app: tauri::AppHandle,
) -> Result<Vec<denki_core::Track>, String> {
    let android_fs = app.android_fs_async();
    let mut named_files = Vec::with_capacity(files.len());
    for (uri, fallback_name) in files {
        let name = if fallback_name == uri {
            let fs_uri = tauri_plugin_android_fs::FsUri::from_uri(uri.clone());
            match android_fs.get_name(&fs_uri).await {
                Ok(name) => name,
                Err(_) => android_fs.get_name_or_last_path_segment(&fs_uri).await,
            }
        } else {
            fallback_name
        };
        named_files.push((uri, audio_fallback_title(&name)));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut tracks = Vec::new();
        for (uri, name) in named_files {
            let file_path = uri
                .parse::<FsFilePath>()
                .expect("FilePath parsing is infallible");
            let file = app
                .fs()
                .open(file_path, FsOpenOptions::new())
                .map_err(|error| format!("Could not open selected audio: {error}"))?;
            if let Some(track) = denki_core::read_track_from_file(file, uri, name) {
                tracks.push(track);
            }
        }
        Ok(tracks)
    })
    .await
    .map_err(|error| format!("Library scan task failed: {error}"))?
}

#[cfg(target_os = "android")]
fn is_supported_audio_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac"
            )
        })
        .unwrap_or(false)
}

#[cfg(target_os = "android")]
fn audio_fallback_title(name: &str) -> String {
    name.rsplit_once('.')
        .filter(|(_, extension)| is_supported_audio_name(&format!("audio.{extension}")))
        .map(|(stem, _)| stem.to_string())
        .unwrap_or_else(|| name.to_string())
}

#[tauri::command]
#[cfg(not(target_os = "android"))]
async fn download_audio(url: String, app: tauri::AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_audio_blocking(url, app))
        .await
        .map_err(|error| format!("Audio download task failed: {error}"))?
}

#[cfg(not(target_os = "android"))]
fn download_audio_blocking(url: String, app: tauri::AppHandle) -> Result<String, String> {
    let url = url.trim();
    let parsed = url::Url::parse(url).map_err(|_| "Enter a valid http or https URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http and https URLs are supported".into());
    }

    let music_dir = dirs::audio_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "Could not find a music folder".to_string())?;
    let download_dir = music_dir.join("Denki Downloads");
    std::fs::create_dir_all(&download_dir)
        .map_err(|e| format!("Could not create download folder: {e}"))?;
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
        let details = stderr
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(if details.is_empty() {
            format!("yt-dlp failed with {}", output.status)
        } else {
            details
        });
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

#[cfg(not(target_os = "android"))]
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
        return Ok(
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64",
        );
        #[allow(unreachable_code)]
        return Err("Automatic yt-dlp setup is not available for this Linux architecture".into());
    }
    #[allow(unreachable_code)]
    Err("Automatic yt-dlp setup is not supported on this operating system".into())
}

#[cfg(not(target_os = "android"))]
fn ensure_ytdlp(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let tool_dir = app
        .path()
        .app_data_dir()
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
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| {
            format!("Could not download yt-dlp. Check your internet connection and try again: {e}")
        })?;

    const MAX_TOOL_SIZE: u64 = 100 * 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_TOOL_SIZE)
    {
        return Err("The yt-dlp download was larger than expected".into());
    }
    let temporary = tool_dir.join("yt-dlp.download");
    let mut file =
        File::create(&temporary).map_err(|e| format!("Could not create yt-dlp file: {e}"))?;
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
        let mut permissions = fs::metadata(&binary)
            .map_err(|e| format!("Could not inspect yt-dlp: {e}"))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions)
            .map_err(|e| format!("Could not make yt-dlp executable: {e}"))?;
    }
    Ok(binary)
}

#[cfg(target_os = "android")]
#[tauri::command]
async fn download_audio(url: String, app: tauri::AppHandle) -> Result<String, String> {
    #[derive(serde::Serialize)]
    struct DownloadRequest {
        url: String,
    }

    app.state::<AndroidYtDlp>()
        .0
        .run_mobile_plugin_async::<String>("download", DownloadRequest { url })
        .await
        .map_err(|error| format!("Android yt-dlp error: {error}"))
}

#[cfg(target_os = "android")]
struct AndroidYtDlp(tauri::plugin::PluginHandle<tauri::Wry>);

#[tauri::command]
fn player_toggle(handle: tauri::State<PlayerHandle>) -> Result<denki_core::Status, String> {
    handle.toggle().map_err(|e| e.to_string())
}

#[tauri::command]
fn player_seek(seconds: f64, handle: tauri::State<PlayerHandle>) -> Result<(), String> {
    handle
        .seek(Duration::from_secs_f64(seconds))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn player_set_volume(volume: f32, handle: tauri::State<PlayerHandle>) {
    handle.set_volume(volume);
}

#[tauri::command]
fn player_status(handle: tauri::State<PlayerHandle>) -> denki_core::Status {
    handle.status()
}

#[tauri::command]
#[cfg(not(target_os = "android"))]
fn queue_load(
    paths: Vec<String>,
    start_index: usize,
    autoplay: bool,
    handle: tauri::State<PlayerHandle>,
) -> Result<(), String> {
    let paths = paths.into_iter().map(PathBuf::from).collect();
    handle
        .load_queue(paths, start_index, autoplay)
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
#[tauri::command]
fn queue_load(
    paths: Vec<String>,
    start_index: usize,
    autoplay: bool,
    app: tauri::AppHandle,
    handle: tauri::State<PlayerHandle>,
) -> Result<(), String> {
    let mut options = FsOpenOptions::new();
    options.read(true);
    let sources = paths
        .into_iter()
        .map(|uri| {
            let file_path = uri
                .parse::<FsFilePath>()
                .expect("FilePath parsing is infallible");
            app.fs()
                .open(file_path, options.clone())
                .map(AudioSource::from_file)
                .map_err(|error| format!("Could not open selected audio: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    handle
        .load_source_queue(sources, start_index, autoplay)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn queue_next(handle: tauri::State<PlayerHandle>) -> Result<(), String> {
    handle.next().map_err(|e| e.to_string())
}

#[tauri::command]
fn queue_prev(handle: tauri::State<PlayerHandle>) -> Result<(), String> {
    handle.prev().map_err(|e| e.to_string())
}

fn state_file_path(_app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    let mut path = _app.path().app_config_dir().map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "android"))]
    let mut path = dirs::config_dir()
        .ok_or_else(|| "Could not locate Denki's configuration folder".to_string())?
        .join("denki");
    path.push("state.toml");
    Ok(path)
}

#[tauri::command]
fn state_load(app: tauri::AppHandle) -> denki_core::state::AppState {
    state_file_path(&app)
        .map(|path| denki_core::state::load_state(&path))
        .unwrap_or_default()
}

#[tauri::command]
fn state_save(
    app: tauri::AppHandle,
    last_path: Option<String>,
    #[allow(unused)] last_paths: Option<Vec<String>>,
    last_index: Option<usize>,
    last_position_secs: Option<f64>,
    volume: Option<f32>,
) {
    if let Ok(path) = state_file_path(&app) {
        denki_core::state::save_state(
            &path,
            &denki_core::state::AppState {
                last_path,
                last_paths: last_paths.unwrap_or_default(),
                last_index,
                last_position_secs,
                volume,
            },
        );
    }
}

#[cfg(not(target_os = "android"))]
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

#[cfg(target_os = "android")]
#[tauri::command]
fn mpris_set_metadata(
    _title: String,
    _artist: String,
    _duration_secs: Option<f64>,
) -> Result<(), String> {
    Err("System media controls are not available in this Android build yet".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let player_handle = PlayerHandle::spawn();

    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());

    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_fs::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(tauri_plugin_android_fs::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(
        tauri::plugin::Builder::<tauri::Wry>::new("ytdlp")
            .setup(|app, api| {
                let plugin = api.register_android_plugin("com.zaha.denki", "YtDlpPlugin")?;
                app.manage(AndroidYtDlp(plugin));
                Ok(())
            })
            .build(),
    );

    builder
        .manage(player_handle.clone())
        .setup(move |app| {
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(not(target_os = "android"))]
                {
                    let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                        width: 480.0,
                        height: 440.0,
                    }));
                    let _ = window.center();
                }
                let _ = window.show();
            }

            #[cfg(not(target_os = "android"))]
            {
                #[cfg(target_os = "windows")]
                let hwnd: Option<*mut std::ffi::c_void> = {
                    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                    let window = app.get_webview_window("main").expect("no main window");
                    let handle = window.window_handle().expect("failed to get window handle");
                    match handle.as_raw() {
                        RawWindowHandle::Win32(h) => {
                            Some(isize::from(h.hwnd) as *mut std::ffi::c_void)
                        }
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
            }
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
            android_pick_files,
            android_pick_folder,
            download_audio,
            state_load,
            state_save,
            mpris_set_metadata,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
