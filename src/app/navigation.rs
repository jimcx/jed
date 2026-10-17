//! Back / forward navigation (`app.nav`): recording places before jumps and going to them.
//!
//! Recording happens in one place: `nav_begin` takes the active editor's place before each
//! input event (`handle_event`, `test_execute`) and background result (`handle_app_event`),
//! and `nav_end` compares it with the place after. The place before is recorded when the
//! active group or file changed (opening or switching to another file's tab, a file landing
//! from a quick open, closing a tab), when a jump command moved the primary selection at all
//! without an edit (`nav.jump`, set by `open_file` — quick open, Search view / Problems /
//! Locations results, F8, go to definition — go to line and Cmd+Home / End), or, for input
//! only, when the primary cursor moved `MIN_LINES` lines or more without an edit. Go Back /
//! Go Forward themselves (and the arrival of a file they reopened) record nothing
//! (`nav.navigating`). Entries follow edits: `follow_edits` and `apply_reload` map them through
//! each buffer's applied change sets (`History::map`); positions are clamped when used.

use std::path::PathBuf;

use ropey::Rope;

use super::App;
use super::files::Goto;
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::{Buffer, file::LoadError};
use crate::group::Tab;
use crate::navigation::{Dir, Entry, History, MIN_LINES, Pos};
use crate::selection::{Selection, Selections};
use crate::view::View;

#[derive(Default)]
pub struct NavState {
    pub history: History,
    /// A jump command ran during this event: a move of any size is recorded.
    jump: bool,
    /// Go Back / Go Forward moved during this event (or a file it reopened arrived).
    navigating: bool,
    /// A file Go Back / Go Forward is reopening, the way it went and the history's index then:
    /// skipped if it's gone from disk.
    pending: Option<(PathBuf, Dir, usize)>,
}

/// The active editor's place before an event.
pub(super) struct Here {
    entry: Entry,
    /// Its buffer's revision (0 for a diff tab).
    revision: u64,
}

impl App {
    /// The active editor's place: its group, file and (a text tab's) primary selection and scroll.
    fn here(&self) -> Option<Here> {
        let group = self.group();
        match group.tabs.get(group.active)? {
            Tab::Text(t) => {
                let buf = self.buffers.get(t.buffer)?;
                let primary = t.view.selections.primary();
                let pos = Pos {
                    anchor: primary.anchor,
                    head: primary.head,
                    top: t.view.top_byte(),
                    left_col: t.view.left_col,
                };
                Some(Here {
                    entry: Entry {
                        group: self.groups.active,
                        path: buf.path.clone(),
                        pos: Some(pos),
                    },
                    revision: buf.revision(),
                })
            }
            Tab::Diff(d) => Some(Here {
                entry: Entry {
                    group: self.groups.active,
                    path: d.path.clone(),
                    pos: None,
                },
                revision: 0,
            }),
        }
    }

    /// Start of an event: the place to record if it jumps.
    pub(super) fn nav_begin(&mut self) -> Option<Here> {
        self.nav.jump = false;
        self.nav.navigating = false;
        self.here()
    }

    /// A jump command is running: the place before it is recorded if the selection moves.
    pub(super) fn nav_jump(&mut self) {
        self.nav.jump = true;
    }

    /// End of an event (before `sync_views`, so the recorded place follows the event's edits):
    /// records `before` if the event jumped away from it. `input`: a key or mouse event, for
    /// which a move of `MIN_LINES` or more counts as a jump.
    pub(super) fn nav_end(&mut self, before: Option<Here>, input: bool) {
        if std::mem::take(&mut self.nav.navigating) {
            return;
        }
        let jump = std::mem::take(&mut self.nav.jump);
        let Some(before) = before else { return };
        let after = self.here();
        let record = match &after {
            // The last tab closed.
            None => true,
            Some(a) if a.entry.group != before.entry.group || a.entry.path != before.entry.path => true,
            Some(a) => match (before.entry.pos, a.entry.pos) {
                // Not moved by an edit (typing, undo, a rename's edits).
                (Some(p), Some(q)) if (p.anchor, p.head) != (q.anchor, q.head) && a.revision == before.revision => {
                    jump || (input && self.lines_apart(p.head, q.head) >= MIN_LINES)
                }
                _ => false,
            },
        };
        if record {
            self.nav.history.record(before.entry);
        }
    }

