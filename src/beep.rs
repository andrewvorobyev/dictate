use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::source::{SineWave, Zero};
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::time::Duration;

const WARMUP_MS: u64 = 120;
const BEEP_MS: u64 = 180;
const FREQ_HZ: f32 = 880.0;
const VOLUME: f32 = 0.2;
const CHANNELS: u16 = 1;
const SAMPLE_RATE: u32 = 48_000;

pub struct BeepPlayer {
    stream: OutputStream,
    handle: OutputStreamHandle,
    device_name: Option<String>,
}

impl BeepPlayer {
    pub fn new() -> Result<Self> {
        let (stream, handle) = OutputStream::try_default().context("default output device")?;
        Ok(Self {
            stream,
            handle,
            device_name: default_output_name(),
        })
    }

    pub fn play(&mut self) -> Result<()> {
        self.refresh_output_if_needed()?;

        let sink = Sink::try_new(&self.handle).context("create output sink")?;
        let silence = Zero::<f32>::new(CHANNELS, SAMPLE_RATE)
            .take_duration(Duration::from_millis(WARMUP_MS));
        let beep = SineWave::new(FREQ_HZ)
            .take_duration(Duration::from_millis(BEEP_MS))
            .amplify(VOLUME);
        sink.append(silence);
        sink.append(beep);
        sink.sleep_until_end();
        Ok(())
    }

    fn refresh_output_if_needed(&mut self) -> Result<()> {
        let current = default_output_name();
        if current != self.device_name {
            let (stream, handle) = OutputStream::try_default().context("default output device")?;
            self.stream = stream;
            self.handle = handle;
            self.device_name = current;
        }
        Ok(())
    }
}

fn default_output_name() -> Option<String> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    device.name().ok()
}
