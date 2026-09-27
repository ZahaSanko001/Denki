# Denki

Denki is a compact desktop music player built with Tauri, Rust, and a small
vanilla JavaScript frontend.

## Features

- Open audio files or scan a folder recursively.
- Play, pause, seek, change volume, and move through the queue.
- View a stereo spectrum visualizer inspired by CAVA.
- Restore the last folder, track, playback position, and volume on launch.
- Expose playback controls and track metadata through platform media controls
  where supported.

The library currently scans MP3, FLAC, WAV, OGG, M4A, and AAC files.

## Build and run

Install Rust stable, the Tauri CLI, and the native prerequisites for your
platform. Tauri's platform-specific setup instructions are at
[v2.tauri.app/start/prerequisites](https://v2.tauri.app/start/prerequisites/).
This project serves its frontend directly from `src/`; a JavaScript package
manager is not required.

```sh
cargo install tauri-cli --version '^2'
cargo tauri dev
```

Create a release bundle with:

```sh
cargo tauri build
```

Build and verify the release separately on each operating system you intend to
support. Bundling and signing requirements differ by platform.

## Controls

- **Space:** play or pause
- **Left / Right arrows:** previous or next track

## Project layout

- `src/` — HTML, CSS, and JavaScript user interface
- `crates/player-core/` — audio playback and spectrum analysis
- `crates/library-core/` — recursive audio-file scanning and tag reading
- `src-tauri/` — Tauri commands, window configuration, persistence, and media
  controls

## License

Denki is licensed under the GNU General Public License, version 3 only. See
[COPYING](COPYING) for the complete license text.
