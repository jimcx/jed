//! Opening, saving, closing tabs and quitting.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{AfterSave, App, AppEvent, Focus, Level, Prompt, SLOW_TASK};
use crate::buffer::file::LoadError;
use crate::buffer::pos::{self, DisplayCol};
use crate::buffer::{Buffer, BufferId};
use crate::selection::{Selection, Selections};
use crate::view::View;

impl App {
    /// Opens `path` in a tab (or switches to its tab). Loading happens off the UI thread.
    pub(super) fn open_file(&mut self, path: PathBuf, focus: bool) {
        if let Some(id) = self.buffers.find_path(&path) {
            match self.group.find(id) {
                Some(i) => self.activate_tab(i),
                None => {
                    self.group.open(id, View::new());
                    self.reveal_active();
                }
            }
            if focus {
                self.ui.focus = Focus::Editor;
            }
            return;
        }
        if !self.opening.insert(path.clone()) {
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = Buffer::open(&path);
            let _ = tx.send(AppEvent::Opened { path, result, focus });
        });
    }

    pub(super) fn opened(&mut self, path: PathBuf, result: Result<Buffer, LoadError>, focus: bool) {
        self.opening.remove(&path);
        match result {
            Ok(buffer) => {
                if let Some(reason) = &buffer.read_only {
                    self.notify(Level::Warning, format!("{}: read-only, {reason}", buffer.name()));
                }
                self.add_buffer(buffer, None, focus);
                self.reveal_active();
            }
            Err(LoadError::Binary) => self.notify(Level::Error, format!("{}: binary file, not opened", path.display())),
            Err(e) => self.notify(Level::Error, format!("{}: {e}", path.display())),
        }
    }

    /// Puts a loaded buffer in a new tab and activates it. `goto` is 1-based line and column.
    /// Starts no background work (it runs from `App::new`); callers reveal the file.
    pub(super) fn add_buffer(&mut self, buffer: Buffer, goto: Option<(usize, usize)>, focus: bool) {
        let mut view = View::new();
        if let Some((line, col)) = goto {
            let line = line.saturating_sub(1).min(pos::line_count(&buffer.rope) - 1);
            let at = pos::byte_at_col(
                &buffer.rope,
                line,
                DisplayCol(col.saturating_sub(1)),
                self.settings.tab_width,
            );
            view.selections = Selections::single(Selection::cursor(at));
        }
        let id = self.buffers.insert(buffer);
        self.group.open(id, view);
        if focus {
            self.ui.focus = Focus::Editor;
        }
    }

    pub(super) fn activate_tab(&mut self, index: usize) {
        if index < self.group.tabs.len() {
            if let Some((buf, _)) = self.active_mut() {
                buf.seal_undo();
            }
            self.group.active = index;
            self.reveal_active();
        }
    }

    pub(super) fn cycle_tab(&mut self, delta: isize) {
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        self.group.cycle(delta);
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

    pub(super) fn close_tab(&mut self, index: usize) {
        let Some(tab) = self.group.tabs.get(index) else { return };
        let id = tab.buffer;
        let last_tab_for_buffer = self.group.tabs.iter().filter(|t| t.buffer == id).count() == 1;
        if last_tab_for_buffer && self.buffers.get(id).is_some_and(Buffer::is_dirty) {
            self.group.active = index;
            self.prompt = Some(Prompt::SaveBeforeClose { buffer: id });
            return;
        }
        self.discard_tab(index);
    }

    /// Closes a tab without asking; drops its buffer if no other tab shows it.
    pub(super) fn discard_tab(&mut self, index: usize) {
        let tab = self.group.close(index);
        if self.group.find(tab.buffer).is_none() {
            self.buffers.remove(tab.buffer);
        }
        if self.group.tabs.is_empty() && self.ui.side_visible {
            self.ui.focus = Focus::SidePanel;
        }
        self.reveal_active();
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
            Some(AfterSave::Close(closing)) if closing == id => {
                self.after_save = None;
                if let Some(i) = self.group.find(id) {
                    self.close_tab(i);
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
