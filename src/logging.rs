use std::fs;

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// File-only logging: the terminal belongs to the TUI. Returns the guard that
/// must stay alive for the process lifetime or buffered logs are dropped.
pub fn init() -> Result<WorkerGuard> {
    let dir = crate::config::state_dir();
    fs::create_dir_all(&dir)?;
    let appender = tracing_appender::rolling::daily(&dir, "maplet.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(writer)
        .with_ansi(false)
        .init();
    Ok(guard)
}
