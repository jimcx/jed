//! Merge conflicts in the app: Next / Previous Conflict (the cursor to a conflict's `<<<<<<<`
//! line, wrapping around) and Compare Current Conflict (a read-only diff tab of the conflict's
//! current and incoming blocks). Accepting a side is `edit::merge_command`; the editor draws the
//! blocks and the action row (`ui::editor`); the model is `crate::conflict`.

use std::path::PathBuf;

use super::App;
use crate::buffer::pos::ByteIdx;
use crate::conflict::{self, Choice};
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
