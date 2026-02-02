use anyhow::{Context, Result};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::ffi::CStr;
use std::fs::File;
use std::os::raw::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::cmp::Ordering;
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
        self.transcribe_file_with_progress_and_prompt(path, None::<fn(i32)>, None, None)
    }

    pub fn transcribe_file_with_prompt(
        &self,
        path: &Path,
        prompt: Option<&str>,
    ) -> Result<String> {
        self.transcribe_file_with_progress_and_prompt(path, None::<fn(i32)>, prompt, None)
    }

    pub fn transcribe_file_with_progress<F>(
        &self,
        path: &Path,
        progress: Option<F>,
    ) -> Result<String>
    where
        // Progress callbacks can be invoked from non-main threads; keep them Send to avoid UB.
        F: FnMut(i32) + Send + 'static,
    {
        self.transcribe_file_with_progress_and_prompt(path, progress, None, None)
    }

    pub fn transcribe_file_with_progress_and_prompt<F>(
        &self,
        path: &Path,
        progress: Option<F>,
        prompt: Option<&str>,
        language: Option<&str>,
    ) -> Result<String>
    where
        // Progress callbacks can be invoked from non-main threads; keep them Send to avoid UB.
        F: FnMut(i32) + Send + 'static,
    {
        tracing::debug!(path = %path.display(), "decoding audio");
        let (samples, sample_rate) = decode_to_mono_f32(path)?;
        let raw_duration = if sample_rate == 0 {
            0.0
        } else {
            samples.len() as f32 / sample_rate as f32
        };
        tracing::debug!(
            sample_rate,
            samples = samples.len(),
            duration_sec = raw_duration,
            "decoded audio"
        );
        let samples_16k = resample_to_16k(samples, sample_rate)?;
        let duration_16k = samples_16k.len() as f32 / 16_000.0;
        tracing::debug!(
            samples = samples_16k.len(),
            duration_sec = duration_16k,
            "resampled audio"
        );
        let mut samples_16k = samples_16k;
        if let Some(vad) = prefilter_speech(&mut samples_16k, 16_000) {
            tracing::debug!(
                removed_samples = vad.removed_samples,
                kept_samples = vad.kept_samples,
                removed_sec = vad.removed_samples as f32 / 16_000.0,
                segments = vad.segments,
                threshold = vad.threshold,
                noise_floor = vad.noise_floor,
                keep_silence_ms = vad.keep_silence_ms,
                pad_ms = vad.pad_ms,
                "prefiltered non-speech"
            );
        }
        if let Some(trim) = trim_silence(&mut samples_16k, 16_000) {
            tracing::debug!(
                trimmed_samples = trim.trimmed_samples,
                trimmed_leading_samples = trim.trimmed_leading_samples,
                trimmed_trailing_samples = trim.trimmed_trailing_samples,
                trimmed_sec = trim.trimmed_samples as f32 / 16_000.0,
                threshold = trim.threshold,
                noise_floor = trim.noise_floor,
                leading_frames = trim.leading_frames,
                trailing_frames = trim.trailing_frames,
                "trimmed leading/trailing silence"
            );
        }
        if samples_16k.is_empty() {
            tracing::debug!("audio is silent after trimming; skipping inference");
            return Ok(String::new());
        }
        self.transcribe_samples_with_progress(&samples_16k, progress, prompt, language)
    }

    fn transcribe_samples_with_progress<F>(
        &self,
        samples: &[f32],
        progress: Option<F>,
        prompt: Option<&str>,
        language: Option<&str>,
    ) -> Result<String>
    where
        // Progress callbacks can be invoked from non-main threads; keep them Send to avoid UB.
        F: FnMut(i32) + Send + 'static,
    {
        let _silence = StderrSilencer::new();
        let model_path = self
            .model_path
            .to_str()
            .context("model path not valid utf-8")?;
        let threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);
        let prompt = prompt.and_then(|prompt| {
            let prompt = prompt.trim();
            if prompt.is_empty() {
                None
            } else {
                Some(prompt)
            }
        });
        let mut max_abs = 0.0f32;
        let mut sum_abs = 0.0f32;
        for &sample in samples {
            let abs = sample.abs();
            if abs > max_abs {
                max_abs = abs;
            }
            sum_abs += abs;
        }
        let avg_abs = if samples.is_empty() {
            0.0
        } else {
            sum_abs / samples.len() as f32
        };
        let language = language.and_then(|lang| {
            let lang = lang.trim();
            if lang.is_empty() {
                None
            } else {
                Some(lang)
            }
        });
        let mut detect_language = false;
        let mut language_for_params = None;
        let mut language_label = "default-en";
        if let Some(language) = language {
            if language.eq_ignore_ascii_case("auto") {
                detect_language = true;
                language_label = "auto";
            } else {
                language_for_params = Some(language);
                language_label = language;
            }
        }
        let prompt_len = prompt.map(|p| p.len()).unwrap_or(0);
        let duration_sec = samples.len() as f32 / 16_000.0;
        let run_inference = |use_gpu: bool, progress: Option<F>| -> Result<(String, i32)> {
            let mut ctx_params = whisper_rs::WhisperContextParameters::default();
            ctx_params.use_gpu(use_gpu);
            let ctx = whisper_rs::WhisperContext::new_with_params(model_path, ctx_params)
                .with_context(|| format!("load whisper model {model_path}"))?;
            unsafe {
                set_metal_log_callback();
            }
            let mut state = ctx
                .create_state()
                .context("create whisper state")?;
            let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::BeamSearch {
                beam_size: 5,
                patience: 1.0,
            });
            params.set_n_threads(threads);
            params.set_suppress_blank(true);
            params.set_suppress_non_speech_tokens(true);
            params.set_temperature(0.0);
            params.set_temperature_inc(0.2);
            params.set_logprob_thold(-1.0);
            params.set_entropy_thold(2.4);
            params.set_no_speech_thold(0.6);
            if let Some(prompt) = prompt {
                params.set_initial_prompt(prompt);
            }
            tracing::debug!(
                model = %model_path,
                threads,
                prompt_len,
                language = language_label,
                detect_language,
                duration_sec,
                max_abs,
                avg_abs,
                use_gpu,
                "starting whisper inference"
            );
            params.set_progress_callback_safe::<Option<F>, F>(progress);
            if detect_language {
                params.set_language(None);
                params.set_detect_language(true);
            } else if let Some(language) = language_for_params {
                params.set_language(Some(language));
                params.set_detect_language(false);
            }
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
            Ok((out.trim().to_string(), num_segments))
        };

        let mut used_gpu = true;
        let mut progress = progress;
        let (mut text, mut num_segments) = match run_inference(true, progress.take()) {
            Ok(result) => result,
            Err(err) => {
                tracing::debug!(error = %err, "whisper inference failed with gpu; retrying on cpu");
                used_gpu = false;
                run_inference(false, None)?
            }
        };

        if num_segments == 0 && used_gpu {
            tracing::debug!(
                duration_sec,
                max_abs,
                avg_abs,
                "whisper returned no segments with gpu; retrying on cpu"
            );
            let (cpu_text, cpu_segments) = run_inference(false, None)?;
            text = cpu_text;
            num_segments = cpu_segments;
            used_gpu = false;
        }

        if num_segments == 0 {
            tracing::debug!(
                duration_sec,
                max_abs,
                avg_abs,
                use_gpu = used_gpu,
                "whisper returned no segments"
            );
        } else {
            tracing::debug!(num_segments, use_gpu = used_gpu, "whisper returned segments");
        }
        Ok(text)
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

    tracing::error!("metal resources not found; set GGML_METAL_PATH_RESOURCES");
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

