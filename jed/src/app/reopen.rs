//! Reopen Closed Editor (Cmd+Shift+T): a stack of closed file tabs (`app.closed`) with their place.
//!
//! `discard_tab` (every user close path) calls `remember_closed` before the tab goes; text tabs
//! of a file on disk are kept (not untitled, log or commit buffers, nor diff, hex, preview or
//! shell tabs), newest last, at most `MAX_CLOSED`. `reopen_closed_tab` pops the newest: in its
//! group if that still exists (else the active one), its file reopened off the UI thread with
//! its selection and scroll (`Goto::Place`, as Go Back). A file gone from disk is skipped, as
//! Go Back skips it (`reopen_opened`, from `opened`), and the next entry is tried.

use std::path::PathBuf;

use super::files::Goto;
use super::{App, Level};
use crate::buffer::file::LoadError;
use crate::group::{Tab, is_terminal_group};
use crate::layout::GroupId;
use crate::navigation::{Entry, Place, Pos};

/// Closed tabs kept.
const MAX_CLOSED: usize = 50;

#[derive(Default)]
pub struct ClosedTabs {
    stack: Vec<Entry>,
    /// A file being reopened, skipped if it turns out to be gone.
    pending: Option<PathBuf>,
}

impl ClosedTabs {
    fn push(&mut self, entry: Entry) {
        // One entry per file: closing it again replaces its older place.
        self.stack.retain(|e| e.path != entry.path);
        self.stack.push(entry);
        if self.stack.len() > MAX_CLOSED {
            self.stack.remove(0);
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.stack.len()
    }
}

impl App {
    /// Before a tab closes: keeps a file's text tab on the stack.
    pub(super) fn remember_closed(&mut self, group: GroupId, index: usize) {
        if is_terminal_group(group) {
            return;
        }
        let Some(Tab::Text(t)) = self.any_group(group).and_then(|g| g.tabs.get(index)) else {
            return;
        };
        let Some(buf) = self.buffers.get(t.buffer).filter(|b| b.has_file()) else {
            return;
        };
        let primary = t.view.selections.primary();
        let entry = Entry {
            group,
            path: buf.path.clone(),
            place: Some(Place::Text(Pos {
                anchor: primary.anchor,
                head: primary.head,
                top: t.view.top_byte(),
                left_col: t.view.left_col,
            })),
        };
        self.closed.push(entry);
    }

    /// "View: Reopen Closed Editor": the newest closed file tab comes back.
    pub(super) fn reopen_closed_tab(&mut self) {
        self.closed.pending = None;
        let Some(entry) = self.closed.stack.pop() else {
            self.notify(Level::Info, "No closed editors to reopen".into());
            return;
        };
        let group = if self.groups.get(entry.group).is_some() {
            entry.group
        } else {
            self.groups.active
        };
        self.focus_group(group);
        if self.buffers.find_path(&entry.path).is_none() {
            self.closed.pending = Some(entry.path.clone());
        }
        let goto = entry.pos().map(Goto::Place);
        self.open_file(entry.path, true, goto);
    }

    /// From `opened`: a file Reopen Closed Editor loaded. If it's gone from disk (or failed to
    /// load) it's dropped — true: the caller stops — and the next closed tab is tried.
    pub(super) fn reopen_opened(
        &mut self,
        path: &std::path::Path,
        result: &Result<super::files::Loaded, LoadError>,
    ) -> bool {
        if self.closed.pending.as_deref() != Some(path) {
            return false;
        }
        self.closed.pending = None;
        let gone = match result {
            Ok(super::files::Loaded::Text(buf)) => buf.disk().is_none(),
            Ok(super::files::Loaded::Binary(_)) => false,
            Err(_) => true,
        };
        let alone = self
            .opening
            .get(path)
            .is_some_and(|r| r.edits.is_empty() && r.restore.is_none() && r.backup.is_none());
        if !gone || !alone {
            return false;
        }
        self.opening.remove(path);
        tracing::debug!(path = %path.display(), "reopen closed tab: file gone, skipped");
        self.reopen_closed_tab();
        true
    }
}
