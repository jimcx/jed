//! File logging. Nothing may write to stdout/stderr while the UI is up.

use std::path::{Path, PathBuf};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, reload};

use crate::paths;

/// Changes the log filter while running ("Set Log Level").
pub type LevelHandle = reload::Handle<EnvFilter, Registry>;

pub fn log_dir() -> PathBuf {
    paths::state_dir().join("logs")
}

/// `filter` comes from `--log-level`; otherwise `RED_LOG`, otherwise `info`.
/// Both use `EnvFilter` syntax, e.g. `red::buffer=debug,input=trace`.
pub fn init(filter: Option<&str>) -> anyhow::Result<LevelHandle> {
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
    let (filter, handle) = reload::Layer::new(filter);
    // Blocking writer: the panic hook exits the process, which would lose a non-blocking writer's queue.
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(appender).with_ansi(false))
        .init();
    Ok(handle)
}

/// The filter in use, in `EnvFilter` syntax.
pub fn current_filter(handle: &LevelHandle) -> Option<String> {
    handle.with_current(|f| f.to_string()).ok()
}

/// Replaces the filter; `filter` uses `EnvFilter` syntax (`debug`, `red::lsp=trace,info`, …).
pub fn set_filter(handle: &LevelHandle, filter: &str) -> Result<(), String> {
    let new = EnvFilter::try_new(filter).map_err(|e| e.to_string())?;
    handle.reload(new).map_err(|e| e.to_string())?;
    tracing::info!(filter, "log filter changed");
    Ok(())
}

/// Blocking. The newest `red.*.log` in `dir` (daily files sort by date).
pub fn newest_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("red.") && n.ends_with(".log"))
        })
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_log_by_date() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(newest_log(dir.path()), None);
        for name in [
            "red.2026-10-12.log",
            "red.2026-10-14.log",
            "other.log",
            "red.2026-10-13.log",
        ] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        assert_eq!(newest_log(dir.path()), Some(dir.path().join("red.2026-10-14.log")));
    }

    #[test]
    fn filter_reloads() {
        let (layer, handle) = reload::Layer::new(EnvFilter::new("info"));
        let _subscriber = tracing_subscriber::registry().with(layer);
        assert_eq!(current_filter(&handle).as_deref(), Some("info"));
        set_filter(&handle, "red::app=trace").unwrap();
        assert_eq!(current_filter(&handle).as_deref(), Some("red::app=trace"));
        assert!(set_filter(&handle, "red::app=loud").is_err());
        assert_eq!(current_filter(&handle).as_deref(), Some("red::app=trace"));
    }
}
