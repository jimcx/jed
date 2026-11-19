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

/// `notify::recommended_watcher`. In tests it waits (up to 30 s) for a free inotify instance
/// when the per-user limit (`fs.inotify.max_user_instances`, often 128) is used up: several
/// parallel test runs each start hundreds of watching apps, and a test whose watcher failed
/// would wait out its settle for events that never come.
fn new_watcher(handler: impl notify::EventHandler + Clone) -> notify::Result<RecommendedWatcher> {
    #[cfg(test)]
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match notify::recommended_watcher(handler.clone()) {
                Err(e) if out_of_instances(&e) && std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                r => return r,
            }
        }
    }
    #[cfg(not(test))]
    notify::recommended_watcher(handler)
}

/// EMFILE from `inotify_init`: the user's inotify instances are all in use.
#[cfg(test)]
fn out_of_instances(e: &notify::Error) -> bool {
    matches!(&e.kind, notify::ErrorKind::Io(io) if io.raw_os_error() == Some(24))
}

pub fn start(root: &Path, on_change: impl Fn(Vec<PathBuf>) + Send + Sync + 'static) -> notify::Result<Watcher> {
    let on_change = std::sync::Arc::new(on_change);
    let mut w = new_watcher(move |res: notify::Result<notify::Event>| match res {
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
    on_change: impl Fn(Vec<PathBuf>) + Send + Sync + 'static,
) -> notify::Result<Watcher> {
    let on_change = std::sync::Arc::new(on_change);
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let target = file.to_path_buf();
    let config = config_dir.to_path_buf();
    let mut w = new_watcher(move |res: notify::Result<notify::Event>| match res {
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
