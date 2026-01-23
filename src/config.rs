use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub selected_mic: Option<String>,
    pub model: String,
    pub recordings_dir: PathBuf,
    pub vocabulary: Vec<String>,
    pub auto_transcribe: Option<AutoTranscribeConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AutoTranscribeConfig {
    pub watches: Vec<WatchPair>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WatchPair {
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub processed_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            selected_mic: None,
            model: "small".to_string(),
            recordings_dir: PathBuf::from(".recordings"),
            vocabulary: Vec::new(),
            auto_transcribe: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new() -> Result<Self> {
        let base = BaseDirs::new().context("unable to resolve home directory")?;
        let path = base.home_dir().join(".config").join("dictate.yaml");
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Config> {
        if !self.path.exists() {
            return Ok(Config::default());
        }
        let contents = fs::read_to_string(&self.path)
            .with_context(|| format!("read config {}", self.path.display()))?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {}", parent.display()))?;
        }
        let contents = serde_yaml::to_string(config)?;
        fs::write(&self.path, contents)
            .with_context(|| format!("write config {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_roundtrip() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("dictate.yaml");
        let store = ConfigStore { path };
        let mut cfg = Config::default();
        cfg.selected_mic = Some("Test Mic".to_string());
        cfg.model = "tiny".to_string();
        cfg.recordings_dir = PathBuf::from("custom");
        cfg.vocabulary = vec!["Dictate".to_string(), "Whisper".to_string()];
        cfg.auto_transcribe = Some(AutoTranscribeConfig {
            watches: vec![WatchPair {
                input_dir: PathBuf::from("input"),
                output_dir: PathBuf::from("output"),
                processed_dir: PathBuf::from("processed"),
            }],
        });
        store.save(&cfg)?;
        let loaded = store.load()?;
        assert_eq!(loaded.selected_mic, cfg.selected_mic);
        assert_eq!(loaded.model, cfg.model);
        assert_eq!(loaded.recordings_dir, cfg.recordings_dir);
        assert_eq!(loaded.vocabulary, cfg.vocabulary);
        assert_eq!(
            loaded
                .auto_transcribe
                .as_ref()
                .and_then(|c| c.watches.first())
                .map(|watch| (&watch.input_dir, &watch.output_dir, &watch.processed_dir)),
            cfg.auto_transcribe
                .as_ref()
                .and_then(|c| c.watches.first())
                .map(|watch| (&watch.input_dir, &watch.output_dir, &watch.processed_dir))
        );
        Ok(())
    }
}