struct VadResult {
    removed_samples: usize,
    kept_samples: usize,
    segments: usize,
    threshold: f32,
    noise_floor: f32,
    keep_silence_ms: usize,
    pad_ms: usize,
}

fn speech_median(energies: &[f32], threshold: f32) -> Option<f32> {
    let mut speech: Vec<f32> = energies.iter().copied().filter(|&e| e >= threshold).collect();
    if speech.len() < 3 {
        return None;
    }
    speech.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    Some(speech[speech.len() / 2])
}

fn find_dynamic_tail_start(
    energies: &[f32],
    frame_ms: usize,
    speech_ref: f32,
) -> Option<usize> {
    let num_frames = energies.len();
    if num_frames == 0 || speech_ref <= 0.0 {
        return None;
    }
    let window_ms = 200usize;
    let min_tail_ms = 300usize;
    let drop_ratio = 0.25f32;
    let window_frames = ((window_ms + frame_ms - 1) / frame_ms)
        .max(3)
        .min(num_frames);
    let min_tail_frames = (min_tail_ms + frame_ms - 1) / frame_ms;
    if num_frames < window_frames + min_tail_frames {
        return None;
    }
    let mut prefix = Vec::with_capacity(num_frames + 1);
    prefix.push(0.0);
    for &e in energies {
        let last = *prefix.last().unwrap();
        prefix.push(last + e);
    }
    let threshold = speech_ref * drop_ratio;
    let mut tail_start: Option<usize> = None;
    for i in (0..=num_frames - window_frames).rev() {
        let sum = prefix[i + window_frames] - prefix[i];
        let avg = sum / window_frames as f32;
        if avg >= threshold {
            tail_start = Some(i + window_frames);
            break;
        }
    }
    let Some(tail_start) = tail_start else {
        return None;
    };
    let tail_frames = num_frames.saturating_sub(tail_start);
    if tail_frames < min_tail_frames {
        return None;
    }
    Some(tail_start)
}

