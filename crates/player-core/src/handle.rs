use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::{Player, PlayerError};

enum Command {
    Load(PathBuf, Sender<Result<(), PlayerError>>),
    Play,
    Pause,
    Toggle(Sender<Result<Status, PlayerError>>),
    Seek(Duration, Sender<Result<(), PlayerError>>),
    SetVolume(f32),
    GetStatus(Sender<Status>),
    GetPlaybackStatus(Sender<PlaybackStatus>),
    LoadQueue(Vec<PathBuf>, usize, bool, Sender<Result<(), PlayerError>>),
    Next(Sender<Result<(), PlayerError>>),
    Prev(Sender<Result<(), PlayerError>>),
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Status {
    pub is_playing: bool,
    pub volume: f32,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub spectrum: Vec<f32>,
    pub queue_index: Option<usize>,
    pub queue_len: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackStatus {
    pub is_playing: bool,
    pub position_secs: f64,
    pub has_track: bool,
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<Command>,
}

impl PlayerHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<Command>();

        thread::spawn(move || {
            let mut player = match Player::new() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("failed to init audio player: {e}");
                    return;
                }
            };
            let mut queue: Vec<PathBuf> = Vec::new();
            let mut queue_index: usize = 0;

            loop {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(cmd) => handle_command(cmd, &mut player, &mut queue, &mut queue_index),
                    Err(RecvTimeoutError::Timeout) => {
                        if !queue.is_empty() && player.finished() && queue_index + 1 < queue.len() {
                            queue_index += 1;
                            if player.load(&queue[queue_index]).is_ok() {
                                player.play();
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Self { tx }
    }

    pub fn load(&self, path: PathBuf) -> Result<(), PlayerError> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Command::Load(path, tx)).ok();
        rx.recv().unwrap_or(Err(PlayerError::NoTrack))
    }

    pub fn play(&self) { self.tx.send(Command::Play).ok(); }
    pub fn pause(&self) { self.tx.send(Command::Pause).ok(); }

    pub fn toggle(&self) -> Result<Status, PlayerError> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Command::Toggle(tx))
            .map_err(|_| PlayerError::NoTrack)?;

        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(PlayerError::CommandTimeout),
            Err(RecvTimeoutError::Disconnected) => Err(PlayerError::NoTrack),
        }
    }

    pub fn seek(&self, pos: Duration) -> Result<(), PlayerError> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Command::Seek(pos, tx)).ok();
        rx.recv().unwrap_or(Err(PlayerError::NoTrack))
    }

    pub fn set_volume(&self, vol: f32) { self.tx.send(Command::SetVolume(vol)).ok(); }

    pub fn status(&self) -> Status {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Command::GetStatus(tx)).ok();
        rx.recv().unwrap_or(Status {
            is_playing: false, volume: 0.0, position_secs: 0.0,
            duration_secs: None, queue_index: None, queue_len: 0,
            spectrum: vec![0.0; 48],
        })
    }

    /// Returns only transport state, without calculating the spectrum.
    pub fn playback_status(&self) -> PlaybackStatus {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Command::GetPlaybackStatus(tx)).ok();
        rx.recv().unwrap_or(PlaybackStatus {
            is_playing: false,
            position_secs: 0.0,
            has_track: false,
        })
    }

    pub fn load_queue(&self, paths: Vec<PathBuf>, start_index: usize, autoplay: bool) -> Result<(), PlayerError> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Command::LoadQueue(paths, start_index, autoplay, tx)).ok();
        rx.recv().unwrap_or(Err(PlayerError::NoTrack))
    }

    pub fn next(&self) -> Result<(), PlayerError> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Command::Next(tx)).ok();
        rx.recv().unwrap_or(Err(PlayerError::NoTrack))
    }

    pub fn prev(&self) -> Result<(), PlayerError> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Command::Prev(tx)).ok();
        rx.recv().unwrap_or(Err(PlayerError::NoTrack))
    }
}

fn handle_command(cmd: Command, player: &mut Player, queue: &mut Vec<PathBuf>, queue_index: &mut usize) {
    match cmd {
        Command::Load(path, reply) => { let _ = reply.send(player.load(&path)); }
        Command::Play => player.play(),
        Command::Pause => player.pause(),
        Command::Toggle(reply) => {
            let result = player.toggle().map(|_| status(player, queue, *queue_index));
            let _ = reply.send(result);
        }
        Command::Seek(pos, reply) => { let _ = reply.send(player.seek(pos)); }
        Command::SetVolume(vol) => player.set_volume(vol),
        Command::GetStatus(reply) => { let _ = reply.send(status(player, queue, *queue_index)); }
        Command::GetPlaybackStatus(reply) => {
            let _ = reply.send(playback_status(player, queue));
        }
        Command::LoadQueue(paths, start, autoplay, reply) => {
            *queue = paths;
            *queue_index = start.min(queue.len().saturating_sub(1));
            let result = queue.get(*queue_index).map_or(Err(PlayerError::NoTrack), |p| player.load(p));
            if result.is_ok() && autoplay { player.play(); }
            let _ = reply.send(result);
        }
        Command::Next(reply) => {
            if *queue_index + 1 < queue.len() {
                *queue_index += 1;
                let result = player.load(&queue[*queue_index]);
                if result.is_ok() { player.play(); }
                let _ = reply.send(result);
            } else {
                let _ = reply.send(Err(PlayerError::NoTrack));
            }
        }
        Command::Prev(reply) => {
            if *queue_index > 0 {
                *queue_index -= 1;
                let result = player.load(&queue[*queue_index]);
                if result.is_ok() { player.play(); }
                let _ = reply.send(result);
            } else {
                let _ = reply.send(Err(PlayerError::NoTrack));
            }
        }
    }
}

fn status(player: &Player, queue: &[PathBuf], queue_index: usize) -> Status {
    Status {
        is_playing: player.is_playing(),
        volume: player.volume(),
        position_secs: player.position().as_secs_f64(),
        duration_secs: player.total_duration().map(|d| d.as_secs_f64()),
        spectrum: player.spectrum(),
        queue_index: if queue.is_empty() { None } else { Some(queue_index) },
        queue_len: queue.len(),
    }
}

fn playback_status(player: &Player, queue: &[PathBuf]) -> PlaybackStatus {
    PlaybackStatus {
        is_playing: player.is_playing(),
        position_secs: player.position().as_secs_f64(),
        has_track: !queue.is_empty(),
    }
}
