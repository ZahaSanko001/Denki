use player_core::PlayerHandle;
use std::path::PathBuf;
use std::time::Duration;
use tauri::Manager;
mod state;
mod mpris;

#[tauri::command]
fn library_scan_paths(paths: Vec<String>) -> Vec<library_core::Track> {
    let paths: Vec<std::path::PathBuf> = paths.into_iter().map(std::path::PathBuf::from).collect();
    library_core::scan_paths(&paths)
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
            state_load,
            state_save,
            mpris_set_metadata,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
