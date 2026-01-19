use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub enum LanguageSupport {
    English,
    Multilingual,
}

impl LanguageSupport {
    pub fn label(self) -> &'static str {
        match self {
            LanguageSupport::English => "english",
            LanguageSupport::Multilingual => "multilingual",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ModelSpec {
    name: &'static str,
    filename: &'static str,
    size_bytes: u64,
    description: &'static str,
    languages: LanguageSupport,
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

const MODEL_SPECS: &[ModelSpec] = &[
    ModelSpec {
        name: "turbo",
        filename: "ggml-large-v3-turbo.bin",
        size_bytes: (3 * GIB) / 2,
        description: "Fast large-v3 turbo; strong speed/quality balance.",
        languages: LanguageSupport::Multilingual,
    },
    ModelSpec {
        name: "tiny",
        filename: "ggml-tiny.bin",
        size_bytes: 75 * MIB,
        description: "Smallest and fastest; lowest accuracy.",
        languages: LanguageSupport::Multilingual,
    },
    ModelSpec {
        name: "base",
        filename: "ggml-base.bin",
        size_bytes: 142 * MIB,
        description: "Compact model with better accuracy than tiny.",
        languages: LanguageSupport::Multilingual,
    },
    ModelSpec {
        name: "small",
        filename: "ggml-small.bin",
        size_bytes: 466 * MIB,
        description: "Good accuracy; moderate CPU/RAM usage.",
        languages: LanguageSupport::Multilingual,
    },
    ModelSpec {
        name: "medium",
        filename: "ggml-medium.bin",
        size_bytes: (3 * GIB) / 2,
        description: "High accuracy; slower on CPU.",
        languages: LanguageSupport::Multilingual,
    },
    ModelSpec {
        name: "large",
        filename: "ggml-large.bin",
        size_bytes: (29 * GIB) / 10,
        description: "Best accuracy; largest and slowest.",
        languages: LanguageSupport::Multilingual,
    },
];

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub name: &'static str,
    pub filename: &'static str,
    pub url: String,
    pub size_bytes: u64,
    pub description: &'static str,
    pub languages: LanguageSupport,
}

pub fn available_models() -> Vec<ModelInfo> {
    MODEL_SPECS.iter().map(model_from_spec).collect()
}

pub fn model_info(name: &str) -> Result<ModelInfo> {
    let spec = MODEL_SPECS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| {
            let available = MODEL_SPECS
                .iter()
                .map(|spec| spec.name)
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::anyhow!("unknown model '{name}'. Try: {available}")
        })?;
    Ok(model_from_spec(spec))
}

pub fn ensure_model(models_dir: &Path, name: &str) -> Result<PathBuf> {
    ensure_model_with_progress(models_dir, name, |_| {})
}

pub fn ensure_model_with_progress<F>(models_dir: &Path, name: &str, mut progress: F) -> Result<PathBuf>
where
    F: FnMut(u8),
{
    fs::create_dir_all(models_dir)
        .with_context(|| format!("create models dir {}", models_dir.display()))?;
    let info = model_info(name)?;
    let target = models_dir.join(&info.filename);
    if target.exists() {
        tracing::info!(path = %target.display(), model = %info.name, "model already present");
        return Ok(target);
    }
    tracing::info!(
        path = %target.display(),
        url = %info.url,
        model = %info.name,
        "downloading model"
    );
    download_model(&info, &target, &mut progress)?;
    tracing::info!(
        path = %target.display(),
        model = %info.name,
        "model download complete"
    );
    Ok(target)
}

fn download_model<F>(info: &ModelInfo, target: &Path, progress: &mut F) -> Result<()>
where
    F: FnMut(u8),
{
    let tmp = target.with_extension("partial");
    if tmp.exists() {
        fs::remove_file(&tmp)
            .with_context(|| format!("remove partial model {}", tmp.display()))?;
    }
    let mut resp = reqwest::blocking::get(&info.url)
        .with_context(|| format!("download model {}", info.url))?;
    let total = resp.content_length().unwrap_or(0);
    let pb = if total > 0 {
        ProgressBar::new(total)
    } else {
        ProgressBar::new_spinner()
    };
    pb.set_style(
        ProgressStyle::with_template("{spinner} {bytes}/{total_bytes} ({eta})")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    let mut file =
        File::create(&tmp).with_context(|| format!("create model file {}", tmp.display()))?;
    let mut buf = [0u8; 8192];
    let mut downloaded = 0u64;
    let mut last_pct: Option<u8> = None;
    loop {
        let read = resp.read(&mut buf)?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])?;
        pb.inc(read as u64);
        if total > 0 {
            downloaded += read as u64;
            let pct = ((downloaded as f64 / total as f64) * 100.0).round() as u8;
            let pct = pct.min(100);
            if last_pct != Some(pct) {
                progress(pct);
                last_pct = Some(pct);
            }
        }
    }
    fs::rename(&tmp, target)
        .with_context(|| format!("finalize model {}", target.display()))?;
    pb.finish_with_message(format!("downloaded {}", info.name));
    Ok(())
}

fn model_from_spec(spec: &ModelSpec) -> ModelInfo {
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        spec.filename
    );
    ModelInfo {
        name: spec.name,
        filename: spec.filename,
        url,
        size_bytes: spec.size_bytes,
        description: spec.description,
        languages: spec.languages,
    }
}

pub fn format_size(bytes: u64) -> String {
    let bytes_f = bytes as f64;
    let gib = GIB as f64;
    let mib = MIB as f64;
    if bytes_f >= gib {
        let value = bytes_f / gib;
        if value >= 10.0 {
            format!("{value:.0} GB")
        } else {
            format!("{value:.1} GB")
        }
    } else {
        let value = bytes_f / mib;
        format!("{value:.0} MB")
    }
}
