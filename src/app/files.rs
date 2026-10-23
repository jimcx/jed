//! Opening, saving, closing tabs and quitting.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ropey::Rope;

use super::prompt::{Input, InputKind};
use super::{AfterSave, App, AppEvent, Focus, Level, Prompt, SLOW_TASK};
use crate::buffer::file::{LoadError, Stamp};
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::buffer::{Buffer, BufferId, SaveError, SaveJob};
use crate::command::Command;
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

/// What an open read: a text file's buffer, or a binary file's bytes for a hex tab (the bytes
/// the load read: no second read).
pub(super) enum Loaded {
    /// Boxed: a `Buffer` is several times bigger than any other event.
    Text(Box<Buffer>),
    Binary(crate::hex::HexDoc),
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
    /// Read the file's bytes as they are, for a hex tab ("View: Reopen as Hex").
    pub raw: bool,
    /// "View: Reopen as Hex / Text": the tab arriving replaces the active group's other tab of
    /// the file (`App::add_hex_tab`, `App::replace_hex_with_text`).
    pub reopen: bool,
}

impl App {
    /// Opens `path` in a tab of the active group (or switches to its tab there), with the
    /// selection at `goto` if given. Loading happens off the UI thread; an already open buffer
    /// gets the selection at once.
    pub(super) fn open_file(&mut self, path: PathBuf, focus: bool, goto: Option<Goto>) {
        // A jump: the place before it is recorded for Go Back (`app/navigation.rs`).
        self.nav_jump();
        // The group's hex tab of it (unless it has a text tab of it too); with no buffer open
        // on it, another group's hex tab's bytes in a new tab.
        let open = self.buffers.find_path(&path);
        let text_here = open.is_some_and(|id| self.group().find(id).is_some());
        let hex_here = self.group().find_hex(&path).is_some();
        if ((hex_here && !text_here) || open.is_none()) && self.show_hex(&path, focus) {
            return;
        }
        if let Some(id) = self.buffers.find_path(&path) {
            match self.group().find(id) {
                Some(i) => self.activate_tab(i),
                None => {
                    self.group_mut().open(id, View::new());
                    self.reveal_active();
                }
            }
            let settings = self.settings.clone();
            if let Some(goto) = &goto
                && let Some((buf, view)) = self.active_mut()
            {
                buf.seal_undo();
                apply_goto(view, &buf.rope, goto, settings.for_buffer(buf).tab_width);
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
        self.start_open(
            path,
            OpenRequest {
                focus,
                goto,
                ..Default::default()
            },
        );
    }

    pub(super) fn opened(&mut self, path: PathBuf, result: Result<Loaded, LoadError>) {
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
            raw: _,
            reopen,
        } = self.opening.remove(&path).unwrap_or_default();
        // Said after the load error's own message, which it would otherwise hide.
        let unopened = match &result {
            Err(e) => backup.take().map(|b| (b, e.to_string())),
            Ok(Loaded::Binary(_)) => backup.take().map(|b| (b, "binary file".to_string())),
            Ok(Loaded::Text(_)) => None,
        };
        match result {
            Ok(Loaded::Binary(_)) if !edits.is_empty() => {
                let what = edits[0].what;
                self.notify(
                    Level::Error,
                    format!("{what} not applied to {}: binary file", path.display()),
                )
            }
            Ok(Loaded::Binary(doc)) => {
                if restore.is_some() {
                    let text = format!("{}: binary file, its history entry not restored", path.display());
                    self.notify(Level::Error, text);
                }
                self.add_hex_tab(path, doc, focus, reopen);
            }
            Ok(Loaded::Text(buffer)) => {
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
                } else if reopen && self.group().find_hex(&path).is_some() {
                    let (id, _) = self.register_buffer(*buffer, None);
                    self.replace_hex_with_text(&path, id);
                    if focus {
                        self.ui.focus = Focus::Editor;
                    }
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
                let what = edits[0].what;
                self.notify(Level::Error, format!("{what} not applied to {}: {e}", path.display()))
            }
            Err(e) => self.notify(Level::Error, format!("{}: {e}", path.display())),
        }
        if let Some((b, e)) = unopened {
            self.backup_not_opened(*b, e);
        }
        self.ipc_sweep();
    }

    /// "File: New Untitled File": a new empty "Untitled-N" (the lowest free N) in a tab of the
    /// active group.
    pub(super) fn new_untitled_file(&mut self) {
        let n = self.buffers.free_untitled_number();
        self.add_buffer(Buffer::untitled(n), None, true);
        self.reveal_active();
        tracing::debug!(n, "new untitled buffer");
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
            apply_goto(
                &mut view,
                &buffer.rope,
                goto,
                self.settings.for_buffer(&buffer).tab_width,
            );
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

    /// Shows the active file in the explorer (a text or hex tab's: not a diff's or a log's).
    pub(super) fn reveal_active(&mut self) {
        let path = match self.active_hex() {
            Some(h) => Some(h.path.clone()),
            None => self.active_buffer().filter(|b| b.has_file()).map(|b| b.path.clone()),
        };
        let Some(path) = path.filter(|_| !self.mode.is_single()) else {
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
        self.ipc_sweep();
        if now_empty {
            self.close_group(group);
        }
        if self.groups.len() == 1 && self.group().tabs.is_empty() {
            if self.mode.is_single() {
                // Single file mode: closing the last tab quits.
                self.request_quit();
            } else if self.ui.side_visible {
                self.ui.focus = Focus::SidePanel;
            }
        }
        self.reveal_active();
    }

    fn close_group(&mut self, group: GroupId) {
        if self.groups.remove(group) {
            self.reveal_active();
        }
    }

    /// The buffer Save / Save As act on: the active file buffer (a diff tab's file) or the
    /// active tab's untitled one.
    pub(super) fn active_savable_buffer(&self) -> Option<BufferId> {
        self.active_file_buffer().or_else(|| {
            let id = self.group().active_tab()?.buffer;
            self.buffers.get(id).filter(|b| b.is_untitled()).map(|_| id)
        })
    }

    pub(super) fn save(&mut self, id: BufferId) {
        let Some(buf) = self.buffers.get_mut(id) else { return };
        if buf.is_untitled() {
            self.open_save_as(id);
            return;
        }
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
        self.spawn_save(id, job, None);
    }

    /// Runs `job` off the UI thread and reports `AppEvent::SaveDone` (`to`: a Save As's path).
    fn spawn_save(&mut self, id: BufferId, job: SaveJob, to: Option<PathBuf>) {
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
                to,
            });
            if let Some((history, bytes)) = history {
                let result = tokio::task::spawn_blocking(move || history.record(&bytes))
                    .await
                    .unwrap_or_else(|e| Err(format!("File history not saved: {e}")));
                super::history::send_done(&tx, result);
            }
        });
    }

    /// The path as the Save As prompt shows it: relative to the root when inside it.
    fn save_as_label(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.workspace.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// Opens the Save As prompt for `id`, prefilled with its path (an untitled buffer: its name).
    pub(super) fn open_save_as(&mut self, id: BufferId) {
        let Some(buf) = self.buffers.get(id) else { return };
        if !self.mode.allows(Command::SaveAs) {
            self.cancel_save_as();
            self.notify(Level::Info, "Save As isn't available in single file mode".into());
            return;
        }
        let text = if buf.has_file() {
            self.save_as_label(&buf.path)
        } else {
            buf.name()
        };
        self.prompt = Some(Prompt::Input(save_as_input(id, text)));
    }

    /// Opens the Save As prompt of the next queued untitled buffer, if any.
    pub(super) fn next_save_as(&mut self) {
        while let Some(id) = self.save_as_queue.pop_front() {
            if self.buffers.get(id).is_some_and(Buffer::is_untitled) {
                self.open_save_as(id);
                return;
            }
        }
    }

    /// The Save As flow was abandoned: a pending close / quit and the rest of Save All stop.
    pub(super) fn cancel_save_as(&mut self) {
        self.after_save = None;
        self.save_as_queue.clear();
    }

    /// Whether a Save As prompt (or its overwrite question) is open.
    fn save_as_pending(&self) -> bool {
        match &self.prompt {
            Some(Prompt::ConfirmOverwrite { .. }) => true,
            Some(Prompt::Input(i)) => matches!(i.kind, InputKind::SaveAs { .. }),
            _ => false,
        }
    }

    /// The absolute path a Save As prompt's `text` names for `buffer`, or why it can't be.
    /// Nothing is read from disk here: the write job reports existing files and folders.
    pub(super) fn save_as_target(&self, buffer: BufferId, text: &str) -> Result<PathBuf, &'static str> {
        if text.is_empty() {
            return Err("Enter a file name");
        }
        if text.ends_with('/') {
            return Err("That is a folder: add a file name");
        }
        // `.` and `..` resolved lexically (no disk access), so `sub/../a.c` is `a.c`.
        let mut path = PathBuf::new();
        for part in self.workspace.root.join(text).components() {
            match part {
                std::path::Component::ParentDir => {
                    path.pop();
                }
                std::path::Component::CurDir => {}
                part => path.push(part),
            }
        }
        if self.buffers.find_path(&path).is_some_and(|other| other != buffer) {
            return Err("That file is open in another tab");
        }
        Ok(path)
    }

    /// Saves `buffer` to `path` and makes it the buffer's file (a Save As). To its own path it's
    /// a plain save.
    pub(super) fn save_as_to(&mut self, buffer: BufferId, path: PathBuf, overwrite: bool) {
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        if buf.has_file() && buf.path == path {
            self.save(buffer);
            return;
        }
        if let Some(reason) = buf.read_only.clone() {
            self.cancel_save_as();
            self.notify(Level::Error, format!("Can't save, read-only: {reason}"));
            return;
        }
        if buf.save_in_flight() {
            self.notify(Level::Info, "Save already in progress".into());
            return;
        }
        let job = buf.begin_save_as(path.clone(), overwrite);
        self.spawn_save(buffer, job, Some(path));
    }

    pub(super) fn save_done(
        &mut self,
        id: BufferId,
        state: u64,
        result: Result<Stamp, SaveError>,
        elapsed: Duration,
        to: Option<PathBuf>,
    ) {
        if elapsed > SLOW_TASK {
            tracing::warn!(elapsed_ms = elapsed.as_millis() as u64, "slow save");
        }
        let Some(buf) = self.buffers.get_mut(id) else { return };
        match (&result, &to) {
            (Ok(stamp), Some(to)) => {
                buf.finish_save_as(state, *stamp, to.clone());
                // The path decides the language (unless one was picked).
                let lang = self.detect_language(self.buffers.get(id).expect("just used"));
                if let Some(buf) = self.buffers.get_mut(id) {
                    buf.syntax.set_language(lang);
                }
                self.servers_dirty = true;
            }
            _ => buf.finish_save(state, result.as_ref().ok().copied()),
        }
        let Some(buf) = self.buffers.get(id) else { return };
        let path = to.clone().unwrap_or_else(|| buf.path.clone()).display().to_string();
        let name = buf.name();
        // A stage waiting for this save goes ahead (or is dropped if it failed).
        self.stage_save_done(id, result.is_ok());
        self.stage_all_save_done(id, result.is_ok());
        match result {
            Ok(_) => {
                self.disk_save_done(id);
                self.lsp.saved(&mut self.buffers, id);
                self.notify(Level::Info, format!("Saved {path}"));
                if to.is_some() {
                    self.next_save_as();
                }
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
            Err(SaveError::Exists) => {
                self.disk_save_done(id);
                let Some(to) = to else { return };
                self.prompt = Some(Prompt::ConfirmOverwrite { buffer: id, path: to });
                return;
            }
            Err(SaveError::Failed(e)) => {
                self.disk_save_done(id);
                self.notify(Level::Error, format!("Save failed: {path}: {e}"));
                match to {
                    // Back to the prompt to pick another path.
                    Some(to) => {
                        let text = self.save_as_label(&to);
                        let mut input = save_as_input(id, text);
                        input.error = Some("Can't save there (see the message below)");
                        self.prompt = Some(Prompt::Input(input));
                    }
                    None => self.after_save = None,
                }
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
            Some(AfterSave::Quit)
                if !self.buffers.any_saving() && self.save_as_queue.is_empty() && !self.save_as_pending() =>
            {
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
        self.save_batch(ids, true);
    }

    /// Saves `ids`: files at once, untitled buffers one Save As prompt after another. With
    /// `quit`, a refused save (read-only) stops the lot.
    pub(super) fn save_batch(&mut self, ids: Vec<BufferId>, quit: bool) {
        self.save_as_queue.clear();
        for id in ids {
            if self.buffers.get(id).is_some_and(Buffer::is_untitled) {
                self.save_as_queue.push_back(id);
                continue;
            }
            self.save(id);
            if quit && self.after_save.is_none() {
                self.save_as_queue.clear();
                return;
            }
        }
        self.next_save_as();
    }
}

/// The Save As prompt's input: `text` selected, so typing replaces it.
fn save_as_input(buffer: BufferId, text: String) -> Input {
    let mut input = Input::new(InputKind::SaveAs { buffer }, text);
    input.field.select_all();
    input
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
