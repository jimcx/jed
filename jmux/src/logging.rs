//! `tracing` to one file per server under `<state dir>/jmux/logs`; nothing is printed (a server's
//! stdio is /dev/null). Filter: `JMUX_LOG` (`EnvFilter` syntax), else `info`.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub fn log_dir() -> PathBuf {
    jtui::paths::state_dir(crate::APP).join("logs")
}

/// The log file of the server of session `name` running as process `pid`.
pub fn log_path(dir: &Path, name: &str, pid: u32) -> PathBuf {
    dir.join(format!("{name}-{pid}.log"))
}

/// Installs the subscriber writing to `path` (its folder is created) and a panic hook that logs
/// the panic first. Blocking: runs once at start, before the event loop.
pub fn init(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = File::create(path)?;
    let filter = EnvFilter::try_from_env("JMUX_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(Mutex::new(file))
        .with_ansi(false);
    if tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .is_err()
    {
        return Ok(());
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(target: "panic", "{info}");
        previous(info);
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_file_per_server() {
        let dir = Path::new("/x/logs");
        assert_eq!(log_path(dir, "work", 7), Path::new("/x/logs/work-7.log"));
        assert_ne!(log_path(dir, "work", 8), log_path(dir, "work", 7));
    }
}
