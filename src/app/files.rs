//! Opening, saving, closing tabs and quitting.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ropey::Rope;

use super::{AfterSave, App, AppEvent, Focus, Level, Prompt, SLOW_TASK};
use crate::buffer::file::LoadError;
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::buffer::{Buffer, BufferId};
use crate::layout::{GroupId, SplitDir};
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
}

/// An open in flight: what to do once the buffer arrives. A later request for the same file
/// replaces its `goto` (the last F4 wins) and can only add focus.
#[derive(Debug)]
pub(super) struct OpenRequest {
    focus: bool,
    goto: Option<Goto>,
}

impl App {
    /// Opens `path` in a tab of the active group (or switches to its tab there), with the
    /// selection at `goto` if given. Loading happens off the UI thread; an already open buffer
    /// gets the selection at once.
    pub(super) fn open_file(&mut self, path: PathBuf, focus: bool, goto: Option<Goto>) {
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
            if goto.is_some() {
                pending.goto = goto;
            }
            return;
        }
        self.opening.insert(path.clone(), OpenRequest { focus, goto });
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = Buffer::open(&path).map(Box::new);
            let _ = tx.send(AppEvent::Opened { path, result });
        });
    }

    pub(super) fn opened(&mut self, path: PathBuf, result: Result<Box<Buffer>, LoadError>) {
        let OpenRequest { focus, goto } = self.opening.remove(&path).unwrap_or(OpenRequest {
            focus: false,
            goto: None,
        });
        match result {
            Ok(buffer) => {
                if let Some(reason) = &buffer.read_only {
                    self.notify(Level::Warning, format!("{}: read-only, {reason}", buffer.name()));
                }
                self.add_buffer(*buffer, goto, focus);
                self.reveal_active();
            }
            Err(LoadError::Binary) => self.notify(Level::Error, format!("{}: binary file, not opened", path.display())),
            Err(e) => self.notify(Level::Error, format!("{}: {e}", path.display())),
        }
    }

    /// Puts a loaded buffer in a new tab and activates it, with the selection at `goto`.
    /// Starts no background work (it runs from `App::new`); callers reveal the file.
    pub(super) fn add_buffer(&mut self, mut buffer: Buffer, goto: Option<Goto>, focus: bool) {
        buffer.syntax.set_language(self.detect_language(&buffer));
        let mut view = View::new();
        if let Some(goto) = &goto {
            apply_goto(&mut view, &buffer.rope, goto, self.settings.tab_width);
            view.cursor_history.reset(&view.selections);
        }
        let id = self.buffers.insert(buffer);
        self.servers_dirty = true;
        self.group_mut().open(id, view);
        if focus {
            self.ui.focus = Focus::Editor;
        }
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
        let Some(path) = self.active_buffer().map(|b| b.path.clone()) else {
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
        self.groups.split(dir);
        self.ui.focus = Focus::Editor;
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
        let id = tab.buffer;
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
        if self.groups.tabs_showing(tab.buffer) == 0 {
            self.buffers.remove(tab.buffer);
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
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let t = Instant::now();
            let result = match tokio::task::spawn_blocking(move || job.run()).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::SaveDone {
                buffer: id,
                state,
                result,
                elapsed: t.elapsed(),
            });
        });
    }

    pub(super) fn save_done(&mut self, id: BufferId, state: u64, result: Result<(), String>, elapsed: Duration) {
        if elapsed > SLOW_TASK {
            tracing::warn!(elapsed_ms = elapsed.as_millis() as u64, "slow save");
        }
        let Some(buf) = self.buffers.get_mut(id) else { return };
        buf.finish_save(state, result.is_ok());
        let path = buf.path.display().to_string();
        match result {
            Ok(()) => self.notify(Level::Info, format!("Saved {path}")),
            Err(e) => {
                self.after_save = None;
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
