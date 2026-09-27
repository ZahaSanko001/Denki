use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use player_core::PlayerHandle;

pub type SharedControls = Arc<Mutex<MediaControls>>;

pub fn init(handle: PlayerHandle, hwnd: Option<*mut std::ffi::c_void>) -> Result<SharedControls, String> {
    let config = PlatformConfig {
        dbus_name: "denki",
        display_name: "Denki",
        hwnd,
    };

    let controls = MediaControls::new(config)
        .map_err(|error| format!("could not create media controls: {error:?}"))?;
    let shared = Arc::new(Mutex::new(controls));
    let stopped_by_command = Arc::new(AtomicBool::new(false));

    // Wire incoming MPRIS/media-key events back into the player.
    {
        let event_handle = handle.clone();
        let shared_for_volume = shared.clone();
        let shared_for_stop = shared.clone();
        let stopped_for_events = stopped_by_command.clone();
        shared
            .lock()
            .map_err(|error| format!("could not lock media controls: {error}"))?
            .attach(move |event: MediaControlEvent| match event {
                MediaControlEvent::Play => {
                    stopped_for_events.store(false, Ordering::Relaxed);
                    event_handle.play();
                }
                MediaControlEvent::Pause => event_handle.pause(),
                MediaControlEvent::Toggle => {
                    if event_handle.playback_status().is_playing {
                        event_handle.pause();
                    } else {
                        stopped_for_events.store(false, Ordering::Relaxed);
                        event_handle.play();
                    }
                }
                MediaControlEvent::Next => {
                    let _ = event_handle.next();
                }
                MediaControlEvent::Previous => {
                    let _ = event_handle.prev();
                }
                MediaControlEvent::Stop => {
                    event_handle.pause();
                    let _ = event_handle.seek(Duration::ZERO);
                    stopped_for_events.store(true, Ordering::Relaxed);
                    if let Ok(mut controls) = shared_for_stop.lock() {
                        let _ = controls.set_playback(MediaPlayback::Stopped);
                    }
                }
                MediaControlEvent::Seek(direction) => {
                    seek_relative(&event_handle, direction, Duration::from_secs(5));
                }
                MediaControlEvent::SeekBy(direction, delta) => {
                    seek_relative(&event_handle, direction, delta);
                }
                MediaControlEvent::SetPosition(pos) => {
                    let _ = event_handle.seek(pos.0);
                }
                MediaControlEvent::SetVolume(vol) => {
                    event_handle.set_volume(vol as f32);
                    // Souvlaki requires this acknowledgement call on the MPRIS backend.
                    if let Ok(mut controls) = shared_for_volume.lock() {
                        let _ = controls.set_volume(vol);
                    }
                }
                // Raise / Quit / OpenUri: not wired up yet.
                _ => {}
            })
            .map_err(|error| format!("could not attach media control events: {error:?}"))?;
    }

    // Background sync: pushes real play/pause + position state to MPRIS.
    // Runs on its own thread since this is a separate protocol (D-Bus)
    // from the Tauri IPC layer entirely. Metadata (title/artist) is
    // pushed separately, from the frontend — see `mpris_set_metadata`.
    {
        let shared = shared.clone();
        let stopped_by_command = stopped_by_command.clone();
        thread::spawn(move || {
            let mut last_playing: Option<bool> = None;
            let mut last_stopped: Option<bool> = None;
            let mut last_position_secs: Option<f64> = None;
            loop {
                thread::sleep(Duration::from_millis(1000));
                let status = handle.playback_status();
                if status.is_playing {
                    stopped_by_command.store(false, Ordering::Relaxed);
                }
                let stopped = !status.has_track || stopped_by_command.load(Ordering::Relaxed);
                let position_changed = last_position_secs
                    .map(|last| status.position_secs != last)
                    .unwrap_or(true);
                if last_playing != Some(status.is_playing)
                    || last_stopped != Some(stopped)
                    || position_changed
                {
                    last_playing = Some(status.is_playing);
                    last_stopped = Some(stopped);
                    last_position_secs = Some(status.position_secs);
                    if let Ok(mut controls) = shared.lock() {
                        let progress =
                            Some(MediaPosition(Duration::from_secs_f64(status.position_secs)));
                        let playback = if stopped {
                            MediaPlayback::Stopped
                        } else if status.is_playing {
                            MediaPlayback::Playing { progress }
                        } else {
                            MediaPlayback::Paused { progress }
                        };
                        let _ = controls.set_playback(playback);
                    }
                }
            }
        });
    }

    Ok(shared)
}

fn seek_relative(handle: &PlayerHandle, direction: SeekDirection, delta: Duration) {
    let current = Duration::from_secs_f64(handle.playback_status().position_secs);
    let target = match direction {
        SeekDirection::Forward => current + delta,
        SeekDirection::Backward => current.saturating_sub(delta),
    };
    let _ = handle.seek(target);
}
