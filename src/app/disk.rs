//! Open files changed on disk: after the (debounced) file system events naming an open buffer's
//! file, `disk::check` reads it off the UI thread and compares it with what the buffer last knew
//! (`Buffer::disk`). Our own saves' events find just what we wrote; a clean buffer takes in a
//! change at once as an undoable edit (cursors and scroll kept); a dirty one gets the banner
//! (reload / keep mine); a missing file marks the buffer deleted. One check per buffer at a
//! time, none while it's being saved: a request then waits for the running one or the save.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{App, AppEvent, Level};
use crate::buffer::BufferId;
use crate::buffer::pos::ByteIdx;
use crate::disk::{self, Check, OnDisk, Reload};
use crate::layout::GroupId;
use crate::selection::{Selection, Selections};

/// Disk checks in flight and wanted.
#[derive(Default)]
pub struct DiskChecks {
    running: HashSet<BufferId>,
    /// Buffers to check again once their running check or save ends; `true` = forced
    /// ("Reload from Disk").
    again: HashMap<BufferId, bool>,
    /// Checks finished, and reloads made (tests wait on these).
    pub checked: u64,
    pub reloads: u64,
}

impl DiskChecks {
    fn want(&mut self, id: BufferId, force: bool) {
        *self.again.entry(id).or_default() |= force;
    }

    /// A check is running or wanted for `id`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn busy(&self, id: BufferId) -> bool {
        self.running.contains(&id) || self.again.contains_key(&id)
    }
}

/// The result of one check, as `AppEvent::DiskChecked` carries it.
pub(super) struct Checked {
    pub buffer: BufferId,
    /// The buffer's revision and save epoch when the check started.
    pub revision: u64,
    pub epoch: u64,
    pub force: bool,
    pub result: std::io::Result<OnDisk>,
}

