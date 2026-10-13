//! A text buffer: rope, undo history, file state.

pub mod change;
pub mod file;
pub mod pos;
pub mod undo;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ropey::Rope;

use change::ChangeSet;
use file::{Eol, LoadError};
use undo::{EditKind, History};

use crate::selection::Selections;

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
        let inverse = changes.apply(&mut self.rope);
        self.history
            .record(changes, inverse, before, after, kind, Instant::now());
    }

    /// Called for any command that isn't an edit, so the next edit starts a new undo step.
    pub fn seal_undo(&mut self) {
        self.history.seal();
    }

    pub fn undo(&mut self) -> Option<Selections> {
        self.history.undo(&mut self.rope)
    }

    pub fn redo(&mut self) -> Option<Selections> {
        self.history.redo(&mut self.rope)
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
