# Dictate 2 - Spec

## Overview
Command-line transcription app that runs in the background with a tray icon and a global hotkey to start/stop recording and transcribe audio via Whisper (Rust bindings).

## Requirements
### Core behavior
- CLI app only; no GUI windows besides tray menu.
- Starts from terminal and then runs in background while user switches apps.
- Global hotkey (Command+Space) toggles recording on/off.
- When recording starts, play a short beep; play the same beep when recording stops.
- On stop, transcription starts automatically; result is copied to clipboard.
- Tray icon reflects state:
  - Idle: blue.
  - Recording: red.
  - Transcribing: yellow with progress indicator (percent or busy indicator).
  - Model download/loading shows distinct status (text or indicator).
- Log key events: app start, recording start/stop, transcription start/stop, model download, errors.

### Storage and file output
- Default recordings folder: `.recordings` in repo root (git ignored).
- Each recording is saved as M4A with name based on ISO timestamp with timezone and milliseconds
  (e.g. `2025-01-01T23-59-59.123-0800.m4a`).
- Transcription is saved as Markdown with matching name
  (e.g. `2025-01-01T23-59-59.123-0800.md`) in same folder.
- Original audio file is kept.

### CLI and Justfile commands
- `just front` starts the app in foreground.
- `just run` starts the app (alias of front).
- `just transcribe <file>` transcribes a single audio file (MP3/M4A/WAV/etc) and outputs a `.md` file in the same folder as the input (same basename).
- `run` supports options:
  - `--model <name>` default: `turbo`.
  - `--recordings-dir <path>` default: `.recordings`.

### Microphone selection
- Tray menu shows current input device and allows switching to another available microphone.
- Selected microphone persists across app relaunches.

### Model handling
- Whisper model is downloaded if missing, stored locally for debug and reuse.
- Model download progress is reflected in tray status.

### Testing
- System interfaces are abstracted (traits) so they can be mocked.
- Prefer end-to-end tests with mocked system layers.
- Include at least 1-2 true end-to-end tests that perform a real short recording.
- Use a real Whisper model in tests (do not mock the transcriber).
- Do not mock the clock; create a new temp folder per test and discover output files there.

## Decisions
- Global shortcut: Command+Space.
- Target OS: macOS only.
- M4A encoding: prefer a native macOS encoder or a Rust crate over external ffmpeg CLI.

## Proposed architecture
### High-level components
- `app`: main entrypoint, CLI parsing, bootstraps services and event loop.
- `state`: state machine (Idle, Recording, Transcribing, Downloading, Error).
- `tray`: manages tray icon, menu, and state indicator updates.
- `hotkey`: registers global shortcut and dispatches toggle events.
- `audio`: microphone discovery + recording capture.
- `transcriber`: wraps whisper-rs; handles model download, load, and inference.
- `storage`: recordings/transcripts paths and file IO.
- `clipboard`: copies transcription result to clipboard.
- `config`: persisted settings (mic selection, model, recordings dir).
- `logging`: structured logging setup.

### Data flow
1. App starts, loads config, ensures model available.
2. Tray + hotkey registered; idle state.
3. Hotkey toggles recording:
   - start: beep, set red tray, record to temp/target file.
   - stop: beep, finalize file, set yellow tray, transcribe.
4. Transcription completes: save `.md`, copy to clipboard, set tray blue.

### Interfaces for testing
- `AudioInput`: list devices, select device, start/stop recording.
- `HotkeyProvider`: register/unregister global hotkey, emit events.
- `TrayUi`: set icon/state, update menu, show progress.
- `Transcriber`: load model, transcribe audio file or PCM buffer.
- `Clipboard`: write text.
- `FileStore`: file system access to allow temp dirs in tests.

## Proposed libraries (Rust crates)
- CLI: `clap` (subcommands: `run`, `transcribe`).
- Async/runtime: `tokio`.
- Tray: `tray-icon` + `tao` event loop.
- Global hotkey: `global-hotkey` (tauri).
- Audio capture: `cpal`.
- Audio decode/format handling: `symphonia` (MP3/M4A/WAV input for `transcribe`).
- M4A encoding: native macOS encoder (AVFoundation/CoreAudio) or `ffmpeg-next` if viable without external CLI.
- Whisper: `whisper-rs` (bindings to whisper.cpp).
- Clipboard: `arboard`.
- Config: `serde` + `toml` + `directories`.
- Logging: `tracing` + `tracing-subscriber`.
- Timestamping: `chrono`.
- Progress display: `indicatif` (for model download progress; feed into tray indicator).
- Testing: `tempfile`, `assert_cmd`, `mockall` (avoid mocking the transcriber and clock in E2E tests).
