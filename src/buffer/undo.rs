//! Undo history with VS Code-style grouping.
//!
//! Consecutive typing is one step. A new step starts when the cursor moves,
//! after a pause, or when switching between inserting and deleting.
//! Anything else (paste, cut, multi-cursor commands) is a step of its own.

use std::time::{Duration, Instant};

use ropey::Rope;

use super::change::ChangeSet;
use crate::selection::Selections;

const GROUP_PAUSE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    Delete,
    /// Never grouped with neighbouring edits.
    Other,
}

#[derive(Debug)]
struct Step {
    id: u64,
    /// (forward, inverse) pairs, in the order they were applied.
    edits: Vec<(ChangeSet, ChangeSet)>,
    sel_before: Selections,
    sel_after: Selections,
    kind: EditKind,
    last_edit: Instant,
    sealed: bool,
}

#[derive(Debug)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    next_id: u64,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
        }
    }
}

impl History {
    /// Identifies the current text state; 0 is the state the buffer was loaded in.
    pub fn state_id(&self) -> u64 {
        self.undo.last().map_or(0, |s| s.id)
    }

    /// Stops further edits from joining the current step.
    pub fn seal(&mut self) {
        if let Some(step) = self.undo.last_mut() {
            step.sealed = true;
        }
    }

    pub fn record(
        &mut self,
        forward: ChangeSet,
        inverse: ChangeSet,
        sel_before: Selections,
        sel_after: Selections,
        kind: EditKind,
        now: Instant,
    ) {
        self.redo.clear();
        if let Some(last) = self.undo.last_mut() {
            let joins = !last.sealed
                && kind != EditKind::Other
                && last.kind == kind
                && now.duration_since(last.last_edit) < GROUP_PAUSE
                && last.sel_after == sel_before;
            if joins {
                last.edits.push((forward, inverse));
                last.sel_after = sel_after;
                last.last_edit = now;
                return;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.undo.push(Step {
            id,
            edits: vec![(forward, inverse)],
            sel_before,
            sel_after,
            kind,
            last_edit: now,
            sealed: false,
        });
    }

    /// Reverts the last step; returns the selections from before it and the changes applied.
    pub fn undo(&mut self, rope: &mut Rope) -> Option<(Selections, Vec<ChangeSet>)> {
        let mut step = self.undo.pop()?;
        let mut applied = Vec::with_capacity(step.edits.len());
        for (_, inverse) in step.edits.iter().rev() {
            inverse.apply(rope);
            applied.push(inverse.clone());
        }
        step.sealed = true;
        let sel = step.sel_before.clone();
        self.redo.push(step);
        Some((sel, applied))
    }

    /// Re-applies the last undone step; returns the selections from after it and the changes applied.
    pub fn redo(&mut self, rope: &mut Rope) -> Option<(Selections, Vec<ChangeSet>)> {
        let step = self.redo.pop()?;
        for (forward, _) in &step.edits {
            forward.apply(rope);
        }
        let applied = step.edits.iter().map(|(f, _)| f.clone()).collect();
        let sel = step.sel_after.clone();
        self.undo.push(step);
        Some((sel, applied))
    }
}
