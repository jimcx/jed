//! File watching. notify's callback is synchronous; it forwards paths into the app's channel.

use std::path::{Path, PathBuf};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// Stops watching when dropped.
pub struct Watcher {
    inner: RecommendedWatcher,
}

impl Watcher {
    /// Also watches `path` (recursively), reporting through the same callback: the git
    /// directory when it's outside the workspace root.
    pub fn watch(&mut self, path: &Path) -> notify::Result<()> {
        self.inner.watch(path, RecursiveMode::Recursive)
    }
}

pub fn start(root: &Path, on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> notify::Result<Watcher> {
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
        Ok(ev) if !matches!(ev.kind, EventKind::Access(_)) => on_change(ev.paths),
        Ok(_) => {}
        Err(e) => tracing::warn!("watch error: {e}"),
    })?;
    w.watch(root, RecursiveMode::Recursive)?;
    Ok(Watcher { inner: w })
}
