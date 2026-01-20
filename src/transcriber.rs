use anyhow::{Context, Result};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::ffi::CStr;
use std::fs::File;
use std::os::raw::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::{env, fs};
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
        init_whisper_runtime();
        Ok(Self { model_path })
    }

    pub fn transcribe_file(&self, path: &Path) -> Result<String> {
        self.transcribe_file_with_progress_and_prompt(path, None::<fn(i32)>, None)
    }

    pub fn transcribe_file_with_prompt(
        &self,
        path: &Path,
        prompt: Option<&str>,
    ) -> Result<String> {
        self.transcribe_file_with_progress_and_prompt(path, None::<fn(i32)>, prompt)
    }

    pub fn transcribe_file_with_progress<F>(
        &self,
        path: &Path,
        progress: Option<F>,
    ) -> Result<String>
    where
        F: FnMut(i32) + 'static,
    {
        self.transcribe_file_with_progress_and_prompt(path, progress, None)
    }

    pub fn transcribe_file_with_progress_and_prompt<F>(
        &self,
        path: &Path,
        progress: Option<F>,
        prompt: Option<&str>,
    ) -> Result<String>
    where
        F: FnMut(i32) + 'static,
    {
        let (samples, sample_rate) = decode_to_mono_f32(path)?;
        let samples_16k = resample_to_16k(samples, sample_rate)?;
        self.transcribe_samples_with_progress(&samples_16k, progress, prompt)
    }

    fn transcribe_samples_with_progress<F>(
        &self,
        samples: &[f32],
        progress: Option<F>,
        prompt: Option<&str>,
    ) -> Result<String>
    where
        F: FnMut(i32) + 'static,
    {
        let _silence = StderrSilencer::new();
        let model_path = self
            .model_path
            .to_str()
            .context("model path not valid utf-8")?;
        let mut ctx_params = whisper_rs::WhisperContextParameters::default();
        ctx_params.use_gpu(true);
        let ctx = whisper_rs::WhisperContext::new_with_params(model_path, ctx_params)
            .with_context(|| format!("load whisper model {model_path}"))?;
        unsafe {
            set_metal_log_callback();
        }
        let mut state = ctx
            .create_state()
            .context("create whisper state")?;
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);
        params.set_n_threads(threads);
        params.set_suppress_blank(true);
        params.set_suppress_non_speech_tokens(true);
        if let Some(prompt) = prompt {
            let prompt = prompt.trim();
            if !prompt.is_empty() {
                params.set_initial_prompt(prompt);
            }
        }
        params.set_progress_callback_safe::<Option<F>, F>(progress);
        state
            .full(params, samples)
            .context("whisper inference")?;

        let num_segments = state.full_n_segments().context("segment count")?;
        let mut out = String::new();
        for i in 0..num_segments {
            let segment = state
                .full_get_segment_text(i)
                .context("segment text")?;
            out.push_str(&segment);
        }
        Ok(out.trim().to_string())
    }
}

static WHISPER_RUNTIME_INIT: Once = Once::new();

fn init_whisper_runtime() {
    WHISPER_RUNTIME_INIT.call_once(|| {
        ensure_metal_resources();
        unsafe {
            whisper_rs::set_log_callback(Some(whisper_log_filtered), std::ptr::null_mut());
            set_metal_log_callback();
        }
    });
}

unsafe extern "C" fn whisper_log_filtered(
    _level: whisper_rs::whisper_rs_sys::ggml_log_level,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    let line = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let msg = line.trim();
    if msg.is_empty() || is_noisy_metal_log(msg) {
        return;
    }
    match _level {
        whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_ERROR => {
            tracing::error!(message = msg, "whisper");
        }
        whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_WARN => {
            tracing::warn!(message = msg, "whisper");
        }
        _ => {}
    }
}

unsafe extern "C" fn whisper_log_silent(
    _level: whisper_rs::whisper_rs_sys::ggml_log_level,
    _text: *const c_char,
    _user_data: *mut c_void,
) {
}

struct StderrSilencer {
    #[cfg(unix)]
    original_fd: i32,
    #[cfg(unix)]
    null_fd: i32,
}

impl StderrSilencer {
    fn new() -> Option<Self> {
        #[cfg(unix)]
        unsafe {
            let original_fd = libc::dup(libc::STDERR_FILENO);
            if original_fd < 0 {
                return None;
            }
            let null_fd = libc::open(b"/dev/null\0".as_ptr().cast(), libc::O_WRONLY);
            if null_fd < 0 {
                libc::close(original_fd);
                return None;
            }
            if libc::dup2(null_fd, libc::STDERR_FILENO) < 0 {
                libc::close(original_fd);
                libc::close(null_fd);
                return None;
            }
            Some(Self { original_fd, null_fd })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

#[cfg(unix)]
impl Drop for StderrSilencer {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::dup2(self.original_fd, libc::STDERR_FILENO);
            let _ = libc::close(self.original_fd);
            let _ = libc::close(self.null_fd);
        }
    }
}

fn is_noisy_metal_log(msg: &str) -> bool {
    msg.starts_with("ggml_metal_")
        || msg.starts_with("ggml_backend_metal_")
        || msg.contains("GGML_METAL_PATH_RESOURCES")
        || msg.contains("ggml-metal.metal")
        || msg.contains("Metal backend")
        || msg.contains("Metal GPU")
        || msg.contains("ggml_backend_metal")
}

#[cfg(target_os = "macos")]
unsafe fn set_metal_log_callback() {
    unsafe {
        ggml_metal_log_set_callback(Some(whisper_log_silent), std::ptr::null_mut());
    }
}

#[cfg(not(target_os = "macos"))]
unsafe fn set_metal_log_callback() {}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn ggml_metal_log_set_callback(
        callback: Option<
            unsafe extern "C" fn(
                level: whisper_rs::whisper_rs_sys::ggml_log_level,
                text: *const c_char,
                user_data: *mut c_void,
            ),
        >,
        user_data: *mut c_void,
    );
}

fn ensure_metal_resources() {
    if env::var("GGML_METAL_PATH_RESOURCES").is_ok() {
        return;
    }

    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let models_dir = cwd.join(".models");
    if models_dir.join("ggml-metal.metal").exists() {
        unsafe {
            env::set_var("GGML_METAL_PATH_RESOURCES", &models_dir);
        }
        tracing::debug!(
            path = %models_dir.display(),
            "using metal resources from .models"
        );
        return;
    }

    let candidates = [
        cwd.join("target/debug/build"),
        cwd.join("target/release/build"),
    ];
    for base in candidates {
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let path = entry
                    .path()
                    .join("out/whisper.cpp/ggml-metal.metal");
                if path.exists() {
                    if let Some(dir) = path.parent() {
                        unsafe {
                            env::set_var("GGML_METAL_PATH_RESOURCES", dir);
                        }
                        tracing::debug!(
                            path = %dir.display(),
                            "using metal resources from build output"
                        );
                        return;
                    }
                }
            }
        }
    }

    tracing::warn!("metal resources not found; set GGML_METAL_PATH_RESOURCES");
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
        let duration = decoded.frames() as u64;
        let mut sample_buf = SampleBuffer::<f32>::new(duration, spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();
        if channels == 1 {
            mono.extend_from_slice(samples);
        } else {
            let frames = samples.len() / channels;
            for frame in 0..frames {
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
