use anyhow::Result;
use dictate::audio::{encode_m4a, CpalRecorder};
use dictate::model;
use dictate::transcriber::WhisperTranscriber;
use std::fs;
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
