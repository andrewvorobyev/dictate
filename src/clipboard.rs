use anyhow::{Context, Result};

pub struct Clipboard;

impl Clipboard {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn set_text(&mut self, text: &str) -> Result<()> {
        let mut clipboard = arboard::Clipboard::new().context("init clipboard")?;
        clipboard.set_text(text.to_string()).context("set clipboard")?;
        Ok(())
    }
}
