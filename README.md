# Dictate 2

Dictate 2 is a macOS-only background transcription app. It sits in the menu bar,
listens for the global hotkey (Command+Space) to start/stop recording, transcribes
audio with Whisper, and copies the result to the clipboard. Recordings and
transcripts are stored under `.recordings`.

## Prerequisites (macOS)
- Rust toolchain (stable): https://rustup.rs
- Xcode Command Line Tools:
  - `xcode-select --install`
- CMake (required to build `whisper-rs`):
  - `brew install cmake`
- FFmpeg (used for M4A encoding via CLI):
  - `brew install ffmpeg`
- just (task runner):
  - `brew install just`

## Build
```bash
cargo build
```

## Run
```bash
just run
# or
cargo run -- run
```

## Permissions (macOS)
Because the app runs from your terminal, macOS will prompt for permissions tied to the
terminal app you launch it from.

- Microphone: required for recording audio.
- Accessibility (or Input Monitoring): required for the global hotkey.

## Transcribe a file
```bash
just transcribe /path/to/audio.m4a
# or
cargo run -- transcribe --input /path/to/audio.m4a
```

## Tests
```bash
cargo test
```

End-to-end tests that use a real microphone and model download are marked ignored:
```bash
cargo test -- --ignored
```
