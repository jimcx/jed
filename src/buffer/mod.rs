//! A text buffer: rope, undo history, file state.

pub mod change;
pub mod file;
pub mod pos;
pub mod undo;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ropey::Rope;

use change::ChangeSet;
use file::{DiskMeta, Eol, LoadError, Stamp};
use undo::{EditKind, History};

use crate::diagnostics::Marks;
use crate::quick_diff::QuickDiff;
use crate::selection::Selections;
use crate::syntax::Syntax;

pub type BufferId = u64;

/// A `saved_state` no history state has: the text differs from the file whatever is undone
/// ("keep mine" kept edits over a newer file).
const NEVER_SAVED: u64 = u64::MAX;

#[derive(Debug)]
pub struct Buffer {
    pub rope: Rope,
    /// Absolute.
    pub path: PathBuf,
    pub bom: bool,
    pub eol: Eol,
    pub read_only: Option<String>,
    /// The file was deleted on disk; saving recreates it.
    pub deleted: bool,
    /// The file's bytes as this buffer last knew them on disk (loaded, saved, reloaded, or
    /// accepted by "keep mine"); `None`: there was no file. A save checks the file still holds
    /// them, and a file system event that finds them is no change (our own save's events).
    disk: Option<Stamp>,
    /// The file changed on disk while the buffer had unsaved edits: the version the banner
    /// (reload / keep mine) is about.
    pub disk_conflict: Option<Stamp>,
    /// Bumped when a save starts and when it ends: a disk check begun before is stale.
    save_epoch: u64,
    history: History,
    /// History state written by the last successful save.
    saved_state: u64,
    /// History state currently being written, if a save is in flight.
    saving_state: Option<u64>,
    /// Changes applied since the last `take_applied`, for other views on this buffer to follow
    /// (shared with the undo history: an edit at 60k selections isn't copied).
    applied: Vec<Arc<ChangeSet>>,
    /// Counts text changes (edits, undos, redos): equal revisions mean the same text.
    revision: u64,
    /// Counts change sets applied (an undo step can apply several in one revision).
    sets_applied: u64,
    /// Language and parse tree; every change set applied here reaches `Syntax::edit`.
    pub syntax: Syntax,
    /// While a language server has this document open (`track_changes`): every change set
    /// applied since the last `take_tracked`, with the text from before it.
    tracked: Option<Vec<Tracked>>,
    /// Language servers' diagnostics in this text, mapped through every change set applied
    /// until a server publishes again (`lsp::Manager::publish`).
    pub diagnostics: Marks,
    /// The gutter's git markers (lines changed against the index), mapped through every change
    /// set applied until the next computation (`app/quick_diff.rs`).
    pub quick_diff: QuickDiff,
}

/// A change set as a language server needs it: positions are in `before`'s coordinates.
#[derive(Debug, Clone)]
pub struct Tracked {
    /// The text the changes apply to (a cheap clone).
    pub before: Rope,
    pub changes: Arc<ChangeSet>,
}

/// What a background save needs; the rope clone is cheap.
pub struct SaveJob {
    pub state: u64,
    pub path: PathBuf,
    pub rope: Rope,
    pub bom: bool,
    /// What the file must still hold for the save to go ahead (`None`: no file); anything
    /// else (but what we're writing) is a `SaveError::Conflict`.
    pub expected: Option<Stamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The file changed on disk since the buffer last knew it: nothing written.
    Conflict,
    Failed(String),
}

impl Buffer {
    /// Blocking: reads the file.
    pub fn open(path: &Path) -> Result<Self, LoadError> {
        let loaded = file::load(path)?;
        Ok(Self::from_loaded(loaded, path.to_path_buf()))
    }

    fn from_loaded(loaded: file::Loaded, path: PathBuf) -> Self {
        Self {
            rope: loaded.rope,
            path,
            bom: loaded.bom,
            eol: loaded.eol,
            read_only: loaded.read_only,
            deleted: false,
            disk: loaded.stamp,
            disk_conflict: None,
            save_epoch: 0,
            history: History::default(),
            saved_state: 0,
            saving_state: None,
            applied: Vec::new(),
            revision: 0,
            sets_applied: 0,
            syntax: Syntax::default(),
            tracked: None,
            diagnostics: Marks::default(),
            quick_diff: QuickDiff::default(),
        }
    }

