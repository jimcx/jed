//! Opening, saving, closing tabs and quitting.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ropey::Rope;

use super::{AfterSave, App, AppEvent, Focus, Level, Prompt, SLOW_TASK};
use crate::buffer::file::{LoadError, Stamp};
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::buffer::{Buffer, BufferId, SaveError};
use crate::layout::{GroupId, SplitDir};
use crate::problems::Place;
use crate::selection::{Selection, Selections};
use crate::view::View;

/// Where to put the selection in a file being opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Goto {
    /// A cursor at a 1-based line and display column (`file:line:col`).
    Cursor(usize, usize),
    /// A workspace search match: 0-based line and byte offsets into it, selected (anchor at the
    /// start) and centred if off screen. Clamped to the text, which may have changed since.
    Match { line: usize, range: Range<usize> },
    /// A problem (the Problems list, F8): its range selected (`select`) or a cursor at its
    /// start, centred if off screen. A `Place::Text` range is in the buffer's current text
    /// (clamped); a `Place::Lsp` one is read in the text as loaded.
    Problem { place: Place, select: bool },
    /// A place Go Back / Go Forward returns to: its selection and scroll, clamped.
    Place(crate::navigation::Pos),
}

/// An open in flight: what to do once the buffer arrives. A later request for the same file
/// replaces its `goto` (the last F4 wins) and can only add focus.
#[derive(Debug, Default)]
pub(super) struct OpenRequest {
    pub focus: bool,
    pub goto: Option<Goto>,
    /// Opened only for `edits` (a rename's file not open): a tab beside the active one, which
    /// stays active. Any other request for the file clears it.
    pub background: bool,
    /// A rename's edits to make once it's loaded (`App::apply_pending_edits`).
    pub edits: Vec<super::rename::PendingEdit>,
    /// A file history entry to restore into the buffer once it's loaded (Restore on a file
    /// that isn't open: `App::restore_history`).
    pub restore: Option<crate::history::Snapshot>,
    /// A crash backup to restore into the buffer once it's loaded (`App::restore_backup`).
    pub backup: Option<Box<crate::backup::Backup>>,
}

