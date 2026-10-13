//! A text buffer: rope, undo history, file state.

pub mod change;
pub mod file;
pub mod pos;
pub mod undo;

use std::path::{Path, PathBuf};
use std::time::Instant;

use ropey::Rope;

use change::ChangeSet;
use file::{Eol, LoadError};
use undo::{EditKind, History};

use crate::selection::Selections;

#[derive(Debug)]
pub struct Buffer {
    pub rope: Rope,
    pub path: PathBuf,
    pub bom: bool,
    pub eol: Eol,
    pub read_only: Option<String>,
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
    pub fn open(path: &Path) -> Result<Self, LoadError> {
        let loaded = file::load(path)?;
        Ok(Self {
            rope: loaded.rope,
            path: path.to_path_buf(),
            bom: loaded.bom,
            eol: loaded.eol,
            read_only: loaded.read_only,
            history: History::default(),
            saved_state: 0,
            saving_state: None,
        })
    }

    #[cfg(test)]
    pub fn from_str(text: &str) -> Self {
        let loaded = file::decode(text.as_bytes().to_vec()).unwrap();
        Self {
            rope: loaded.rope,
            path: PathBuf::from("test.txt"),
            bom: loaded.bom,
            eol: loaded.eol,
            read_only: None,
            history: History::default(),
            saved_state: 0,
            saving_state: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.history.state_id() != self.saved_state
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
        }
    }
}

impl SaveJob {
    /// Blocking: call from `spawn_blocking`.
    pub fn run(&self) -> std::io::Result<()> {
        file::atomic_write(&self.path, &file::encode(&self.rope, self.bom))
    }
}
