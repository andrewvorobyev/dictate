use anyhow::{anyhow, Result};
use dictate::audio::{encode_m4a, CpalRecorder};
use dictate::model;
use dictate::transcriber::WhisperTranscriber;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

#[test]
#[ignore = "requires microphone access, ffmpeg, and model download"]
fn e2e_record_and_transcribe() -> Result<()> {
    let dir = tempdir()?;
    let handle = CpalRecorder::start_recording(None)?;
    thread::sleep(Duration::from_secs(2));
    let recorded = handle.stop()?;

    let audio = dir.path().join("recording.m4a");
    encode_m4a(&recorded, &audio)?;

    let model_dir = dir.path().join("models");
    let model_path = model::ensure_model(&model_dir, "tiny")?;
    let transcriber = WhisperTranscriber::new(model_path)?;
    let text = transcriber.transcribe_file(&audio)?;
    fs::write(dir.path().join("recording.md"), &text)?;
    Ok(())
}

#[test]
fn e2e_transcribe_fixture_audio() -> Result<()> {
    let audio_path =
        Path::new(".recordings/2026-01-20T12-30-50.419-0800.m4a");
    if !audio_path.exists() {
        return Err(anyhow!(
            "missing test audio at {}",
            audio_path.display()
        ));
    }

    let model_dir = PathBuf::from(".models");
    let model_path = model::ensure_model(&model_dir, "small")?;
    let transcriber = WhisperTranscriber::new(model_path)?;
    let text = transcriber.transcribe_file_with_progress_and_prompt(
        audio_path,
        None::<fn(i32)>,
        None,
        Some("en"),
    )?;
    let normalized = text.to_lowercase();
    let has_digits = normalized.contains('1')
        && normalized.contains('2')
        && normalized.contains('3')
        && normalized.contains('4');
    let has_words = normalized.contains("one")
        && normalized.contains("two")
        && normalized.contains("three")
        && normalized.contains("four");
    if !has_digits && !has_words {
        return Err(anyhow!(
            "expected '1 2 3 4' (or words), got: {text:?}"
        ));
    }
    Ok(())
}
