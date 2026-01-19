use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "dictate-2", version, about = "Background transcription app")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Run(RunArgs),
    Transcribe(TranscribeArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct RunArgs {
    #[arg(long, default_value = "turbo")]
    pub model: String,
    #[arg(long, default_value = ".recordings")]
    pub recordings_dir: PathBuf,
}

impl Default for RunArgs {
    fn default() -> Self {
        Self {
            model: "turbo".to_string(),
            recordings_dir: PathBuf::from(".recordings"),
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct TranscribeArgs {
    #[arg(long)]
    pub input: PathBuf,
    #[arg(long, default_value = "turbo")]
    pub model: String,
}
