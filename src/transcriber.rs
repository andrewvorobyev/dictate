use anyhow::{Context, Result};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::fs::File;
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub struct WhisperTranscriber {
    model_path: PathBuf,
}

impl WhisperTranscriber {
    pub fn new(model_path: PathBuf) -> Result<Self> {
        Ok(Self { model_path })
    }

    pub fn transcribe_file(&self, path: &Path) -> Result<String> {
        let (samples, sample_rate) = decode_to_mono_f32(path)?;
        let samples_16k = resample_to_16k(samples, sample_rate)?;
        self.transcribe_samples(&samples_16k)
    }

    fn transcribe_samples(&self, samples: &[f32]) -> Result<String> {
        let model_path = self
            .model_path
            .to_str()
            .context("model path not valid utf-8")?;
        let ctx = whisper_rs::WhisperContext::new(model_path)
            .with_context(|| format!("load whisper model {model_path}"))?;
        let mut state = ctx
            .create_state()
            .context("create whisper state")?;
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(4);
        state
            .full(params, samples)
            .context("whisper inference")?;

        let num_segments = state.full_n_segments().context("segment count")?;
        let mut out = String::new();
        for i in 0..num_segments {
            let segment = state
                .full_get_segment_text(i)
                .context("segment text")?;
            out.push_str(segment);
        }
        Ok(out.trim().to_string())
    }
}

fn decode_to_mono_f32(path: &Path) -> Result<(Vec<f32>, u32)> {
    let file = File::open(path).with_context(|| format!("open audio {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format.default_track().context("no default audio track")?;
    let sample_rate = track
        .codec_params
        .sample_rate
        .context("missing sample rate")?;

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut mono = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(err) => return Err(err.into()),
        };

        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        let mut sample_buf = SampleBuffer::<f32>::new(decoded.frames(), spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();
        if channels == 1 {
            mono.extend_from_slice(samples);
        } else {
            for frame in 0..sample_buf.frames() {
                let mut sum = 0.0;
                for ch in 0..channels {
                    sum += samples[frame * channels + ch];
                }
                mono.push(sum / channels as f32);
            }
        }
    }

    Ok((mono, sample_rate))
}

fn resample_to_16k(input: Vec<f32>, sample_rate: u32) -> Result<Vec<f32>> {
    if sample_rate == 16_000 {
        return Ok(input);
    }
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler =
        SincFixedIn::<f32>::new(16_000.0 / sample_rate as f64, 1.0, params, input.len(), 1)?;
    let out = resampler.process(&[input], None)?;
    Ok(out.into_iter().next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    #[ignore = "requires model download and audio input"]
    fn e2e_transcribe_silence() -> Result<()> {
        let dir = tempdir()?;
        let wav = dir.path().join("silence.wav");
        write_silence_wav(&wav)?;
        let model_dir = dir.path().join("models");
        let model_path = crate::model::ensure_model(&model_dir, "tiny")?;
        let transcriber = WhisperTranscriber::new(model_path)?;
        let _ = transcriber.transcribe_file(&wav)?;
        Ok(())
    }

    fn write_silence_wav(path: &Path) -> Result<()> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec)?;
        for _ in 0..(16_000 / 2) {
            writer.write_sample(0i16)?;
        }
        writer.finalize()?;
        Ok(())
    }
}