impl App {
    /// Opens `path` in a tab of the active group (or switches to its tab there), with the
    /// selection at `goto` if given. Loading happens off the UI thread; an already open buffer
    /// gets the selection at once.
    pub(super) fn open_file(&mut self, path: PathBuf, focus: bool, goto: Option<Goto>) {
        // A jump: the place before it is recorded for Go Back (`app/navigation.rs`).
        self.nav_jump();
        if let Some(id) = self.buffers.find_path(&path) {
            match self.group().find(id) {
                Some(i) => self.activate_tab(i),
                None => {
                    self.group_mut().open(id, View::new());
                    self.reveal_active();
                }
            }
            let tab_width = self.settings.tab_width;
            if let Some(goto) = &goto
                && let Some((buf, view)) = self.active_mut()
            {
                buf.seal_undo();
                apply_goto(view, &buf.rope, goto, tab_width);
            }
            if focus {
                self.ui.focus = Focus::Editor;
            }
            return;
        }
        if let Some(pending) = self.opening.get_mut(&path) {
            pending.focus |= focus;
            pending.background = false;
            if goto.is_some() {
                pending.goto = goto;
            }
            return;
        }
        self.opening.insert(
            path.clone(),
            OpenRequest {
                focus,
                goto,
                ..Default::default()
            },
        );
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = Buffer::open(&path).map(Box::new);
            let _ = tx.send(AppEvent::Opened { path, result });
        });
    }

    pub(super) fn opened(&mut self, path: PathBuf, result: Result<Box<Buffer>, LoadError>) {
        if self.nav_opened(&path, &result) {
            return;
        }
        let OpenRequest {
            focus,
            goto,
            background,
            edits,
            restore,
            mut backup,
        } = self.opening.remove(&path).unwrap_or_default();
        // Said after the load error's own message, which it would otherwise hide.
        let unopened = match &result {
            Err(e) => backup.take().map(|b| (b, e.to_string())),
            Ok(_) => None,
        };
        match result {
            Ok(buffer) => {
                if let Some(reason) = &buffer.read_only
                    && edits.is_empty()
                {
                    self.notify(Level::Warning, format!("{}: read-only, {reason}", buffer.name()));
                }
                if self.buffers.find_path(&path).is_some() {
                    // Loaded meanwhile (a session's restore): shown from that buffer, not a
                    // second one.
                    if !background {
                        self.open_file(path.clone(), focus, goto);
                    }
                } else if background {
                    let (id, view) = self.register_buffer(*buffer, None);
                    self.group_mut().open_background(id, view);
                } else {
                    self.add_buffer(*buffer, goto, focus);
                    self.reveal_active();
                }
                if !edits.is_empty() {
                    self.apply_pending_edits(&path, edits);
                }
                if let Some(snapshot) = restore
                    && let Some(rel) = crate::history::relative(&self.workspace.root, &path)
                {
                    self.restore_history(path, rel, snapshot);
                }
                if let Some(b) = backup {
                    self.restore_backup(*b);
                }
            }
            Err(e) if !edits.is_empty() => {
                self.notify(Level::Error, format!("Rename not applied to {}: {e}", path.display()))
            }
            Err(LoadError::Binary) => self.notify(Level::Error, format!("{}: binary file, not opened", path.display())),
            Err(e) => self.notify(Level::Error, format!("{}: {e}", path.display())),
        }
        if let Some((b, e)) = unopened {
            self.backup_not_opened(*b, e);
        }
    }

    /// Puts a loaded buffer in a new tab and activates it, with the selection at `goto`.
    /// Starts no background work (it runs from `App::new`); callers reveal the file.
    pub(super) fn add_buffer(&mut self, buffer: Buffer, goto: Option<Goto>, focus: bool) {
        let (id, view) = self.register_buffer(buffer, goto);
        self.group_mut().open(id, view);
        if focus {
            self.ui.focus = Focus::Editor;
        }
    }

    /// Adds a loaded buffer (language, stored diagnostics) and makes its first view, with the
    /// selection at `goto`; the caller puts it in a tab.
    pub(super) fn register_buffer(&mut self, mut buffer: Buffer, goto: Option<Goto>) -> (BufferId, View) {
        buffer.syntax.set_language(self.detect_language(&buffer));
        self.lsp.load_diagnostics(&mut buffer);
        let mut view = View::new();
        if let Some(goto) = &goto {
            apply_goto(&mut view, &buffer.rope, goto, self.settings.tab_width);
            view.cursor_history.reset(&view.selections);
        }
        let id = self.buffers.insert(buffer);
        self.servers_dirty = true;
        (id, view)
    }

    pub(super) fn activate_tab(&mut self, index: usize) {
        if index < self.group().tabs.len() {
            if let Some((buf, _)) = self.active_mut() {
                buf.seal_undo();
            }
            self.group_mut().active = index;
            self.reveal_active();
        }
    }

    pub(super) fn cycle_tab(&mut self, delta: isize) {
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        self.group_mut().cycle(delta);
        self.reveal_active();
    }

    /// Shows the active file in the explorer.
    pub(super) fn reveal_active(&mut self) {
        let Some(path) = self.active_buffer().filter(|b| b.has_file()).map(|b| b.path.clone()) else {
            return;
        };
        for dir in self.explorer.reveal(&path) {
            self.request_listing(dir);
        }
    }

    /// Makes `group` the active group and focuses the editor.
    pub(super) fn focus_group(&mut self, group: GroupId) {
        if self.groups.get(group).is_none() {
            return;
        }
        if group != self.groups.active
            && let Some((buf, _)) = self.active_mut()
        {
            buf.seal_undo();
        }
        self.groups.active = group;
        self.ui.focus = Focus::Editor;
        self.reveal_active();
    }

    pub(super) fn split(&mut self, dir: SplitDir) {
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let diff_id = self.next_diff_id();
        self.groups.split(dir, diff_id);
        self.ui.focus = Focus::Editor;
    }

    /// "Move Editor into Next / Previous Group": the active tab, its view as it is, into the
    /// neighbouring group (`Groups::move_active_tab`), which gets the focus.
    pub(super) fn move_tab_to_group(&mut self, forward: bool) {
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        if self.groups.move_active_tab(forward) {
            self.ui.focus = Focus::Editor;
            self.reveal_active();
        }
    }

    /// "Move Editor Left / Right": the active tab one place along its group's tab bar.
    pub(super) fn move_tab(&mut self, delta: isize) {
        self.group_mut().move_active(delta);
    }

    /// Closes a tab; asks first if it's the last tab (in any group) showing an unsaved buffer.
    pub(super) fn close_tab(&mut self, group: GroupId, index: usize) {
        let Some(tab) = self.groups.get(group).and_then(|g| g.tabs.get(index)) else {
            // Closing in an empty group closes the group.
            if self.groups.get(group).is_some_and(|g| g.tabs.is_empty()) {
                self.close_group(group);
            }
            return;
        };
        // A diff tab closes without asking.
        let Some(id) = tab.buffer() else {
            self.discard_tab(group, index);
            return;
        };
        if self.groups.tabs_showing(id) == 1 && self.buffers.get(id).is_some_and(Buffer::is_dirty) {
            self.groups.active = group;
            self.group_mut().active = index;
            self.prompt = Some(Prompt::SaveBeforeClose { group, buffer: id });
            return;
        }
        self.discard_tab(group, index);
    }

    /// Closes a tab without asking; drops its buffer if no tab anywhere shows it,
    /// and the group if it's now empty (unless it's the last group).
    pub(super) fn discard_tab(&mut self, group: GroupId, index: usize) {
        let Some(g) = self.groups.get_mut(group) else { return };
        if index >= g.tabs.len() {
            return;
        }
        let tab = g.close(index);
        let now_empty = g.tabs.is_empty();
        if let Some(buffer) = tab.buffer()
            && self.groups.tabs_showing(buffer) == 0
        {
            self.buffers.remove(buffer);
        }
        if now_empty {
            self.close_group(group);
        }
        if self.groups.len() == 1 && self.group().tabs.is_empty() && self.ui.side_visible {
            self.ui.focus = Focus::SidePanel;
        }
        self.reveal_active();
    }

    fn close_group(&mut self, group: GroupId) {
        if self.groups.remove(group) {
            self.reveal_active();
        }
    }

    pub(super) fn save(&mut self, id: BufferId) {
        let Some(buf) = self.buffers.get_mut(id) else { return };
        if let Some(reason) = buf.read_only.clone() {
            self.after_save = None;
            self.notify(Level::Error, format!("Can't save, read-only: {reason}"));
            return;
        }
        if buf.save_in_flight() {
            self.notify(Level::Info, "Save already in progress".into());
            return;
        }
        let job = buf.begin_save();
        let state = job.state;
        let history = self.history_job(&job.path);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let t = Instant::now();
            let (result, bytes) = match tokio::task::spawn_blocking(move || job.write()).await {
                Ok(Ok((stamp, bytes))) => (Ok(stamp), Some(bytes)),
                Ok(Err(e)) => (Err(e), None),
                Err(e) => (Err(SaveError::Failed(e.to_string())), None),
            };
            // The snapshot is taken after the rename, off the UI thread, and never fails the save.
            let history = history.zip(bytes);
            let _ = tx.send(AppEvent::SaveDone {
                buffer: id,
                state,
                result,
                elapsed: t.elapsed(),
                history: history.is_some(),
            });
            if let Some((history, bytes)) = history {
                let result = tokio::task::spawn_blocking(move || history.record(&bytes))
                    .await
                    .unwrap_or_else(|e| Err(format!("File history not saved: {e}")));
                super::history::send_done(&tx, result);
            }
        });
    }

    pub(super) fn save_done(&mut self, id: BufferId, state: u64, result: Result<Stamp, SaveError>, elapsed: Duration) {
        if elapsed > SLOW_TASK {
            tracing::warn!(elapsed_ms = elapsed.as_millis() as u64, "slow save");
        }
        let Some(buf) = self.buffers.get_mut(id) else { return };
        buf.finish_save(state, result.as_ref().ok().copied());
        let path = buf.path.display().to_string();
        let name = buf.name();
        // A stage waiting for this save goes ahead (or is dropped if it failed).
        self.stage_save_done(id, result.is_ok());
        self.stage_all_save_done(id, result.is_ok());
        match result {
            Ok(_) => {
                self.disk_save_done(id);
                self.lsp.saved(&mut self.buffers, id);
                self.notify(Level::Info, format!("Saved {path}"));
            }
            Err(SaveError::Conflict) => {
                self.after_save = None;
                // Shows the banner (or reloads a buffer without edits).
                self.check_disk(id, false);
                self.disk_save_done(id);
                let text = format!("{name} changed on disk: not saved. Reload it, or Keep Mine to overwrite it");
                self.notify(Level::Error, text);
                return;
            }
            Err(SaveError::Failed(e)) => {
                self.after_save = None;
                self.disk_save_done(id);
                self.notify(Level::Error, format!("Save failed: {path}: {e}"));
                return;
            }
        }
        match self.after_save {
            Some(AfterSave::Close(group, closing)) if closing == id => {
                self.after_save = None;
                if let Some(i) = self.groups.get(group).and_then(|g| g.find(id)) {
                    self.close_tab(group, i);
                }
            }
            Some(AfterSave::Quit) if !self.buffers.any_saving() => {
                self.after_save = None;
                self.request_quit();
            }
            _ => {}
        }
    }

    pub(super) fn request_quit(&mut self) {
        if self.buffers.any_saving() {
            self.after_save = Some(AfterSave::Quit);
            self.notify(Level::Info, "Waiting for saves to finish…".into());
            return;
        }
        let dirty = self.buffers.dirty();
        if dirty.is_empty() {
            self.should_quit = true;
        } else {
            self.prompt = Some(Prompt::SaveAllBeforeQuit { buffers: dirty });
        }
    }

    pub(super) fn save_all_then_quit(&mut self, ids: Vec<BufferId>) {
        self.after_save = Some(AfterSave::Quit);
        for id in ids {
            self.save(id);
            if self.after_save.is_none() {
                // A save was refused (read-only): stop.
                return;
            }
        }
    }
}

