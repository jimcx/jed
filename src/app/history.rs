//! File history: snapshots recorded after saves, moved with explorer renames (`crate::history`
//! does the disk work, off-thread).

use std::path::{Path, PathBuf};

use super::{App, AppEvent, Level};
use crate::history::{self, Recorded};
use crate::workspace::config::HistoryConfig;

/// History tasks in flight and finished (tests wait on these).
#[derive(Default)]
pub struct HistoryTasks {
    running: usize,
    pub done: u64,
}

impl HistoryTasks {
    #[cfg_attr(not(test), allow(dead_code))] // tests
    pub fn running(&self) -> usize {
        self.running
    }
}

/// What recording a save's snapshot needs; `None` from `App::history_job` when the file has no
/// history (outside the workspace, in `.red/`, history disabled).
pub(super) struct Job {
    root: PathBuf,
    rel: PathBuf,
    limits: HistoryConfig,
}

impl Job {
    /// Blocking: call from spawn_blocking.
    pub(super) fn record(&self, bytes: &[u8]) -> Result<(), String> {
        let rel = self.rel.display();
        match history::record(&self.root, &self.rel, bytes, &self.limits) {
            Ok(Recorded::Added(entry)) => {
                tracing::debug!(target: "red::history", %rel, id = entry.id, size = entry.size, "snapshot recorded");
                Ok(())
            }
            Ok(other) => {
                tracing::debug!(target: "red::history", %rel, ?other, "no snapshot");
                Ok(())
            }
            Err(e) => Err(format!("File history of {rel} not saved: {e}")),
        }
    }
}

impl App {
    /// The history snapshot job for a save of `path`, if it gets one.
    pub(super) fn history_job(&self, path: &Path) -> Option<Job> {
        let limits = self.workspace.config.history.clone();
        if !limits.enabled {
            return None;
        }
        let root = self.workspace.root.clone();
        let rel = history::relative(&root, path)?;
        Some(Job { root, rel, limits })
    }

    /// A save that will send `AppEvent::HistoryDone` once its snapshot is recorded.
    pub(super) fn history_started(&mut self) {
        self.file_history.running += 1;
    }

    /// After an explorer rename of `from` to `to` (run off-thread, after the file op): the
    /// history of the file, or of every file under the folder, moves too (a delete keeps it).
    /// Sends `AppEvent::HistoryDone`; `None` when neither path has a history.
    pub(super) fn history_rename(
        &mut self,
        from: &Path,
        to: &Path,
    ) -> Option<impl FnOnce() -> Result<(), String> + Send + use<>> {
        let root = self.workspace.root.clone();
        let (from, to) = (history::relative(&root, from)?, history::relative(&root, to)?);
        self.history_started();
        Some(move || {
            let moved = history::rename(&root, &from, &to)
                .map_err(|e| format!("File history of {} not moved: {e}", from.display()))?;
            tracing::debug!(target: "red::history", from = %from.display(), to = %to.display(), moved, "history moved");
            Ok(())
        })
    }

    pub(super) fn history_done(&mut self, result: Result<(), String>) {
        self.file_history.running = self.file_history.running.saturating_sub(1);
        self.file_history.done += 1;
        if let Err(e) = result {
            self.notify(Level::Warning, e);
        }
    }

    /// File system events under `.red/` (history writes) aren't changes anyone needs to see.
    pub(super) fn fs_changed(&mut self, mut paths: Vec<std::path::PathBuf>) {
        let red = self.workspace.root.join(history::RED_DIR);
        paths.retain(|p| !p.starts_with(&red));
        if paths.is_empty() {
            return;
        }
        self.fs_pending.extend(paths);
        self.fs_deadline
            .get_or_insert_with(|| tokio::time::Instant::now() + super::FS_DEBOUNCE);
    }
}

/// Sends a history task's result.
pub(super) fn send_done(tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>, result: Result<(), String>) {
    let _ = tx.send(AppEvent::HistoryDone(result));
}
