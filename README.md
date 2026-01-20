# Dictate

Dictate is a macOS-only background transcription app. It sits in the menu bar,
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
```

## Models
The default model is `small`. Choose a different one with `--model`:
```bash
just run --model tiny
```

List all available models (with size, description, and language support):
```bash
just models
```

## Automatic transcription
Configure directory watching in `~/.config/dictate.yaml`:
```yaml
model: small
vocabulary:
  - Dictate
  - Whisper
auto_transcribe:
  processed_dir: /Users/you/Dictate/processed
  watches:
    - input_dir: /Users/you/Dictate/inbox
      output_dir: /Users/you/Dictate/transcripts
```

When `dictate` is running, it watches each `input_dir` for `.m4a` files, writes the
transcript to the corresponding `output_dir` using the same filename with `.md`,
and moves the source file into `processed_dir`.

Vocabulary entries are passed to the model as an initial prompt for both hotkey
recordings and auto-transcribed files.

## Permissions (macOS)
Because the app runs from your terminal, macOS will prompt for permissions tied to the
terminal app you launch it from.

- Microphone: required for recording audio.
- Accessibility (or Input Monitoring): required for the global hotkey.

## Transcribe a file
```bash
just transcribe /path/to/audio.m4a
```
Force a specific language (omit for auto-detect):
```bash
just transcribe /path/to/audio.m4a --language ru
```

## Tests
```bash
cargo test
```

End-to-end tests that use a real microphone and model download are marked ignored:
```bash
cargo test -- --ignored
```
