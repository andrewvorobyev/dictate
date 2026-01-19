use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub name: String,
    pub filename: String,
    pub url: String,
}

pub fn model_info(name: &str) -> Result<ModelInfo> {
    let filename = match name {
        "turbo" => "ggml-large-v3-turbo.bin",
        "tiny" => "ggml-tiny.bin",
        "base" => "ggml-base.bin",
        "small" => "ggml-small.bin",
        "medium" => "ggml-medium.bin",
        "large" => "ggml-large.bin",
        _ => {
            return Err(anyhow::anyhow!(
                "unknown model '{name}'. Try: turbo, tiny, base, small, medium, large"
            ))
        }
    };
    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{filename}"
    );
    Ok(ModelInfo {
        name: name.to_string(),
        filename: filename.to_string(),
        url,
    })
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
        return Ok(target);
    }
    download_model(&info, &target, &mut progress)?;
    Ok(target)
}

fn download_model<F>(info: &ModelInfo, target: &Path, progress: &mut F) -> Result<()>
where
    F: FnMut(u8),
{
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
    let mut file = File::create(target)
        .with_context(|| format!("create model file {}", target.display()))?;
    let mut buf = [0u8; 8192];
    let mut downloaded = 0u64;
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
            progress(pct.min(100));
        }
    }
    pb.finish_with_message(format!("downloaded {}", info.name));
    Ok(())
}
