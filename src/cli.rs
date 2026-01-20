use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "dictate", version, about = "Background transcription app")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Run(RunArgs),
    Transcribe(TranscribeArgs),
    /// List available models, sizes, and language support.
    Models,
}

#[derive(Parser, Debug, Clone)]
pub struct RunArgs {
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long, default_value = ".recordings")]
    pub recordings_dir: PathBuf,
}

impl Default for RunArgs {
    fn default() -> Self {
        Self {
            model: None,
            recordings_dir: PathBuf::from(".recordings"),
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct TranscribeArgs {
    #[arg(long)]
    pub input: PathBuf,
    #[arg(long)]
    pub model: Option<String>,
    /// Force a language (e.g. "en", "ru"); omit for auto-detect.
    #[arg(long)]
    pub language: Option<String>,
}
