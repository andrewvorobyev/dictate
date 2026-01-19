use tracing_subscriber::fmt::time::UtcTime;

pub fn init() {
    let _ = tracing_subscriber::fmt()
        .with_timer(UtcTime::rfc_3339())
        .with_target(false)
        .with_level(true)
        .try_init();
}