pub(super) fn apply_goto(view: &mut View, rope: &Rope, goto: &Goto, tab_width: usize) {
    match goto {
        Goto::Cursor(line, col) => place_cursor(view, rope, (*line, *col), tab_width),
        Goto::Match { line, range } => select_in_line(view, rope, *line, range),
        Goto::Problem { place, select } => {
            let clamp = |b: ByteIdx| ByteIdx(rope.floor_char_boundary(b.0.min(rope.len())));
            let (start, end) = match *place {
                Place::Text { start, end } => (clamp(start), clamp(end)),
                Place::Lsp { start, end, encoding } => (
                    pos::from_lsp_pos(rope, start, encoding),
                    pos::from_lsp_pos(rope, end, encoding),
                ),
            };
            let end = end.max(start);
            let head = if *select { end } else { start };
            view.selections = Selections::single(Selection::new(start, head));
            view.reveal_range = Some((start, end));
        }
        Goto::Place(p) => super::navigation::place_view(view, rope, p),
    }
}

/// Selects `range` (byte offsets) on 0-based line `line` and centres it if it's off screen. The
/// line, the offsets (to the line's end) and char boundaries are clamped: the text may have
/// changed since the search.
fn select_in_line(view: &mut View, rope: &Rope, line: usize, range: &Range<usize>) {
    let line = line.min(pos::line_count(rope) - 1);
    let (start, end, _) = pos::line_bounds(rope, line);
    let at = |offset: usize| ByteIdx(rope.floor_char_boundary(start.0 + offset.min(end.0 - start.0)));
    let (anchor, head) = (at(range.start), at(range.end.max(range.start)));
    view.selections = Selections::single(Selection::new(anchor, head));
    view.reveal_range = Some((anchor, head));
}

/// Puts a single cursor at a 1-based line and display column (both clamped) and scrolls to it.
pub(super) fn place_cursor(view: &mut View, rope: &Rope, (line, col): (usize, usize), tab_width: usize) {
    let line = line.saturating_sub(1).min(pos::line_count(rope) - 1);
    let at = pos::byte_at_col(rope, line, DisplayCol(col.saturating_sub(1)), tab_width);
    view.selections = Selections::single(Selection::cursor(at));
    view.reveal_cursor = true;
}
