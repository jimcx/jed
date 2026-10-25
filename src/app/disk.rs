//! Open files changed on disk: after the (debounced) file system events naming an open buffer's
//! file, `disk::check` reads it off the UI thread and compares it with what the buffer last knew
//! (`Buffer::disk`). Our own saves' events find just what we wrote; a clean buffer takes in a
//! change at once as an undoable edit (cursors and scroll kept); a dirty one gets the banner
//! (reload / keep mine); a missing file marks the buffer deleted. One check per buffer at a
//! time, none while it's being saved: a request then waits for the running one or the save.
//! Hex tabs are checked by path the same way (`hex::check` against their bytes' stamp): changed
//! bytes go to every hex tab of the file at once (`HexTab::set_doc`), a missing file marks them
//! deleted, a file grown past `HEX_MAX_BYTES` keeps the old bytes with a message.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{App, AppEvent, Level};
use crate::buffer::BufferId;
use crate::buffer::pos::ByteIdx;
use crate::disk::{self, Check, OnDisk, Reload};
use crate::hex;
use crate::layout::GroupId;
use crate::selection::{Selection, Selections};

/// Disk checks in flight and wanted.
#[derive(Default)]
pub struct DiskChecks {
    running: HashSet<BufferId>,
    /// Buffers to check again once their running check or save ends; `true` = forced
    /// ("Reload from Disk").
    again: HashMap<BufferId, bool>,
    /// Hex tabs' files being checked, and to check again once that ends.
    hex_running: HashSet<PathBuf>,
    hex_again: HashSet<PathBuf>,
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
            .filter(|(_, b)| b.has_file() && b.path.ancestors().any(|a| changed.contains(a)))
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            self.check_disk(id, false);
        }
        let mut hex: Vec<PathBuf> = Vec::new();
        for h in self.groups.hex_tabs() {
            if h.path.ancestors().any(|a| changed.contains(a)) && !hex.contains(&h.path) {
                hex.push(h.path.clone());
            }
        }
        for path in hex {
            self.check_hex(path);
        }
    }

    /// Reads a hex tab's file off the UI thread (`hex::check`, against the tabs' bytes) →
    /// `AppEvent::HexChecked` → `hex_checked`. Waits for a check of it already running.
    pub(super) fn check_hex(&mut self, path: PathBuf) {
        let mut stamps = self.groups.hex_tabs().filter(|h| h.path == path).map(|h| h.doc.stamp());
        let Some(first) = stamps.next() else { return };
        // Tabs read at different times: any bytes are news to one of them.
        let known = stamps.all(|s| s == first).then_some(first);
        if !self.disk.hex_running.insert(path.clone()) {
            self.disk.hex_again.insert(path);
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = hex::check(&path, known);
            let _ = tx.send(AppEvent::HexChecked { path, result });
        });
    }

    pub(super) fn hex_checked(&mut self, path: PathBuf, result: std::io::Result<hex::OnDisk>) {
        self.disk.hex_running.remove(&path);
        self.disk.checked += 1;
        let gone = self.groups.hex_tabs().any(|h| h.path == path && !h.deleted);
        let mut message = None;
        {
            let tabs = self.groups.hex_tabs_mut().filter(|h| h.path == path);
            match result {
                Err(e) => tracing::warn!(path = %path.display(), "can't check the file on disk: {e}"),
                Ok(hex::OnDisk::Missing) => {
                    if gone {
                        tracing::info!(path = %path.display(), "deleted on disk");
                    }
                    tabs.for_each(|h| h.deleted = true);
                }
                Ok(hex::OnDisk::Unchanged) => tabs.for_each(|h| h.deleted = false),
                Ok(hex::OnDisk::Changed(doc)) => {
                    tracing::info!(path = %path.display(), len = doc.len(), "reloaded from disk (hex)");
                    for h in tabs {
                        if h.doc.stamp() == doc.stamp() {
                            h.deleted = false;
                        } else {
                            h.set_doc(doc.clone());
                        }
                    }
                    self.disk.reloads += 1;
                }
                Ok(hex::OnDisk::TooBig(why)) => {
                    for h in tabs {
                        h.deleted = false;
                        message = Some(format!("{} changed on disk and is {why}: not reloaded", h.name()));
                    }
                }
            }
        }
        if let Some(text) = message {
            self.notify(Level::Warning, text);
        }
        self.needs_redraw = true;
        if self.disk.hex_again.remove(&path) {
            self.check_hex(path);
        }
    }

    /// Reads `id`'s file off the UI thread (`disk::check`) → `disk_checked`. Waits for a check
    /// already running or a save in flight. `force`: reload even with unsaved edits.
    pub(super) fn check_disk(&mut self, id: BufferId, force: bool) {
        let Some(buf) = self.buffers.get(id).filter(|b| b.has_file()) else {
            return;
        };
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
            defaults: self.settings.indentation(),
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
                    // As the buffer knew it (it reappeared, or a change was undone outside jed).
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
        buf.reload(r.changes, before, after, r.meta, r.guess);
        // Not `follow_edits`: it keeps the active view's top line by number, and the file's
        // change didn't come from that view, so its viewport follows the text like the others.
        let changes = buf.take_applied();
        self.completion.follow(id, &changes, buf.sets_applied());
        self.nav.history.map(&buf.path, &changes);
        for (gid, i, sels) in follows {
            if let Some(tab) = self.groups.get_mut(gid).and_then(|g| g.tabs.get_mut(i)?.text_mut()) {
                tab.view.map_through(&changes, &buf.rope, buf.revision());
                // The new text's lines are measured afresh as they're drawn.
                tab.view.scroll_width.reset();
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
            guess: Default::default(),
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

    /// An untitled buffer has no file: a check of it (from any caller) starts nothing, so a
    /// forced one can't report "Untitled-1 doesn't exist on disk".
    #[tokio::test(flavor = "multi_thread")]
    async fn an_untitled_buffer_is_never_checked() {
        let (_d, _path, mut app, id) = setup("one\n");
        app.test_open(Buffer::untitled(1));
        let u = app.group().active_tab().unwrap().buffer;
        assert_ne!(id, u);
        app.check_disk(u, true);
        assert!(!app.disk.busy(u), "nothing started");
        app.check_disk(id, true);
        assert!(app.disk.busy(id), "a file's buffer is checked");
        assert!(app.settle(|a| !a.disk.busy(id)).await);
        assert_eq!(app.disk.checked, 1, "only a.txt's check ran");
        assert!(app.message.is_none(), "{:?}", app.message.as_ref().map(|m| &m.text));
    }

    /// A hex tab of `a.bin` holding `bytes` (read from disk), the app not started (no watcher).
    fn hex_setup(bytes: &[u8]) -> (tempfile::TempDir, PathBuf, App) {
        let dir = tempfile::tempdir().unwrap();
        let root = workspace::normalize(dir.path());
        let path = root.join("a.bin");
        fs::write(&path, bytes).unwrap();
        let mut app = App::new(
            Workspace::load(root.clone()),
            TrustStore::load(root.join("trust.toml")),
            None,
            true,
        );
        app.open_initial_hex(path.clone(), hex::read(&path).unwrap().unwrap());
        (dir, path, app)
    }

    /// A change arriving while a hex check runs is checked again once it ends (not dropped,
    /// not run beside it); the tab ends with the last bytes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hex_check_requested_while_one_runs_runs_after_it() {
        let (_d, path, mut app) = hex_setup(b"\0one");
        fs::write(&path, b"\0two").unwrap();
        app.check_hex(path.clone());
        app.check_hex(path.clone());
        assert!(app.disk.hex_running.contains(&path) && app.disk.hex_again.contains(&path));
        assert!(app.settle(|a| a.disk.checked == 2).await, "checked twice");
        assert!(app.settle(|a| !a.disk.hex_running.contains(&path)).await);
        assert!(app.disk.hex_again.is_empty());
        assert_eq!(app.disk.checked, 2);
        assert_eq!(app.active_hex().unwrap().doc.bytes(), b"\0two");
        assert_eq!(app.disk.reloads, 1, "the second check found the bytes it has");
    }

    /// A hex tab closed while its check runs: the result finds no tab and is dropped, and the
    /// file can be checked again later (nothing left running).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_hex_check_for_a_closed_tab_is_dropped() {
        let (_d, path, mut app) = hex_setup(b"\0one");
        fs::write(&path, b"\0two").unwrap();
        app.check_hex(path.clone());
        app.check_hex(path.clone());
        app.group_mut().tabs.clear();
        assert!(app.settle(|a| a.disk.checked == 1).await);
        assert!(app.disk.hex_running.is_empty() && app.disk.hex_again.is_empty());
        assert!(app.groups.hex_tabs().next().is_none());
    }

    /// A change to a file shown in two hex tabs (a split) and to its folder starts one check of
    /// it, not one per tab or path; a file no hex tab shows starts none.
    #[tokio::test(flavor = "multi_thread")]
    async fn one_hex_check_per_file_for_all_its_tabs() {
        let (_d, path, mut app) = hex_setup(b"\0one");
        app.test_execute(crate::command::Command::SplitRight);
        assert_eq!(app.groups.hex_tabs().count(), 2);
        let dir = path.parent().unwrap().to_path_buf();
        app.disk_paths_changed(&[dir.join("other.bin")]);
        assert!(app.disk.hex_running.is_empty(), "not its file");
        app.disk_paths_changed(&[path.clone(), dir]);
        assert!(app.disk.hex_running.contains(&path));
        assert!(app.disk.hex_again.is_empty(), "one check");
        assert!(app.settle(|a| a.disk.hex_running.is_empty()).await);
        assert_eq!(app.disk.checked, 1);
    }
}
