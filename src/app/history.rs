//! File history: snapshots recorded after saves, moved with explorer renames, and "File
//! History: Show" / "Restore": the active file's entries in the palette, an entry compared
//! with the buffer in a diff tab (`Pair::history`), or restored into the buffer as one undo step
//! (`crate::history` does the disk work, off-thread).

use std::path::{Path, PathBuf};

use super::{App, AppEvent, Level};
use crate::buffer::BufferId;
use crate::buffer::change::ChangeSet;
use crate::buffer::file::{self, LoadError};
use crate::diff_view::Pair;
use crate::history::{self, Entry, Recorded, Snapshot};
use crate::palette::Picker;
use crate::workspace::config::HistoryConfig;

/// History tasks in flight and finished (tests wait on these), and the history picker's list.
#[derive(Default)]
pub struct HistoryTasks {
    running: usize,
    pub done: u64,
    /// The entries the palette's `Picker::History` lists, while it's open.
    pub picker: Option<HistoryPicker>,
    /// Bumped by each "File History: Show" / "Restore": an older listing's result is dropped.
    listing: u64,
    /// Restores reading their entry (tests wait on these).
    pub restoring: usize,
}

/// The file whose history the palette lists, and its entries (newest first).
pub struct HistoryPicker {
    /// Absolute.
    pub path: PathBuf,
    /// Workspace-relative.
    pub rel: PathBuf,
    /// Accepting restores the entry instead of comparing it.
    pub restore: bool,
    pub entries: Vec<Entry>,
}

/// A file's entries read for the picker (`AppEvent::HistoryListed`).
pub(super) struct Listed {
    listing: u64,
    path: PathBuf,
    rel: PathBuf,
    restore: bool,
    result: Result<Vec<Entry>, String>,
}

/// An entry read and diffed against the buffer for a restore (`AppEvent::HistoryRestore`).
pub(super) struct Restore {
    buffer: BufferId,
    /// The buffer's revision the changes apply to.
    revision: u64,
    path: PathBuf,
    rel: PathBuf,
    snapshot: Snapshot,
    /// From the buffer's text to the entry's.
    result: Result<ChangeSet, String>,
}

impl HistoryTasks {
    #[cfg_attr(not(test), allow(dead_code))] // tests
    pub fn running(&self) -> usize {
        self.running
    }
}

/// What recording a save's snapshot needs; `None` from `App::history_job` when the file has no
/// history (outside the workspace, in `.jed/`, history disabled).
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
                tracing::debug!(target: "jed::history", %rel, id = entry.id, size = entry.size, "snapshot recorded");
                Ok(())
            }
            Ok(other) => {
                tracing::debug!(target: "jed::history", %rel, ?other, "no snapshot");
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
            tracing::debug!(target: "jed::history", from = %from.display(), to = %to.display(), moved, "history moved");
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

    /// File system events under `.jed/` (history writes) aren't changes anyone needs to see.
    pub(super) fn fs_changed(&mut self, mut paths: Vec<std::path::PathBuf>) {
        let jed = self.workspace.root.join(history::JED_DIR);
        paths.retain(|p| !p.starts_with(&jed));
        if paths.is_empty() {
            return;
        }
        self.fs_pending.extend(paths);
        self.fs_deadline
            .get_or_insert_with(|| tokio::time::Instant::now() + super::FS_DEBOUNCE);
    }
}

impl App {
    /// "File History: Show" (`restore` false) / "Restore": lists the active tab's file's entries
    /// (read off-thread) in the palette; a diff tab's file needn't be open. On a history diff
    /// tab, Restore restores its entry.
    pub(super) fn show_file_history(&mut self, restore: bool) {
        if restore
            && let Some(d) = self.group().active_diff()
            && let Some(snapshot) = d.pair.snapshot()
        {
            let path = d.path.clone();
            if let Some(rel) = history::relative(&self.workspace.root, &path) {
                self.restore_history(path, rel, snapshot);
            }
            return;
        }
        let Some(path) = self.active_path() else { return };
        self.list_history(path, restore);
    }

    /// "Explorer: Show File History" (also a right-click on a file's row): the selected file's
    /// entries, whether it's open or not.
    pub(super) fn explorer_file_history(&mut self) {
        let Some(path) = self.explorer.selected.clone() else {
            return;
        };
        if self.explorer.find(&path).is_some_and(|e| e.is_dir) {
            let name = file_name(&path);
            self.notify(
                Level::Info,
                format!("{name} is a folder: file history is kept per file"),
            );
            return;
        }
        self.list_history(path, false);
    }

