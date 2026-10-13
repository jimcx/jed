//! File logging. Nothing may write to stdout/stderr while the UI is up.

use std::path::PathBuf;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::paths;

pub fn log_dir() -> PathBuf {
    paths::state_dir().join("logs")
}

/// `filter` comes from `--log-level`; otherwise `RED_LOG`, otherwise `info`.
/// Both use `EnvFilter` syntax, e.g. `red::buffer=debug,input=trace`.
pub fn init(filter: Option<&str>) -> anyhow::Result<()> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("red")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)?;
    let filter = match filter {
        Some(f) => EnvFilter::try_new(f)?,
        None => EnvFilter::try_from_env("RED_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
    };
    // Blocking writer: the panic hook exits the process, which would lose a non-blocking writer's queue.
    tracing_subscriber::fmt()
        .with_writer(appender)
        .with_ansi(false)
        .with_env_filter(filter)
        .init();
    Ok(())
}
