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
use file::{Eol, LoadError};
use undo::{EditKind, History};

use crate::diagnostics::Marks;
use crate::selection::Selections;
use crate::syntax::Syntax;

pub type BufferId = u64;

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
    /// Language and parse tree; every change set applied here reaches `Syntax::edit`.
    pub syntax: Syntax,
    /// While a language server has this document open (`track_changes`): every change set
    /// applied since the last `take_tracked`, with the text from before it.
    tracked: Option<Vec<Tracked>>,
    /// Language servers' diagnostics in this text, mapped through every change set applied
    /// until a server publishes again (`lsp::Manager::publish`).
    pub diagnostics: Marks,
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
            history: History::default(),
            saved_state: 0,
            saving_state: None,
            applied: Vec::new(),
            revision: 0,
            syntax: Syntax::default(),
            tracked: None,
            diagnostics: Marks::default(),
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
        self.deleted || self.history.state_id() != self.saved_state
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

    /// The one place change sets are applied: the syntax tree and the diagnostics follow each one,
    /// and a language server's document is told of each (`tracked`).
    fn apply(&mut self, changes: &Arc<ChangeSet>) -> ChangeSet {
        let old = self.syntax.wants_edits().then(|| self.rope.clone());
        if let Some(tracked) = &mut self.tracked {
            tracked.push(Tracked {
                before: old.clone().unwrap_or_else(|| self.rope.clone()),
                changes: changes.clone(),
            });
        }
        let inverse = changes.apply(&mut self.rope);
        self.syntax.map_highlights(changes);
        self.diagnostics.map(changes);
        if let Some(old) = old {
            self.syntax.edit(changes, &old, &self.rope);
        }
        inverse
    }

    /// Changes with every edit, undo and redo: what cached search results are keyed on.
    pub fn revision(&self) -> u64 {
        self.revision
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
        SaveJob {
            state,
            path: self.path.clone(),
            rope: self.rope.clone(),
            bom: self.bom,
        }
    }

    pub fn finish_save(&mut self, state: u64, ok: bool) {
        self.saving_state = None;
        if ok {
            self.saved_state = state;
            self.deleted = false;
        }
    }
}

impl SaveJob {
    /// Blocking: call from `spawn_blocking`.
    pub fn run(&self) -> std::io::Result<()> {
        file::atomic_write(&self.path, &file::encode(&self.rope, self.bom))
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
