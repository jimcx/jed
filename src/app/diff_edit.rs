//! Editing in diff tabs (task diff_edit): a key on an editable side runs through `edit::run` on a
//! scratch `View` whose selection is the caret at the same bytes of the file's buffer, so the
//! edit is the editor's (`Buffer::edit`: undo grouping, dirty, auto-indent and auto-close; the
//! text tabs of the buffer follow in `sync_views`). The caret then goes back into the diff's
//! text (`DiffTab::edited`) and the diff reloads `diff_view::DELAY` later. A file that isn't
//! open is opened behind the diff first (`DiffViews::pending`), the key applied once it loads.

use std::path::{Path, PathBuf};

use super::files::OpenRequest;
use super::{App, Level};
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::{Buffer, BufferId};
use crate::clipboard::Clipboard;
use crate::command::Command;
use crate::diff_caret::Caret;
use crate::diff_view::{DiffTab, Source};
use crate::edit;
use crate::selection::{Selection, Selections};
use crate::view::View;

/// What a key does to a diff tab's file.
#[derive(Debug, Clone)]
pub(super) enum DiffEdit {
    Command(Command),
    Char(char),
    Paste(String),
    Cut,
}

/// An edit waiting for its file to open.
pub(super) struct PendingEdit {
    tab: u64,
    path: PathBuf,
    edit: DiffEdit,
}

/// Where a side's text is edited.
enum Target {
    Open(BufferId),
    File(PathBuf),
}

impl App {
    /// Where side `right` of `d` is edited: its buffer, else its file (to open); `None` when the
    /// side is read-only.
    fn edit_target(&self, d: &DiffTab, right: bool) -> Option<Target> {
        if let Some(c) = &d.compare {
            return match &c.sides[right as usize].source {
                Source::Open(id) => self.buffers.get(*id).map(|_| Target::Open(*id)),
                Source::File(p) => Some(match self.buffers.find_path(p) {
                    Some(id) => Target::Open(id),
                    None => Target::File(p.clone()),
                }),
            };
        }
        if !d.editable_sides()[right as usize] {
            return None;
        }
        Some(match self.buffer_of(&d.path) {
            Some(id) => Target::Open(id),
            None => Target::File(d.path.clone()),
        })
    }

    /// A key that edits, on the active diff tab.
    pub(super) fn diff_edit(&mut self, edit: DiffEdit) {
        let Some(d) = self.group().active_diff() else { return };
        let (tab, right) = (d.id, d.caret.right);
        match self.edit_target(d, right) {
            None => {
                let side = if right { "right" } else { "left" };
                self.notify(Level::Error, format!("The {side} side is read-only"));
            }
            Some(Target::Open(id)) => self.diff_edit_apply(tab, id, edit),
            Some(Target::File(path)) => {
                // One at a time: the keys typed before the file loads are dropped.
                if self.diffs.pending.is_some() {
                    return;
                }
                if !self.opening.contains_key(&path) {
                    let request = OpenRequest {
                        background: true,
                        ..Default::default()
                    };
                    self.start_open(path.clone(), request);
                }
                self.diffs.pending = Some(PendingEdit { tab, path, edit });
            }
        }
    }

    /// A file finished opening: the key that waited for it applies (or is dropped when the file
    /// didn't open as text; its error was said).
    pub(super) fn diff_edit_opened(&mut self, path: &Path) {
        if self.diffs.pending.as_ref().is_none_or(|p| p.path != path) {
            return;
        }
        let Some(p) = self.diffs.pending.take() else { return };
        if let Some(id) = self.buffers.find_path(path) {
            self.diff_edit_apply(p.tab, id, p.edit);
        }
    }

    fn diff_edit_apply(&mut self, tab: u64, id: BufferId, edit: DiffEdit) {
        let settings = self.settings.clone();
        let (Some(buf), Some((_, d))) = (
            self.buffers.get_mut(id),
            self.groups.diff_tabs_mut().find(|(_, d)| d.id == tab),
        ) else {
            return;
        };
        if let Err(e) = edit_buffer(d, buf, &mut self.clipboard, &settings, edit) {
            self.notify(Level::Error, e);
        }
        self.needs_redraw = true;
    }
}

/// The caret of `d` as a selection of `buf`, `edit` applied, the caret back in the diff. The
/// diff's lines must be the buffer's (a changed buffer not read yet: refused), unless an edit
/// here is waiting for its reload (the caret is then in the buffer's text already).
fn edit_buffer(
    d: &mut DiffTab,
    buf: &mut Buffer,
    clipboard: &mut Clipboard,
    settings: &edit::Settings,
    edit: DiffEdit,
) -> Result<(), String> {
    if let Some(reason) = &buf.read_only {
        return Err(format!("Read-only: {reason}"));
    }
    // Waiting on the reload of an edit here, with no other edit since: its caret is in the
    // buffer's text (`d.caret` is that caret clamped to the lines still shown).
    let waiting = d.edited.filter(|&(_, rev)| rev == buf.revision()).map(|(c, _)| c);
    let c = waiting.unwrap_or(d.caret);
    let waiting = waiting.is_some();
    let rope = &buf.rope;
    let at = |(line, byte): (usize, usize)| -> Result<ByteIdx, String> {
        let stale = || "The diff is updating, try again".to_string();
        if line >= pos::line_count(rope) {
            return Err(stale());
        }
        let (start, end) = (pos::line_start(rope, line), pos::line_end(rope, line));
        let same = waiting || rope.slice(start.0..end.0) == d.side_text(c.right, line);
        if byte > end.0 - start.0 || !same {
            return Err(stale());
        }
        Ok(ByteIdx(start.0 + byte))
    };
    let mut view = View::new();
    view.selections = Selections::single(Selection::new(at(c.anchor)?, at(c.head)?));
    let before = buf.revision();
    match edit {
        DiffEdit::Command(cmd) => {
            edit::run(cmd, buf, &mut view, settings);
        }
        DiffEdit::Char(ch) => edit::insert_char(buf, &mut view, ch, settings),
        DiffEdit::Paste(text) => {
            let (whole_line, pieces) = clipboard
                .copied(&text)
                .map_or((false, &[][..]), |c| (c.whole_line, c.pieces.as_slice()));
            edit::paste(buf, &mut view, &text, whole_line, pieces);
        }
        DiffEdit::Cut => {
            let copied = edit::copy_text(buf, &view);
            edit::cut(buf, &mut view);
            clipboard.write(copied);
        }
    }
    let sel = *view.selections.primary();
    let place = |p: ByteIdx| {
        let line = pos::line_of(&buf.rope, p);
        (line, p.0 - pos::line_start(&buf.rope, line).0)
    };
    let caret = Caret {
        anchor: place(sel.anchor),
        head: place(sel.head),
        col: None,
        ..c
    };
    // The caret shown stays valid in the lines shown (clamped: drawing and moves slice them);
    // the edited caret waits in `edited` for the load of this revision (`take_edited`).
    // Nothing changed (a Backspace at the start) and nothing waiting: the lines are the buffer's.
    if buf.revision() != before || waiting {
        d.edited = Some((caret, buf.revision()));
    }
    d.place_caret(caret);
    Ok(())
}