impl App {
    /// Changed paths (from `flush_fs_changes`): checks every buffer whose file is one of them or
    /// inside one (a folder deleted or renamed).
    pub(super) fn disk_paths_changed(&mut self, paths: &[PathBuf]) {
        let changed: HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let ids: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.path.ancestors().any(|a| changed.contains(a)))
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            self.check_disk(id, false);
        }
    }

    /// Reads `id`'s file off the UI thread (`disk::check`) → `disk_checked`. Waits for a check
    /// already running or a save in flight. `force`: reload even with unsaved edits.
    pub(super) fn check_disk(&mut self, id: BufferId, force: bool) {
        let Some(buf) = self.buffers.get(id) else { return };
        if buf.save_in_flight() || self.disk.running.contains(&id) {
            self.disk.want(id, force);
            return;
        }
        self.disk.running.insert(id);
        let check = Check {
            path: buf.path.clone(),
            known: buf.disk(),
            base: (force || !buf.has_edits()).then(|| buf.rope.clone()),
            force,
        };
        let (revision, epoch) = (buf.revision(), buf.save_epoch());
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = disk::check(&check);
            let _ = tx.send(AppEvent::DiskChecked(Box::new(Checked {
                buffer: id,
                revision,
                epoch,
                force,
                result,
            })));
        });
    }

    pub(super) fn disk_checked(&mut self, c: Checked) {
        let id = c.buffer;
        self.disk.running.remove(&id);
        self.disk.checked += 1;
        let Some(buf) = self.buffers.get_mut(id) else {
            self.disk.again.remove(&id);
            return;
        };
        let name = buf.name();
        let mut message = None;
        let mut take = None;
        if buf.save_in_flight() || buf.save_epoch() != c.epoch {
            // A save started or ended since the read: it may have read either version. Again
            // once the save is done (`disk_save_done`).
            self.disk.want(id, c.force);
        } else {
            match c.result {
                Err(e) => tracing::warn!(path = %buf.path.display(), "can't check the file on disk: {e}"),
                Ok(OnDisk::Missing) => {
                    if c.force {
                        message = Some((Level::Error, format!("{name} doesn't exist on disk")));
                    }
                    // A new file that was never on disk isn't "deleted".
                    if !buf.deleted && buf.disk().is_some() {
                        tracing::info!(path = %buf.path.display(), "deleted on disk");
                        buf.deleted = true;
                    }
                    buf.disk_conflict = None;
                }
                Ok(OnDisk::Unchanged) => {
                    // As the buffer knew it (it reappeared, or a change was undone outside red).
                    buf.deleted = false;
                    buf.disk_conflict = None;
                }
                Ok(OnDisk::Changed { stamp, reload }) => {
                    buf.deleted = false;
                    if !c.force && buf.has_edits() {
                        if buf.disk_conflict != Some(stamp) {
                            tracing::info!(path = %buf.path.display(), "changed on disk over unsaved edits");
                            buf.disk_conflict = Some(stamp);
                        }
                    } else {
                        match reload.filter(|_| c.revision == buf.revision()) {
                            // Edited since the check began (or it had edits then, so no base):
                            // again, from the current text.
                            None => self.disk.want(id, c.force),
                            Some(Err(e)) => {
                                buf.disk_conflict = Some(stamp);
                                message = Some((Level::Error, format!("{name} changed on disk: {e}")));
                            }
                            Some(Ok(r)) if r.meta.read_only.is_some() && !c.force => {
                                buf.disk_conflict = Some(stamp);
                                let why = r.meta.read_only.unwrap_or_default();
                                let text = format!("{name} changed on disk and is {why}: not reloaded");
                                message = Some((Level::Warning, text));
                            }
                            Some(Ok(r)) => {
                                if let Some(why) = &r.meta.read_only {
                                    message = Some((Level::Warning, format!("{name}: read-only, {why}")));
                                }
                                take = Some(r);
                            }
                        }
                    }
                }
            }
        }
        if let Some(r) = take {
            self.apply_reload(id, r);
        }
        if let Some((level, text)) = message {
            self.notify(level, text);
        }
        if let Some(force) = self.disk.again.remove(&id) {
            self.check_disk(id, force);
        }
    }

    /// A save ended: checks it was waiting for run now.
    pub(super) fn disk_save_done(&mut self, id: BufferId) {
        if !self.disk.running.contains(&id)
            && let Some(force) = self.disk.again.remove(&id)
        {
            self.check_disk(id, force);
        }
    }

    /// Takes the file's text into the buffer (one undo step; it ends clean). Every view on it
    /// keeps its selections on the same text (one inside a changed run keeps its offset into the
    /// new text, `rename::follow`) and its scroll: the active one by line, others by mapping.
    fn apply_reload(&mut self, id: BufferId, r: Reload) {
        let mut follows: Vec<(GroupId, usize, Selections)> = Vec::new();
        let active = (self.groups.active, self.group().active);
        let mut before = None;
        for (gid, group) in self.groups.iter_mut() {
            for (i, tab) in group.text_tabs_mut() {
                if tab.buffer == id {
                    if (gid, i) == active || before.is_none() {
                        before = Some(tab.view.selections.clone());
                    }
                    follows.push((gid, i, crate::rename::follow(&tab.view.selections, &r.changes)));
                }
            }
        }
        let before = before.unwrap_or_else(|| Selections::single(Selection::cursor(ByteIdx(0))));
        let after = crate::rename::follow(&before, &r.changes);
        let Some(buf) = self.buffers.get_mut(id) else { return };
        tracing::info!(path = %buf.path.display(), changes = r.changes.len(), "reloaded from disk");
        buf.reload(r.changes, before, after, r.meta);
        // Not `follow_edits`: it keeps the active view's top line by number, and the file's
        // change didn't come from that view, so its viewport follows the text like the others.
        let changes = buf.take_applied();
        self.completion.follow(id, &changes, buf.sets_applied());
        for (gid, i, sels) in follows {
            if let Some(tab) = self.groups.get_mut(gid).and_then(|g| g.tabs.get_mut(i)?.text_mut()) {
                tab.view.map_through(&changes, &buf.rope);
                tab.view.cursor_history.reset(&sels);
                tab.view.selections = sels;
            }
        }
        self.disk.reloads += 1;
        self.needs_redraw = true;
    }

    /// "File: Reload from Disk" (the banner's Reload): the active file's text from disk, even
    /// over unsaved edits (undo brings them back).
    pub(super) fn reload_from_disk(&mut self) {
        if let Some(id) = self.active_file_buffer() {
            if let Some(buf) = self.buffers.get_mut(id) {
                buf.seal_undo();
            }
            self.check_disk(id, true);
        }
    }

    /// "File: Keep Mine" (the banner's): the next save overwrites the changed file.
    pub(super) fn keep_mine(&mut self) {
        let Some(id) = self.active_file_buffer() else {
            return;
        };
        let Some(buf) = self.buffers.get_mut(id) else { return };
        if !buf.keep_mine() {
            let text = format!("{} hasn't changed on disk", buf.name());
            self.notify(Level::Info, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crossterm::event::{KeyCode, KeyModifiers as M};
    use ropey::Rope;

    use super::*;
    use crate::app::InitialFile;
    use crate::buffer::Buffer;
    use crate::buffer::file::{self, Stamp};
    use crate::workspace::trust::TrustStore;
    use crate::workspace::{self, Workspace};

    fn cmd() -> M {
        if cfg!(target_os = "macos") {
            M::SUPER
        } else {
            M::CONTROL
        }
    }

    /// An app on `a.txt` holding `text`, not started (no watcher): the tests hand it the
    /// checks' results themselves, as a check racing a save or an edit would report them.
    fn setup(text: &str) -> (tempfile::TempDir, PathBuf, App, BufferId) {
        let dir = tempfile::tempdir().unwrap();
        let root = workspace::normalize(dir.path());
        let path = root.join("a.txt");
        fs::write(&path, text).unwrap();
        let buffer = Buffer::open(&path).unwrap();
        let app = App::new(
            Workspace::load(root.clone()),
            TrustStore::load(root.join("trust.toml")),
            Some(InitialFile { buffer, goto: None }),
            true,
        );
        let id = app.group().active_tab().unwrap().buffer;
        (dir, path, app, id)
    }

    fn text(app: &App) -> String {
        app.active_buffer().unwrap().rope.to_string()
    }

    /// A check that began before a save and read the file it wrote must not take our own
    /// bytes for someone else's change over the edits typed since: it's redone.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_check_overlapping_a_save_is_redone() {
        let (_d, _path, mut app, id) = setup("one\n");
        app.type_text("x");
        let buf = app.active_buffer().unwrap();
        let (revision, epoch) = (buf.revision(), buf.save_epoch());
        app.press(KeyCode::Char('s'), cmd());
        assert!(app.settle(|a| !a.active_buffer().unwrap().save_in_flight()).await);
        app.type_text("y");
        app.disk_checked(Checked {
            buffer: id,
            revision,
            epoch,
            force: false,
            result: Ok(OnDisk::Changed {
                stamp: Stamp::of(b"xone\n"),
                reload: None,
            }),
        });
        assert!(app.active_buffer().unwrap().disk_conflict.is_none(), "no banner");
        assert!(app.disk.busy(id), "checked again");
        assert!(app.settle(|a| !a.disk.busy(id)).await);
        let buf = app.active_buffer().unwrap();
        assert!(buf.disk_conflict.is_none() && buf.is_dirty());
        assert_eq!(text(&app), "xyone\n");
    }

    /// "Reload from Disk" over edits typed while its read ran: the diff was made for the old
    /// text, so it's thrown away and the file read again.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reload_of_text_edited_since_the_read_is_redone() {
        let (_d, path, mut app, id) = setup("one\n");
        app.type_text("x");
        let revision = app.active_buffer().unwrap().revision();
        let epoch = app.active_buffer().unwrap().save_epoch();
        fs::write(&path, "two\n").unwrap();
        let loaded = file::load(&path).unwrap();
        let stale = Reload {
            changes: disk::changes(&Rope::from_str("xone\n"), &loaded.rope),
            meta: loaded.meta().unwrap(),
        };
        app.type_text("y");
        app.disk_checked(Checked {
            buffer: id,
            revision,
            epoch,
            force: true,
            result: Ok(OnDisk::Changed {
                stamp: Stamp::of(b"two\n"),
                reload: Some(Ok(stale)),
            }),
        });
        assert_eq!(text(&app), "xyone\n", "not applied to the new text");
        assert!(app.settle(|a| !a.disk.busy(id)).await);
        assert_eq!(text(&app), "two\n", "read again and reloaded (forced: over the edits)");
        assert!(!app.active_buffer().unwrap().is_dirty());
    }

    /// A file that's missing because it was never written (opened as a new file) isn't marked
    /// deleted; once it has been on disk, it is.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_file_never_on_disk_is_not_deleted() {
        let (_d, path, mut app, id) = setup("");
        fs::remove_file(&path).unwrap();
        let buffer = Buffer::open(&path).unwrap();
        app.test_open(buffer);
        let id2 = app.group().active_tab().unwrap().buffer;
        assert_ne!(id, id2);
        app.check_disk(id2, false);
        assert!(app.settle(|a| !a.disk.busy(id2)).await);
        assert!(!app.active_buffer().unwrap().deleted, "never existed");
        fs::write(&path, "new\n").unwrap();
        app.check_disk(id2, false);
        assert!(app.settle(|a| text(a) == "new\n").await, "created outside: reloaded");
        fs::remove_file(&path).unwrap();
        app.check_disk(id2, false);
        assert!(app.settle(|a| a.active_buffer().unwrap().deleted).await, "now deleted");
    }
}
