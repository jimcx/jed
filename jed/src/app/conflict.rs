//! Merge conflicts in the app: Next / Previous Conflict (the cursor to a conflict's `<<<<<<<`
//! line, wrapping around) and Compare Current Conflict (a read-only diff tab of the conflict's
//! current and incoming blocks). Accepting a side is `edit::merge_command`; the editor draws the
//! blocks and the action row (`ui::editor`); the model is `crate::conflict`.
//!
//! Resolve Conflicts Using Ours / Theirs (`git.resolveOurs` / `git.resolveTheirs`, also the
//! right-click menu of a Source Control "Merge Changes" row) take one side of a conflicted file
//! whole, off-thread (`stage::resolve_file`): written on disk and staged as resolved. The text
//! before is kept in the file history first; a file with unsaved changes or a save in flight is
//! refused; an open buffer then reloads from disk. Ends in `AppEvent::Diff(DiffEvent::Resolved)`.

use std::path::PathBuf;

use super::{App, AppEvent, Level};
use crate::buffer::pos::ByteIdx;
use crate::conflict::{self, Choice};
use crate::git::stage::{self, ConflictSide};
use crate::scm::{Row, Section};
use crate::selection::{Selection, Selections};

impl App {
    /// `Merge Conflict: Next / Previous Conflict` (Alt+F8 / Shift+Alt+F8): the primary cursor to
    /// the next / previous conflict's start, wrapping; nothing without conflicts or on a tab
    /// that isn't text.
    pub(super) fn go_to_conflict(&mut self, forward: bool) {
        let Some((buf, view)) = self.active_mut() else { return };
        let head = view.selections.primary().head;
        let revision = buf.revision();
        let target = if forward {
            buf.conflicts.next(&buf.rope, revision, head)
        } else {
            buf.conflicts.previous(&buf.rope, revision, head)
        }
        .map(|c| c.start);
        let Some(at) = target else { return };
        buf.seal_undo();
        view.selections = Selections::single(Selection::cursor(at));
        view.reveal_range = Some((at, at));
    }

    /// `Merge Conflict: Compare Current Conflict`: a read-only diff tab of the blocks of the
    /// conflict at the primary cursor, else (from an action-row button) the one at `at`. Nothing
    /// when neither is inside a conflict.
    pub(super) fn compare_conflict(&mut self, at: Option<ByteIdx>) {
        let Some((path, current, incoming)) = self.conflict_blocks(at) else {
            return;
        };
        self.open_conflict_diff(path, current, incoming);
    }

    /// The file's name (the tab's title) and the current / incoming block bytes of the conflict
    /// at `at` (else at the cursor).
    fn conflict_blocks(&mut self, at: Option<ByteIdx>) -> Option<(PathBuf, Vec<u8>, Vec<u8>)> {
        let (buf, view) = self.active_mut()?;
        let head = view.selections.primary().head;
        let revision = buf.revision();
        let scanned = buf.conflicts.scan(&buf.rope, revision);
        let start = at.or_else(|| {
            scanned
                .iter()
                .find(|c| c.start <= head && head < c.end)
                .map(|c| c.start)
        })?;
        let conflict = scanned.iter().find(|c| c.start == start)?;
        let current = conflict::block(&buf.rope, conflict, Choice::Current).into_bytes();
        let incoming = conflict::block(&buf.rope, conflict, Choice::Incoming).into_bytes();
        // A log or untitled buffer has no path: the tab's title uses its name.
        let path = if buf.has_file() {
            buf.path.clone()
        } else {
            PathBuf::from(buf.name())
        };
        Some((path, current, incoming))
    }
}

impl App {
    /// `Git: Resolve Conflicts Using Ours / Theirs`: with the Source Control view focused its
    /// selected file, else the active file.
    pub(super) fn resolve_command(&mut self, side: ConflictSide) {
        self.ensure_status_loaded();
        let path = if self.scm_focused() {
            let Some(status) = self.git.status.as_ref() else {
                return self.notify(Level::Info, "Not in a git repository".into());
            };
            let rows = self.scm.rows(status);
            match self.scm.selected(&rows) {
                Some(Row::File { entry, .. }) => Some(status.repo.workdir.join(&entry.path)),
                _ => None,
            }
        } else {
            self.active_path()
        };
        if let Some(path) = path {
            self.resolve_conflict(path, side);
        }
    }

    /// Resolves `path` (absolute) by taking `side` whole, off-thread.
    pub(super) fn resolve_conflict(&mut self, path: PathBuf, side: ConflictSide) {
        let Some(status) = self.git.status.as_ref() else {
            return self.notify(Level::Info, "Not in a git repository".into());
        };
        let repo = status.repo.clone();
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let Some(rel) = repo.relative(&path) else {
            return self.notify(Level::Info, format!("{name} isn't in the repository"));
        };
        let conflicted = status
            .entry(&rel)
            .is_some_and(|e| Section::MergeChanges.change(e).is_some());
        if !conflicted {
            return self.notify(Level::Info, format!("{name} has no merge conflicts"));
        }
        let buffer = self.buffer_of(&path).and_then(|b| self.buffers.get(b));
        if buffer.is_some_and(|b| b.save_in_flight()) {
            return self.notify(Level::Info, format!("Can't resolve {name}: it is being saved"));
        }
        if buffer.is_some_and(|b| b.is_dirty()) {
            return self.notify(Level::Info, format!("Can't resolve {name}: it has unsaved changes"));
        }
        let job = self.history_job(&path);
        if job.is_some() {
            self.history_started();
        }
        self.diffs.staging += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = stage::resolve_file(&repo, &rel, side);
            if let Some(job) = job {
                let recorded = match &result {
                    Ok(Some(before)) => job.record(before),
                    _ => Ok(()),
                };
                super::history::send_done(&tx, recorded);
            }
            let theirs = side == ConflictSide::Theirs;
            let message = match result {
                Ok(_) => Ok(format!(
                    "Resolved {name} using {}",
                    if theirs { "theirs" } else { "ours" }
                )),
                Err(e) => Err(format!("Can't resolve {name}: {e}")),
            };
            let _ = tx.send(AppEvent::Diff(super::diff_view::DiffEvent::Resolved { path, message }));
        });
    }

    /// `AppEvent::Diff(DiffEvent::Resolved)`: an open buffer reloads from disk (one undo step),
    /// the result is said and the status read again.
    pub(super) fn resolve_done(&mut self, path: PathBuf, message: Result<String, String>) {
        self.diffs.staging -= 1;
        match message {
            Ok(m) => {
                if let Some(id) = self.buffer_of(&path) {
                    if let Some(buf) = self.buffers.get_mut(id) {
                        buf.seal_undo();
                    }
                    self.check_disk(id, true);
                }
                self.notify(Level::Info, m);
            }
            Err(e) => self.notify(Level::Error, e),
        }
        self.quick_diff_index_changed();
        self.refresh_git(None);
    }
}