    #[cfg(test)]
    pub fn from_str(text: &str) -> Self {
        let loaded = file::decode(text.as_bytes().to_vec()).unwrap();
        Self::from_loaded(loaded, PathBuf::from("/test.txt"))
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    pub fn is_dirty(&self) -> bool {
        self.deleted || self.has_edits()
    }

    /// The text differs from what was last saved or loaded (unlike `is_dirty`, a deleted file
    /// alone isn't): a file changed on disk is reloaded silently only without these.
    pub fn has_edits(&self) -> bool {
        self.history.state_id() != self.saved_state
    }

    /// The file's bytes as the buffer last knew them on disk; `None`: no file.
    pub fn disk(&self) -> Option<Stamp> {
        self.disk
    }

    /// Changes when a save starts and ends.
    pub fn save_epoch(&self) -> u64 {
        self.save_epoch
    }

    /// "Keep mine": the file's newer version (`disk_conflict`) is accepted as known, so the next
    /// save overwrites it without a conflict, and the buffer stays dirty until then (even if its
    /// edits are undone: the text isn't the file's). False when there was no conflict.
    pub fn keep_mine(&mut self) -> bool {
        let Some(stamp) = self.disk_conflict.take() else {
            return false;
        };
        self.disk = Some(stamp);
        self.saved_state = NEVER_SAVED;
        true
    }

    /// Takes in the file as it is now on disk: `changes` (from the current text to the file's)
    /// as one undo step of its own (undo brings the text from before back, dirty), then the
    /// buffer is clean at the new text, with the file's BOM, line ending and read-only state.
    pub fn reload(&mut self, changes: ChangeSet, before: Selections, after: Selections, meta: DiskMeta) {
        self.history.seal();
        self.edit(changes, before, after, EditKind::Other);
        self.history.seal();
        self.saved_state = self.history.state_id();
        self.disk = Some(meta.stamp);
        self.disk_conflict = None;
        self.deleted = false;
        self.bom = meta.bom;
        self.eol = meta.eol;
        self.read_only = meta.read_only;
    }

    pub fn save_in_flight(&self) -> bool {
        self.saving_state.is_some()
    }

    /// Applies an edit and records it for undo.
    pub fn edit(&mut self, changes: ChangeSet, before: Selections, after: Selections, kind: EditKind) {
        if changes.is_empty() {
            return;
        }
        let changes = Arc::new(changes);
        let inverse = Arc::new(self.apply(&changes));
        self.applied.push(changes.clone());
        self.revision += 1;
        self.history
            .record(changes, inverse, before, after, kind, Instant::now());
    }

    /// Called for any command that isn't an edit, so the next edit starts a new undo step.
    pub fn seal_undo(&mut self) {
        self.history.seal();
    }

    pub fn undo(&mut self) -> Option<Selections> {
        let (sel, sets) = self.history.undo()?;
        self.apply_all(sets);
        Some(sel)
    }

    pub fn redo(&mut self) -> Option<Selections> {
        let (sel, sets) = self.history.redo()?;
        self.apply_all(sets);
        Some(sel)
    }

    /// Applies an undo / redo step's change sets in order.
    fn apply_all(&mut self, sets: Vec<Arc<ChangeSet>>) {
        for set in &sets {
            self.apply(set);
        }
        self.applied.extend(sets);
        self.revision += 1;
    }

    /// The one place change sets are applied: the syntax tree, the diagnostics and the git
    /// markers follow each one, and a language server's document is told of each (`tracked`).
    fn apply(&mut self, changes: &Arc<ChangeSet>) -> ChangeSet {
        let old = self.syntax.wants_edits().then(|| self.rope.clone());
        if let Some(tracked) = &mut self.tracked {
            tracked.push(Tracked {
                before: old.clone().unwrap_or_else(|| self.rope.clone()),
                changes: changes.clone(),
            });
        }
        let inverse = changes.apply(&mut self.rope);
        self.sets_applied += 1;
        self.syntax.map_highlights(changes);
        self.diagnostics.map(changes);
        self.quick_diff.map(changes);
        if let Some(old) = old {
            self.syntax.edit(changes, &old, &self.rope);
        }
        inverse
    }

    /// Changes with every edit, undo and redo: what cached search results are keyed on.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// How many change sets were applied so far: the ones `take_applied` returns are the last of
    /// them (completion anchors skip those their text already has).
    pub fn sets_applied(&self) -> u64 {
        self.sets_applied
    }

    /// Changes applied since the last call, in order.
    pub fn take_applied(&mut self) -> Vec<Arc<ChangeSet>> {
        std::mem::take(&mut self.applied)
    }

    /// Starts (`true`, from an empty list) or stops recording applied change sets for
    /// `take_tracked`: on while a language server has the document open.
    pub fn track_changes(&mut self, on: bool) {
        self.tracked = on.then(Vec::new);
    }

    /// Change sets applied since tracking started or the last call, in order, each with the text
    /// from before it.
    pub fn take_tracked(&mut self) -> Vec<Tracked> {
        self.tracked.as_mut().map(std::mem::take).unwrap_or_default()
    }

    pub fn begin_save(&mut self) -> SaveJob {
        self.history.seal();
        let state = self.history.state_id();
        self.saving_state = Some(state);
        self.save_epoch += 1;
        SaveJob {
            state,
            path: self.path.clone(),
            rope: self.rope.clone(),
            bom: self.bom,
            expected: self.disk,
        }
    }

    /// A save ended: `written` holds what it wrote (`None`: it failed or was refused).
    pub fn finish_save(&mut self, state: u64, written: Option<Stamp>) {
        self.saving_state = None;
        self.save_epoch += 1;
        if let Some(stamp) = written {
            self.saved_state = state;
            self.deleted = false;
            self.disk = Some(stamp);
            self.disk_conflict = None;
        }
    }
}

impl SaveJob {
    /// Blocking: call from `spawn_blocking`. Refuses (`Conflict`, nothing written) when the
    /// file no longer holds what the buffer last knew of it, unless it already holds what we'd
    /// write; a missing file is (re)created. Returns the stamp of the bytes written, and the
    /// bytes (file history snapshots them).
    pub fn write(&self) -> Result<(Stamp, Vec<u8>), SaveError> {
        let bytes = file::encode(&self.rope, self.bom);
        let stamp = Stamp::of(&bytes);
        // An unreadable file can't be compared: written as before.
        if let Ok(Some(current)) = file::read(&self.path) {
            let current = Stamp::of(&current);
            if Some(current) != self.expected && current != stamp {
                return Err(SaveError::Conflict);
            }
        }
        file::atomic_write(&self.path, &bytes).map_err(|e| SaveError::Failed(e.to_string()))?;
        Ok((stamp, bytes))
    }
}

/// All open buffers. Tabs refer to them by id; several tabs may share one.
#[derive(Default)]
pub struct Buffers {
    map: HashMap<BufferId, Buffer>,
    next: BufferId,
}

impl Buffers {
    pub fn insert(&mut self, buffer: Buffer) -> BufferId {
        let id = self.next;
        self.next += 1;
        self.map.insert(id, buffer);
        id
    }