fn prefilter_speech(samples: &mut Vec<f32>, sample_rate: u32) -> Option<VadResult> {
    if samples.is_empty() || sample_rate == 0 {
        return None;
    }
    let original_len = samples.len();
    let frame_ms = 20usize;
    let frame_len = (sample_rate as usize * frame_ms) / 1000;
    if frame_len == 0 || original_len < frame_len * 2 {
        return None;
    }

    let num_frames = (samples.len() + frame_len - 1) / frame_len;
    if num_frames == 0 {
        return None;
    }

    let mut energies = Vec::with_capacity(num_frames);
    for i in 0..num_frames {
        let start = i * frame_len;
        let end = std::cmp::min(start + frame_len, samples.len());
        if start >= end {
            energies.push(0.0);
            continue;
        }
        let mut sum = 0.0f32;
        for &sample in &samples[start..end] {
            sum += sample.abs();
        }
        energies.push(sum / (end - start) as f32);
    }

    let mut sorted = energies.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let noise_idx = ((sorted.len() as f32 - 1.0) * 0.1).round() as usize;
    let noise_floor = sorted[noise_idx.min(sorted.len() - 1)];
    let threshold = (noise_floor * 2.5).max(0.002);
    let speech_ref = speech_median(&energies, threshold);
    let dynamic_tail_start =
        speech_ref.and_then(|speech_ref| find_dynamic_tail_start(&energies, frame_ms, speech_ref));
    let tail_window_ms = 800usize;
    let tail_frames = (tail_window_ms + frame_ms - 1) / frame_ms;
    let tail_start = num_frames.saturating_sub(tail_frames);
    let tail_threshold = (noise_floor * 2.0).max(0.0015);

    let min_speech_ms = 200usize;
    let pad_ms = 240usize;
    let keep_silence_ms = 800usize;
    let min_speech_frames = (min_speech_ms + frame_ms - 1) / frame_ms;
    let pad_frames = (pad_ms + frame_ms - 1) / frame_ms;
    let keep_silence_frames = (keep_silence_ms + frame_ms - 1) / frame_ms;

    let mut raw_segments: Vec<(usize, usize)> = Vec::new();
    let mut current_start: Option<usize> = None;
    for (idx, &energy) in energies.iter().enumerate() {
        let frame_threshold = if idx >= tail_start {
            tail_threshold
        } else {
            threshold
        };
        if energy >= frame_threshold {
            if current_start.is_none() {
                current_start = Some(idx);
            }
        } else if let Some(start) = current_start.take() {
            let end = idx.saturating_sub(1);
            if end + 1 - start >= min_speech_frames {
                raw_segments.push((start, end));
            }
        }
    }
    if let Some(start) = current_start.take() {
        let end = num_frames.saturating_sub(1);
        if end + 1 - start >= min_speech_frames {
            raw_segments.push((start, end));
        }
    }

    if raw_segments.is_empty() {
        samples.clear();
        return Some(VadResult {
            removed_samples: original_len,
            kept_samples: 0,
            segments: 0,
            threshold,
            noise_floor,
            keep_silence_ms,
            pad_ms,
        });
    }

    let mut padded: Vec<(usize, usize)> = Vec::with_capacity(raw_segments.len());
    for (start, end) in raw_segments {
        let start = start.saturating_sub(pad_frames);
        let end = (end + pad_frames).min(num_frames.saturating_sub(1));
        padded.push((start, end));
    }

    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in padded {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 + keep_silence_frames {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    if let Some(tail_start) = dynamic_tail_start {
        if let Some(last) = merged.last_mut() {
            let tail_start = tail_start.max(last.1 + 1);
            if tail_start > last.1 + 1 {
                last.1 = tail_start.saturating_sub(1).min(num_frames.saturating_sub(1));
            }
        }
    }

    if merged.len() == 1 && merged[0].0 == 0 && merged[0].1 + 1 >= num_frames {
        return None;
    }

    let mut new_samples = Vec::with_capacity(original_len);
    let insert_silence_ms = 120usize;
    let insert_silence_len = (sample_rate as usize * insert_silence_ms) / 1000;
    for (idx, (start_frame, end_frame)) in merged.iter().enumerate() {
        let start_sample = (start_frame * frame_len).min(samples.len());
        let end_sample = ((end_frame + 1) * frame_len).min(samples.len());
        if start_sample < end_sample {
            new_samples.extend_from_slice(&samples[start_sample..end_sample]);
            if idx + 1 < merged.len() && insert_silence_len > 0 {
                new_samples.resize(new_samples.len() + insert_silence_len, 0.0);
            }
        }
    }

    let kept_samples = new_samples.len();
    let removed_samples = original_len.saturating_sub(kept_samples);
    if removed_samples == 0 {
        return None;
    }
    *samples = new_samples;
    Some(VadResult {
        removed_samples,
        kept_samples,
        segments: merged.len(),
        threshold,
        noise_floor,
        keep_silence_ms,
        pad_ms,
    })
}

struct TrimResult {
    trimmed_samples: usize,
    trimmed_leading_samples: usize,
    trimmed_trailing_samples: usize,
    threshold: f32,
    noise_floor: f32,
    leading_frames: usize,
    trailing_frames: usize,
}

fn trim_silence(samples: &mut Vec<f32>, sample_rate: u32) -> Option<TrimResult> {
    if samples.is_empty() || sample_rate == 0 {
        return None;
    }
    let original_len = samples.len();
    let frame_ms = 20usize;
    let frame_len = (sample_rate as usize * frame_ms) / 1000;
    if frame_len == 0 {
        return None;
    }
    let num_frames = (samples.len() + frame_len - 1) / frame_len;
    if num_frames == 0 {
        return None;
    }

    let mut energies = Vec::with_capacity(num_frames);
    for i in 0..num_frames {
        let start = i * frame_len;
        let end = std::cmp::min(start + frame_len, samples.len());
        if start >= end {
            energies.push(0.0);
            continue;
        }
        let mut sum = 0.0f32;
        for &sample in &samples[start..end] {
            sum += sample.abs();
        }
        energies.push(sum / (end - start) as f32);
    }

    let mut sorted = energies.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let noise_idx = ((sorted.len() as f32 - 1.0) * 0.1).round() as usize;
    let noise_floor = sorted[noise_idx.min(sorted.len() - 1)];
    let threshold = (noise_floor * 2.5).max(0.002);
    let speech_ref = speech_median(&energies, threshold);
    let dynamic_tail_start =
        speech_ref.and_then(|speech_ref| find_dynamic_tail_start(&energies, frame_ms, speech_ref));
    let tail_window_ms = 800usize;
    let tail_frames = (tail_window_ms + frame_ms - 1) / frame_ms;
    let tail_start = num_frames.saturating_sub(tail_frames);
    let tail_threshold = (noise_floor * 2.0).max(0.0015);

    let mut first_loud: Option<usize> = None;
    let mut last_loud: Option<usize> = None;
    for (idx, &energy) in energies.iter().enumerate() {
        let frame_threshold = if idx >= tail_start {
            tail_threshold
        } else {
            threshold
        };
        if energy >= frame_threshold {
            if first_loud.is_none() {
                first_loud = Some(idx);
            }
            last_loud = Some(idx);
        }
    }

    let Some(first_loud) = first_loud else {
        samples.clear();
        return Some(TrimResult {
            trimmed_samples: original_len,
            trimmed_leading_samples: original_len,
            trimmed_trailing_samples: 0,
            threshold,
            noise_floor,
            leading_frames: num_frames,
            trailing_frames: 0,
        });
    };
    let last_loud = last_loud.unwrap_or(first_loud);

    let min_leading_silence_ms = 300usize;
    let min_trailing_silence_ms = 400usize;
    let pad_before_ms = 200usize;
    let pad_after_ms = 240usize;

    let min_leading_frames = (min_leading_silence_ms + frame_ms - 1) / frame_ms;
    let min_trailing_frames = (min_trailing_silence_ms + frame_ms - 1) / frame_ms;
    let pad_before_frames = (pad_before_ms + frame_ms - 1) / frame_ms;
    let pad_after_frames = (pad_after_ms + frame_ms - 1) / frame_ms;

    let leading_frames = first_loud;
    let mut trailing_frames = num_frames.saturating_sub(last_loud + 1);

    let start_frame = if leading_frames >= min_leading_frames {
        first_loud.saturating_sub(pad_before_frames)
    } else {
        0
    };
    let mut end_frame = if trailing_frames >= min_trailing_frames {
        (last_loud + 1 + pad_after_frames).min(num_frames)
    } else {
        num_frames
    };
    if let Some(tail_start) = dynamic_tail_start {
        let tail_start = tail_start.max(last_loud + 1);
        let tail_frames = num_frames.saturating_sub(tail_start);
        if tail_frames >= min_trailing_frames {
            trailing_frames = tail_frames;
            end_frame = (tail_start + pad_after_frames).min(num_frames);
        }
    }

    if start_frame == 0 && end_frame == num_frames {
        return None;
    }

    let start_sample = (start_frame * frame_len).min(samples.len());
    let end_sample = (end_frame * frame_len).min(samples.len());
    if start_sample >= end_sample {
        samples.clear();
        return Some(TrimResult {
            trimmed_samples: original_len,
            trimmed_leading_samples: original_len,
            trimmed_trailing_samples: 0,
            threshold,
            noise_floor,
            leading_frames,
            trailing_frames,
        });
    }

    let trimmed_leading_samples = start_sample;
    let trimmed_trailing_samples = original_len - end_sample;
    let keep_len = end_sample - start_sample;
    samples.copy_within(start_sample..end_sample, 0);
    samples.truncate(keep_len);
    let trimmed_samples = trimmed_leading_samples + trimmed_trailing_samples;
    Some(TrimResult {
        trimmed_samples,
        trimmed_leading_samples,
        trimmed_trailing_samples,
        threshold,
        noise_floor,
        leading_frames,
        trailing_frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::ensure;
    use tempfile::tempdir;

    #[test]
    #[ignore = "requires model download and audio input"]
    fn e2e_transcribe_silence() -> Result<()> {
        let dir = tempdir()?;
        let wav = dir.path().join("silence.wav");
        write_silence_wav(&wav)?;
        let model_dir = PathBuf::from(".models");
        let preferred = ["tiny", "base", "small", "turbo", "medium", "large"];
        let model_name = preferred
            .iter()
            .copied()
            .find(|name| {
                crate::model::model_info(name)
                    .map(|info| model_dir.join(info.filename).exists())
                    .unwrap_or(false)
            })
            .unwrap_or("tiny");
        let model_path = crate::model::ensure_model(&model_dir, model_name)?;
        let transcriber = WhisperTranscriber::new(model_path)?;
        let text = transcriber.transcribe_file(&wav)?;
        let has_alnum = text.chars().any(|c| c.is_alphanumeric());
        ensure!(
            !has_alnum,
            "expected silence (no alphanumeric tokens), got: {text:?}"
        );
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
