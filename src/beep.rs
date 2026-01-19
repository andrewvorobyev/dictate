use anyhow::{Context, Result};
use rodio::{source::SineWave, OutputStream, Sink, Source};
use std::time::Duration;

pub fn play() -> Result<()> {
    let (_stream, handle) = OutputStream::try_default().context("open audio output")?;
    let sink = Sink::try_new(&handle).context("create sink")?;
    let source = SineWave::new(880.0)
        .take_duration(Duration::from_millis(120))
        .amplify(0.2);
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}
