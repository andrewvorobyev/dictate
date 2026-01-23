use anyhow::{anyhow, Result};
use dictate::audio::{encode_m4a, CpalRecorder};
use dictate::model;
use dictate::transcriber::WhisperTranscriber;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

#[test]
#[ignore = "requires microphone access, ffmpeg, and model download"]
fn e2e_record_and_transcribe() -> Result<()> {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipping e2e_record_and_transcribe: ffmpeg not available");
        return Ok(());
    }
    let dir = tempdir()?;
    let handle = match CpalRecorder::start_recording(None) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("skipping e2e_record_and_transcribe: {err}");
            return Ok(());
        }
    };
    thread::sleep(Duration::from_secs(2));
    let recorded = handle.stop()?;

    let audio = dir.path().join("recording.m4a");
    encode_m4a(&recorded, &audio)?;

    let model_dir = PathBuf::from(".models");
    let preferred = ["tiny", "base", "small", "turbo", "medium", "large"];
    let model_name = preferred
        .iter()
        .copied()
        .find(|name| {
            model::model_info(name)
                .map(|info| model_dir.join(info.filename).exists())
                .unwrap_or(false)
        })
        .unwrap_or("tiny");
    let model_path = model::ensure_model(&model_dir, model_name)?;
    let transcriber = WhisperTranscriber::new(model_path)?;
    let text = transcriber.transcribe_file(&audio)?;
    fs::write(dir.path().join("recording.md"), &text)?;
    Ok(())
}

#[test]
fn e2e_transcribe_fixture_audio() -> Result<()> {
    let audio_path = Path::new("tests/fixtures/1234.m4a");
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
