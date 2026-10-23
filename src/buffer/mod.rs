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

use crate::blame::Blame;
use crate::commit_view::CommitDoc;
use crate::diagnostics::Marks;
use crate::indentation::{Guess, Indentation};
use crate::logs::Tool;
use crate::logs::tab::LogTab;
use crate::quick_diff::QuickDiff;
use crate::selection::Selections;
use crate::syntax::{Language, Syntax};

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
    /// Inline blame's hunks (each line's last commit), mapped through every change set applied
    /// until the next computation (`app/blame.rs`, only while the toggle is on).
    pub blame: Blame,
    /// A tool's log (`Buffer::log`): no file (an empty `path`), read-only, its text only ever
    /// changed by `append` (no undo, never dirty); what it has read of the tool's ring.
    pub log: Option<LogTab>,
    /// A commit tab's text (`Buffer::commit`): no file either, read-only, changed only by
    /// `append`; what the text was rendered from.
    pub commit: Option<CommitDoc>,
    /// What the text said about its indentation when it was read (load, disk reload):
    /// `edit::Settings::for_buffer` applies it unless `detect_indentation` is off.
    guess: Guess,
    /// Indentation chosen for this buffer (by a command), over the language's, the settings'
    /// and the guess; a disk reload drops it (the file is guessed again). Session-only.
    pub indentation: Option<Indentation>,
    /// A tab's drawn width chosen for this buffer ("Change Tab Display Size"), over the
    /// indentation's size; with spaces the indent size stays (`edit::Settings::indent_size`).
    /// Choosing an indentation, re-detecting or a disk reload drops it. Session-only.
    pub tab_display: Option<usize>,
    /// `Some(n)`: an untitled buffer ("Untitled-n"): no path yet, writable, never on disk.
    untitled: Option<u32>,
    /// The language picked for this buffer ("Change Language Mode"), over detection until the
    /// buffer closes. Session-only.
    pub language_override: Option<Language>,
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
    /// Blocking: reads the file and guesses its indentation (`indentation::guess`, `defaults`
    /// being the settings' `edit::Settings::indentation`).
    pub fn open_with(path: &Path, defaults: Indentation) -> Result<Self, LoadError> {
        let loaded = file::load(path)?;
        let guess = crate::indentation::guess(&loaded.rope, defaults);
        let mut buf = Self::from_loaded(loaded, path.to_path_buf());
        buf.guess = guess;
        Ok(buf)
    }

    /// An empty buffer for `path`, a file not on disk yet (`jed -s` on a new file): the first
    /// save creates it.
    pub fn new_file(path: &Path) -> Self {
        let loaded = file::decode(Vec::new()).expect("empty text decodes");
        let mut buf = Self::from_loaded(loaded, path.to_path_buf());
        buf.disk = None;
        buf
    }

    /// `open_with` the default settings' indentation.
    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self, LoadError> {
        Self::open_with(path, crate::edit::Settings::default().indentation())
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
            blame: Blame::default(),
            log: None,
            commit: None,
            guess: Guess::default(),
            indentation: None,
            tab_display: None,
            untitled: None,
            language_override: None,
        }
    }

    /// An empty untitled buffer "Untitled-`n`": no path (an empty `path`, so nothing keyed by a
    /// path takes it: `has_file()` is false), writable, Plain Text until saved or a language is
    /// picked.
    pub fn untitled(n: u32) -> Self {
        let mut buf = Self::without_file("");
        buf.read_only = None;
        buf.untitled = Some(n);
        buf
    }

    /// An empty log buffer for `tool` (`app/tools.rs` fills it with `append`).
    pub fn log(tool: Tool) -> Self {
        let mut buf = Self::without_file("a log can't be edited");
        buf.log = Some(LogTab::new(tool));
        buf
    }

    /// An empty buffer for commit `commit` (`app/commit_tabs.rs` loads it and fills it with
    /// `append`); `file` is the work-tree-relative path whose history opened it.
    pub fn commit(commit: String, file: Option<String>) -> Self {
        let mut buf = Self::without_file("a commit can't be edited");
        buf.commit = Some(CommitDoc::new(commit, file));
        buf
    }

    /// An empty read-only buffer with no path, so nothing keyed by a file (saves, disk checks,
    /// git, language servers, history, backups, sessions) ever takes it: a log's or a commit's.
    fn without_file(why: &str) -> Self {
        Self::from_loaded(
            file::Loaded {
                rope: Rope::new(),
                bom: false,
                eol: Eol::Lf,
                read_only: Some(why.into()),
                stamp: None,
            },
            PathBuf::new(),
        )
    }

    /// Whether the buffer is a file's: it has a path on disk (not a log's, a commit's or an
    /// untitled buffer's, which have none).
    pub fn has_file(&self) -> bool {
        self.log.is_none() && self.commit.is_none() && self.untitled.is_none()
    }

    /// Whether the buffer is untitled (no path yet).
    pub fn is_untitled(&self) -> bool {
        self.untitled.is_some()
    }

    #[cfg(test)]
    pub fn from_str(text: &str) -> Self {
        let loaded = file::decode(text.as_bytes().to_vec()).unwrap();
        Self::from_loaded(loaded, PathBuf::from("/test.txt"))
    }

    pub fn name(&self) -> String {
        if let Some(log) = &self.log {
            return format!("{} (log)", log.tool.name());
        }
        if let Some(commit) = &self.commit {
            return commit.title();
        }
        if let Some(n) = self.untitled {
            return format!("Untitled-{n}");
        }
        self.path
            .file_name()
            .map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    /// Unsaved changes. An untitled buffer has no file to differ from: it is dirty while it has
    /// text (an emptied one closes without asking).
    pub fn is_dirty(&self) -> bool {
        if self.untitled.is_some() {
            return self.rope.len() > 0;
        }
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

    /// The text is based on the file as it was at `stamp` (a crash backup's base, restored over
    /// a file that changed since): the next disk check sees the newer file as a change made
    /// while the buffer had edits (the banner), and a save refuses to overwrite it unasked.
    pub fn set_disk_base(&mut self, stamp: Option<Stamp>) {
        self.disk = stamp;
    }

    /// Takes in the file as it is now on disk: `changes` (from the current text to the file's)
    /// as one undo step of its own (undo brings the text from before back, dirty), then the
    /// buffer is clean at the new text, with the file's BOM, line ending and read-only state, and
    /// `guess` as its indentation (a chosen one is dropped).
    pub fn reload(&mut self, changes: ChangeSet, before: Selections, after: Selections, meta: DiskMeta, guess: Guess) {
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
        self.redetect(guess);
    }

    /// What the text said about its indentation when last read (`indentation::guess`).
    pub fn guess(&self) -> Guess {
        self.guess
    }

    /// Takes a new guess (a reload's, or from the text as it is now: "Detect Indentation from
    /// Content") and drops the chosen indentation and tab display size.
    pub fn redetect(&mut self, guess: Guess) {
        self.guess = guess;
        self.indentation = None;
        self.tab_display = None;
    }

    /// Chooses an indentation ("Indent Using Spaces / Tabs"): its size is the tab width too, so
    /// a tab display size chosen before is dropped (VS Code sets both).
    pub fn choose_indentation(&mut self, indentation: Indentation) {
        self.indentation = Some(indentation);
        self.tab_display = None;
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

    /// Applies `changes` outside the undo history (a log tab's new and dropped lines): no undo
    /// step, never dirty, but other views and the syntax colours follow as for any edit.
    pub fn append(&mut self, changes: ChangeSet) {
        if changes.is_empty() {
            return;
        }
        let changes = Arc::new(changes);
        self.apply(&changes);
        self.applied.push(changes);
        self.revision += 1;
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
        self.blame.map(changes);
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

    /// The lowest `n` ≥ 1 no open untitled buffer is numbered.
    pub fn free_untitled_number(&self) -> u32 {
        let used: std::collections::HashSet<u32> = self.map.values().filter_map(|b| b.untitled).collect();
        (1..).find(|n| !used.contains(n)).unwrap_or(1)
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

    /// `append` changes the text and tells other views (`take_applied`, a new revision) but
    /// leaves no undo step and never makes the buffer dirty; a log buffer is read-only and has
    /// no file.
    #[test]
    fn append_skips_undo_and_dirtiness() {
        let mut buf = Buffer::log(crate::logs::Tool::Jed);
        assert!(buf.read_only.is_some() && buf.path.as_os_str().is_empty());
        assert_eq!(buf.name(), "jed (log)");
        let revision = buf.revision();
        buf.append(insert(0, "a\n"));
        buf.append(insert(2, "b\n"));
        assert_eq!(buf.rope.to_string(), "a\nb\n");
        assert_eq!(buf.revision(), revision + 2);
        assert_eq!(buf.take_applied().len(), 2);
        assert!(!buf.is_dirty() && !buf.has_edits());
        assert!(buf.undo().is_none(), "nothing to undo");
        assert_eq!(buf.rope.to_string(), "a\nb\n");
        buf.append(ChangeSet::default());
        assert_eq!(buf.revision(), revision + 2, "an empty set changes nothing");
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
        buf.reload(insert(4, "two\n"), cursor(0), cursor(0), meta.clone(), Guess::default());
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

    #[test]
    fn untitled_buffers_have_no_file_and_are_dirty_only_with_text() {
        let mut buf = Buffer::untitled(3);
        assert_eq!(buf.name(), "Untitled-3");
        assert!(buf.is_untitled() && !buf.has_file() && buf.read_only.is_none());
        assert!(buf.path.as_os_str().is_empty() && !buf.is_dirty());
        buf.edit(insert(0, "x"), cursor(0), cursor(1), EditKind::Insert);
        assert!(buf.is_dirty());
        buf.undo();
        assert!(!buf.is_dirty(), "emptied again: nothing to lose");
        assert!(Buffer::from_str("a").has_file() && !Buffer::from_str("a").is_untitled());
    }

    #[test]
    fn the_lowest_free_untitled_number_is_reused() {
        let mut buffers = Buffers::default();
        assert_eq!(buffers.free_untitled_number(), 1);
        let one = buffers.insert(Buffer::untitled(1));
        buffers.insert(Buffer::untitled(2));
        buffers.insert(Buffer::from_str("file"));
        assert_eq!(buffers.free_untitled_number(), 3);
        buffers.remove(one);
        assert_eq!(buffers.free_untitled_number(), 1);
    }
}