    /// Lines between two positions in the active buffer (the same revision as both).
    fn lines_apart(&self, a: ByteIdx, b: ByteIdx) -> usize {
        self.active_buffer()
            .map_or(0, |buf| pos::line_of(&buf.rope, a).abs_diff(pos::line_of(&buf.rope, b)))
    }

    /// Go Back / Go Forward: the place before (after) the current one in the history, in its
    /// group (the active one if it's closed) with its selection and scroll; its file reopened
    /// (off the UI thread) if its tab was closed. The editor gets the keys.
    pub(super) fn navigate(&mut self, dir: Dir) {
        self.nav.navigating = true;
        self.nav.pending = None;
        let current = self.here().map(|h| h.entry);
        let entry = match dir {
            Dir::Back => self.nav.history.back(current),
            Dir::Forward => self.nav.history.forward(current),
        };
        if let Some(entry) = entry {
            self.show_entry(entry, dir);
        }
    }

    fn show_entry(&mut self, entry: Entry, dir: Dir) {
        let group = match self.groups.get(entry.group) {
            Some(_) => entry.group,
            None => self.groups.active,
        };
        self.focus_group(group);
        let found = {
            let g = self.group();
            let text = g.tabs.iter().position(|t| {
                t.text()
                    .and_then(|t| self.buffers.get(t.buffer))
                    .is_some_and(|b| b.path == entry.path)
            });
            let diff = g
                .tabs
                .iter()
                .position(|t| t.diff().is_some_and(|d| d.path == entry.path));
            // A diff tab counts as its file.
            if entry.pos.is_some() {
                text.or(diff)
            } else {
                diff.or(text)
            }
        };
        if let Some(i) = found {
            self.activate_tab(i);
        } else if let Some(id) = self.buffers.find_path(&entry.path) {
            self.group_mut().open(id, View::new());
            self.reveal_active();
        } else {
            self.nav.pending = Some((entry.path.clone(), dir, self.nav.history.index()));
            self.open_file(entry.path, true, entry.pos.map(Goto::Place));
            return;
        }
        if let Some(p) = entry.pos
            && let Some((buf, view)) = self.active_mut()
        {
            buf.seal_undo();
            place_view(view, &buf.rope, &p);
        }
    }

    /// From `opened`: a file Go Back / Go Forward reopened arrived. Its arrival isn't recorded;
    /// if it's gone from disk (or failed to load) it's skipped — true: the caller drops it — and
    /// navigation goes on the same way.
    pub(super) fn nav_opened(&mut self, path: &std::path::Path, result: &Result<Box<Buffer>, LoadError>) -> bool {
        let Some((pending, dir, index)) = self.nav.pending.take_if(|(p, ..)| p.as_path() == path) else {
            return false;
        };
        self.nav.navigating = true;
        let gone = match result {
            Ok(buf) => buf.disk().is_none(),
            Err(_) => true,
        };
        // Only when nothing else wanted the file meanwhile (a rename's edits, a restore).
        let alone = self
            .opening
            .get(&pending)
            .is_some_and(|r| r.edits.is_empty() && r.restore.is_none() && r.backup.is_none());
        if !gone || !alone {
            return false;
        }
        self.opening.remove(&pending);
        tracing::debug!(path = %pending.display(), "navigation: file gone, skipped");
        if self.nav.history.index() == index {
            self.nav.history.skip(dir);
            self.navigate(dir);
        }
        true
    }
}

/// Puts a view at a recorded place: its selection and scroll, clamped to the text (a file not
/// open may have changed); the cursor is then revealed if the scroll doesn't show it.
pub(super) fn place_view(view: &mut View, rope: &Rope, p: &Pos) {
    let clamp = |b: ByteIdx| ByteIdx(rope.floor_char_boundary(b.0.min(rope.len())));
    view.selections = Selections::single(Selection::new(clamp(p.anchor), clamp(p.head)));
    view.set_top_line(rope, pos::line_of(rope, clamp(p.top)));
    view.left_col = p.left_col;
    view.reveal_range = None;
    view.reveal_pos = None;
    view.reveal_cursor = true;
}
