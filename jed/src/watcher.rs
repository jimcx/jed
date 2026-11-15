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

    /// Also watches `path`'s direct children, not its subdirectories: the user config directory,
    /// whose `settings.toml` and `keybindings.toml` reload live (its `languages/` don't).
    pub fn watch_dir(&mut self, path: &Path) -> notify::Result<()> {
        self.inner.watch(path, RecursiveMode::NonRecursive)
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

/// Watches only `file` (single file mode): its directory, not recursively — editors save by
/// renaming over the file, which a watch on its inode would miss — with only `file`'s events let
/// through, plus `config_dir`'s direct children (the user's `settings.toml` / `keybindings.toml`,
/// which reload live; `config_dir` itself is watched by `Watcher::watch_dir`).
pub fn start_file(
    file: &Path,
    config_dir: &Path,
    on_change: impl Fn(Vec<PathBuf>) + Send + 'static,
) -> notify::Result<Watcher> {
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let target = file.to_path_buf();
    let config = config_dir.to_path_buf();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
        Ok(ev) if !matches!(ev.kind, EventKind::Access(_)) => {
            let paths: Vec<PathBuf> = ev
                .paths
                .into_iter()
                .filter(|p| *p == target || p.parent().is_some_and(|d| d == config.as_path()))
                .collect();
            if !paths.is_empty() {
                on_change(paths);
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("watch error: {e}"),
    })?;
    w.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(Watcher { inner: w })
}