    /// Lists `path`'s entries off-thread; the picker opens on `AppEvent::HistoryListed`.
    fn list_history(&mut self, path: PathBuf, restore: bool) {
        let name = file_name(&path);
        let root = self.workspace.root.clone();
        let Some(rel) = history::relative(&root, &path) else {
            self.notify(
                Level::Info,
                format!("{name} has no file history: it's outside the workspace"),
            );
            return;
        };
        if restore && self.history_read_only(&path) {
            return;
        }
        self.file_history.listing += 1;
        let listing = self.file_history.listing;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = history::entries(&root, &rel);
            let listed = Listed {
                listing,
                path,
                rel,
                restore,
                result,
            };
            let _ = tx.send(AppEvent::HistoryListed(Box::new(listed)));
        });
    }

    /// The file is open read-only (with a message): a restore can't go ahead. A file that
    /// isn't open can: the restore opens it first.
    fn history_read_only(&mut self, path: &Path) -> bool {
        let Some(buf) = self.buffer_of(path).and_then(|id| self.buffers.get(id)) else {
            return false;
        };
        if let Some(reason) = &buf.read_only {
            let text = format!("Read-only: {reason}");
            self.notify(Level::Error, text);
            return true;
        }
        false
    }

    /// `AppEvent::HistoryListed`: the picker opens with the entries (none: a message).
    pub(super) fn history_listed(&mut self, listed: Listed) {
        if listed.listing != self.file_history.listing {
            return;
        }
        let rel = listed.rel.display().to_string();
        let entries = match listed.result {
            Ok(entries) => entries,
            Err(e) => {
                self.notify(Level::Error, format!("Can't read the file history of {rel}: {e}"));
                return;
            }
        };
        if entries.is_empty() {
            let off = if self.workspace.config.history.enabled {
                "a snapshot is recorded each time you save"
            } else {
                "file history is off in .workspace"
            };
            self.notify(Level::Info, format!("{rel} has no file history yet: {off}"));
            return;
        }
        let restore = listed.restore;
        self.open_palette(Picker::History { restore }, "");
        self.file_history.picker = Some(HistoryPicker {
            path: listed.path,
            rel: listed.rel,
            restore,
            entries,
        });
        self.refresh_palette();
    }

    /// The picker's choice: compare (a diff tab, entry against the buffer) or restore.
    pub(super) fn history_picked(&mut self, picker: HistoryPicker, snapshot: Snapshot) {
        if picker.restore {
            self.restore_history(picker.path, picker.rel, snapshot);
        } else {
            self.open_diff(picker.path, Pair::history(snapshot));
        }
    }

    /// Reads the entry and diffs it against the open buffer of `path` off-thread; the
    /// `AppEvent::HistoryRestore` that follows applies it. Refused for a read-only buffer. A
    /// file that isn't open opens in a tab first (`OpenRequest::restore`), then restores.
    pub(super) fn restore_history(&mut self, path: PathBuf, rel: PathBuf, snapshot: Snapshot) {
        let Some(buffer) = self.buffer_of(&path) else {
            self.open_file(path.clone(), true, None);
            if let Some(request) = self.opening.get_mut(&path) {
                request.restore = Some(snapshot);
            }
            return;
        };
        if self.history_read_only(&path) {
            return;
        }
        let Some(buf) = self.buffers.get(buffer) else { return };
        let (rope, revision) = (buf.rope.clone(), buf.revision());
        let root = self.workspace.root.clone();
        let tx = self.tx.clone();
        self.file_history.restoring += 1;
        tokio::task::spawn_blocking(move || {
            let result = history::read_snapshot(&root, &rel, snapshot).and_then(|bytes| {
                // Decoded as loading the file would: BOM dropped, line endings kept.
                match file::decode(bytes) {
                    Err(LoadError::Binary(_)) => Err("the entry is a binary file".to_string()),
                    Err(LoadError::Io(e)) => Err(e.to_string()),
                    Ok(loaded) if loaded.read_only.is_some() => Err("the entry isn't valid UTF-8".to_string()),
                    Ok(loaded) => Ok(crate::disk::changes(&rope, &loaded.rope)),
                }
            });
            let restore = Restore {
                buffer,
                revision,
                path,
                rel,
                snapshot,
                result,
            };
            let _ = tx.send(AppEvent::HistoryRestore(Box::new(restore)));
        });
    }

    /// `AppEvent::HistoryRestore`: the entry's text replaces the buffer's as one undo step
    /// (`apply_changes`: cursors in unchanged lines stay), not saved. A buffer edited meanwhile
    /// is diffed again.
    pub(super) fn history_restored(&mut self, r: Restore) {
        self.file_history.restoring = self.file_history.restoring.saturating_sub(1);
        let Some(buf) = self.buffers.get(r.buffer).filter(|b| b.path == r.path) else {
            return;
        };
        let (name, ago) = (file_name(&r.path), history::ago(r.snapshot.time, history::now_ms()));
        let changes = match r.result {
            Ok(changes) => changes,
            Err(e) => {
                self.notify(Level::Error, format!("Can't restore {name}: {e}"));
                return;
            }
        };
        if buf.revision() != r.revision {
            return self.restore_history(r.path, r.rel, r.snapshot);
        }
        if let Some(reason) = &buf.read_only {
            let text = format!("Read-only: {reason}");
            return self.notify(Level::Error, text);
        }
        if changes.is_empty() {
            self.notify(Level::Info, format!("{name} already reads as it did {ago}"));
            return;
        }
        self.apply_changes(r.buffer, changes);
        self.follow_edits();
        self.notify(Level::Info, format!("Restored {name} as it was {ago} (not saved)"));
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Sends a history task's result.
pub(super) fn send_done(tx: &tokio::sync::mpsc::UnboundedSender<AppEvent>, result: Result<(), String>) {
    let _ = tx.send(AppEvent::HistoryDone(result));
}
