use anyhow::{Context, Result};
use chrono::Local;
use std::fs;
use std::path::{Path, PathBuf};

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create dir {}", path.display()))?;
    Ok(())
}

pub fn iso_timestamp() -> String {
    let now = Local::now();
    now.format("%Y-%m-%dT%H-%M-%S%.3f%z").to_string()
}

pub fn next_recording_paths(recordings_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    ensure_dir(recordings_dir)?;
    let stamp = iso_timestamp();
    let audio = recordings_dir.join(format!("{stamp}.m4a"));
    let text = recordings_dir.join(format!("{stamp}.md"));
    Ok((audio, text))
}

pub fn transcript_path_for_input(input: &Path) -> Result<PathBuf> {
    let parent = input
        .parent()
        .context("input file has no parent directory")?;
    let stem = input
        .file_stem()
        .context("input file has no filename")?
        .to_string_lossy();
    Ok(parent.join(format!("{stem}.md")))
}

pub fn transcript_path_for_output_dir(input: &Path, output_dir: &Path) -> Result<PathBuf> {
    let stem = input
        .file_stem()
        .context("input file has no filename")?
        .to_string_lossy();
    let safe = sanitize_filename_component(&stem);
    Ok(output_dir.join(format!("{safe}.md")))
}

pub fn processed_path_for_input(input: &Path, processed_dir: &Path) -> Result<PathBuf> {
    let name = input
        .file_name()
        .context("input file has no filename")?;
    let safe = sanitize_filename_component(&name.to_string_lossy());
    Ok(processed_dir.join(safe))
}

fn sanitize_filename_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| if is_illegal_filename_char(ch) { '-' } else { ch })
        .collect()
}

fn is_illegal_filename_char(ch: char) -> bool {
    matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        || ch.is_ascii_control()
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use tempfile::tempdir;

    #[test]
    fn iso_timestamp_has_timezone_and_ms() {
        let stamp = iso_timestamp();
        let re = Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}\.\d{3}[+-]\d{4}$")
            .expect("regex");
        assert!(re.is_match(&stamp), "stamp format unexpected: {stamp}");
    }

    #[test]
    fn next_recording_paths_create_in_dir() -> Result<()> {
        let dir = tempdir()?;
        let (audio, text) = next_recording_paths(dir.path())?;
        assert!(audio.starts_with(dir.path()));
        assert!(text.starts_with(dir.path()));
        assert!(audio.extension().unwrap_or_default() == "m4a");
        assert!(text.extension().unwrap_or_default() == "md");
        Ok(())
    }

    #[test]
    fn transcript_path_for_output_dir_uses_stem() -> Result<()> {
        let dir = tempdir()?;
        let input = dir.path().join("2024-06-01T12-00-00.m4a");
        let output_dir = dir.path().join("out");
        let out = transcript_path_for_output_dir(&input, &output_dir)?;
        assert_eq!(out, output_dir.join("2024-06-01T12-00-00.md"));
        Ok(())
    }

    #[test]
    fn processed_path_for_input_preserves_filename() -> Result<()> {
        let dir = tempdir()?;
        let input = dir.path().join("sample.m4a");
        let processed_dir = dir.path().join("processed");
        let out = processed_path_for_input(&input, &processed_dir)?;
        assert_eq!(out, processed_dir.join("sample.m4a"));
        Ok(())
    }

    #[test]
    fn transcript_path_for_output_dir_sanitizes_illegal_chars() -> Result<()> {
        let dir = tempdir()?;
        let input = dir.path().join("2026-01-22T16:19:59.m4a");
        let output_dir = dir.path().join("out");
        let out = transcript_path_for_output_dir(&input, &output_dir)?;
        assert_eq!(out, output_dir.join("2026-01-22T16-19-59.md"));
        Ok(())
    }

    #[test]
    fn processed_path_for_input_sanitizes_illegal_chars() -> Result<()> {
        let dir = tempdir()?;
        let input = dir.path().join("clip:01.m4a");
        let processed_dir = dir.path().join("processed");
        let out = processed_path_for_input(&input, &processed_dir)?;
        assert_eq!(out, processed_dir.join("clip-01.m4a"));
        Ok(())
    }
}