    pub fn get(&self, id: BufferId) -> Option<&Buffer> {
        self.map.get(&id)
    }

    pub fn get_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.map.get_mut(&id)
    }

    pub fn remove(&mut self, id: BufferId) -> Option<Buffer> {
        self.map.remove(&id)
    }

    pub fn find_path(&self, path: &Path) -> Option<BufferId> {
        self.map.iter().find(|(_, b)| b.path == path).map(|(id, _)| *id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (BufferId, &Buffer)> {
        self.map.iter().map(|(id, b)| (*id, b))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (BufferId, &mut Buffer)> {
        self.map.iter_mut().map(|(id, b)| (*id, b))
    }

    pub fn dirty(&self) -> Vec<BufferId> {
        let mut ids: Vec<BufferId> = self.iter().filter(|(_, b)| b.is_dirty()).map(|(id, _)| id).collect();
        ids.sort_unstable();
        ids
    }

    pub fn any_saving(&self) -> bool {
        self.map.values().any(Buffer::save_in_flight)
    }
}

#[cfg(test)]
mod tests {
    use super::change::Change;
    use super::pos::ByteIdx;
    use super::*;

    fn insert(at: usize, text: &str) -> ChangeSet {
        ChangeSet::new(vec![Change {
            start: ByteIdx(at),
            end: ByteIdx(at),
            text: text.into(),
        }])
    }

    fn cursor(at: usize) -> Selections {
        Selections::single(crate::selection::Selection::cursor(ByteIdx(at)))
    }

    /// A save refuses to overwrite a file changed since it was loaded; "keep mine" lets the
    /// next one through, and the buffer stays dirty until then.
    #[test]
    fn save_conflicts_and_keep_mine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "one\n").unwrap();
        let mut buf = Buffer::open(&path).unwrap();
        buf.edit(insert(0, "x"), cursor(0), cursor(1), EditKind::Insert);
        std::fs::write(&path, "two\n").unwrap();
        let job = buf.begin_save();
        assert_eq!(job.write(), Err(SaveError::Conflict));
        buf.finish_save(job.state, None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n", "nothing written");

        assert!(!buf.keep_mine(), "no conflict known yet");
        buf.disk_conflict = Some(Stamp::of(b"two\n"));
        assert!(buf.keep_mine());
        assert!(buf.disk_conflict.is_none());
        buf.undo();
        assert_eq!(buf.rope.to_string(), "one\n");
        assert!(buf.is_dirty(), "the text isn't the file's");
        let job = buf.begin_save();
        let (written, _) = job.write().unwrap();
        buf.finish_save(job.state, Some(written));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        assert!(!buf.is_dirty());
        assert_eq!(buf.disk(), Some(Stamp::of(b"one\n")));

        // A file already holding what we'd write is no conflict; a missing one is recreated.
        std::fs::write(&path, "xone\n").unwrap();
        buf.redo();
        let job = buf.begin_save();
        assert!(job.write().is_ok());
        std::fs::remove_file(&path).unwrap();
        assert!(job.write().is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "xone\n");
    }

    /// A reload is one undo step and leaves the buffer clean; undo makes it dirty again.
    #[test]
    fn reload_is_undoable_and_clean() {
        let mut buf = Buffer::from_str("one\n");
        buf.deleted = true;
        let meta = DiskMeta {
            stamp: Stamp::of(b"\xEF\xBB\xBFone\ntwo\n"),
            bom: true,
            eol: Eol::Lf,
            read_only: None,
        };
        buf.reload(insert(4, "two\n"), cursor(0), cursor(0), meta.clone());
        assert_eq!(buf.rope.to_string(), "one\ntwo\n");
        assert!(!buf.is_dirty() && buf.bom && !buf.deleted);
        assert_eq!(buf.disk(), Some(meta.stamp));
        buf.undo();
        assert_eq!(buf.rope.to_string(), "one\n");
        assert!(buf.is_dirty());
        buf.redo();
        assert!(!buf.is_dirty());
    }

    /// While tracking, edits, undo and redo each leave a change set with the text from before it.
    #[test]
    fn tracked_changes_carry_the_text_before_them() {
        let mut buf = Buffer::from_str("ab");
        let sel = Selections::single(crate::selection::Selection::cursor(ByteIdx(0)));
        buf.edit(insert(0, "x"), sel.clone(), sel.clone(), EditKind::Other);
        assert!(buf.take_tracked().is_empty(), "off by default");
        buf.track_changes(true);
        buf.edit(insert(1, "y"), sel.clone(), sel.clone(), EditKind::Other);
        buf.undo();
        buf.redo();
        let tracked = buf.take_tracked();
        let befores: Vec<String> = tracked.iter().map(|t| t.before.to_string()).collect();
        assert_eq!(befores, ["xab", "xyab", "xab"]);
        assert!(buf.take_tracked().is_empty(), "taken");
        buf.track_changes(false);
        buf.undo();
        assert!(buf.take_tracked().is_empty());
    }
}
