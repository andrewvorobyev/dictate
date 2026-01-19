use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat};
use crossbeam_channel::{bounded, Sender};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug)]
pub struct RecordedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct RecordingHandle {
    stop_tx: Sender<()>,
    join: thread::JoinHandle<Result<RecordedAudio>>,
}

impl RecordingHandle {
    pub fn stop(self) -> Result<RecordedAudio> {
        let _ = self.stop_tx.send(());
        self.join.join().unwrap_or_else(|_| Err(anyhow::anyhow!("recording thread panicked")))
    }
}

pub struct CpalRecorder;

impl CpalRecorder {
    pub fn list_devices() -> Result<Vec<AudioDevice>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();
        for device in host.input_devices()? {
            let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
            devices.push(AudioDevice {
                id: name.clone(),
                name,
            });
        }
        Ok(devices)
    }

    pub fn start_recording(selected_device: Option<&str>) -> Result<RecordingHandle> {
        let host = cpal::default_host();
        let device = if let Some(name) = selected_device {
            host.input_devices()?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                .context("selected microphone not found")?
        } else {
            host.default_input_device()
                .context("no default input device")?
        };

        let config = device
            .default_input_config()
            .context("default input config")?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels();
        let sample_format = config.sample_format();

        let (stop_tx, stop_rx) = bounded(1);
        let join = thread::spawn(move || {
            let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
            let samples_cb = Arc::clone(&samples);
            let err_fn = |err| tracing::error!(error = %err, "audio stream error");

            let stream_config = config.into();
            let stream = match sample_format {
                SampleFormat::F32 => device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| write_input_data(data, &samples_cb),
                    err_fn,
                    None,
                )?,
                SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| write_input_data(data, &samples_cb),
                    err_fn,
                    None,
                )?,
                SampleFormat::U16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| write_input_data(data, &samples_cb),
                    err_fn,
                    None,
                )?,
                _ => {
                    return Err(anyhow::anyhow!(
                        "unsupported sample format: {sample_format:?}"
                    ))
                }
            };

            stream.play()?;
            let _ = stop_rx.recv();
            drop(stream);

            let data = std::mem::take(&mut *samples.lock().unwrap());
            Ok(RecordedAudio {
                samples: data,
                sample_rate,
                channels,
            })
        });

        Ok(RecordingHandle { stop_tx, join })
    }
}

fn write_input_data<T>(input: &[T], samples: &Arc<Mutex<Vec<f32>>>)
where
    T: Sample,
    f32: FromSample<T>,
{
    if let Ok(mut buffer) = samples.lock() {
        for &sample in input {
            buffer.push(sample.to_sample::<f32>());
        }
    }
}

pub fn encode_m4a(recorded: &RecordedAudio, output: &Path) -> Result<()> {
    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "f32le",
            "-ar",
            &recorded.sample_rate.to_string(),
            "-ac",
            &recorded.channels.to_string(),
            "-i",
            "pipe:0",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            output
                .to_str()
                .context("output path not valid utf-8")?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn ffmpeg")?;

    let mut stdin = child.stdin.take().context("open ffmpeg stdin")?;
    let mut chunk = Vec::with_capacity(4096);
    for sample in &recorded.samples {
        chunk.extend_from_slice(&sample.to_le_bytes());
        if chunk.len() >= 4096 {
            stdin.write_all(&chunk)?;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        stdin.write_all(&chunk)?;
    }
    drop(stdin);

    let status = child.wait().context("wait for ffmpeg")?;
    if !status.success() {
        return Err(anyhow::anyhow!("ffmpeg failed with status {status}"));
    }
    Ok(())
}
