//! The Source Control view in the app (`app.scm`, a `scm::ScmView`; drawn by `ui::scm`): its
//! focus and keys (`Context::Scm`), opening a file's diff (`open_diff` with its section's
//! pair; a conflicted file opens the file itself), staging / unstaging the selected file or a
//! whole section — a file through the diff view's `stage_path` (an open dirty buffer saved
//! first, as from the gutter; a conflicted file still holding markers asks before it's staged
//! as resolved), Stage All after saving every dirty buffer among its files (one prompt) — and
//! the mouse (rows, buttons, wheel, the row under the pointer; Ctrl / Cmd+click and Shift+click
//! pick files, whose buttons then act on all of them). The list itself is the git
//! status, read again by `app/git.rs` after `notify` events: `scm_status_changed` keeps the
//! selection on its file.

use std::collections::HashSet;
use std::path::PathBuf;

use crossterm::event::{KeyModifiers, MouseEvent};
use ratatui::layout::Position;

use super::{App, Focus, Level, Prompt};
use crate::buffer::BufferId;
use crate::command::ScmCommand as C;
use crate::git::stage;
use crate::layout::{Hit, SidePanel};
use crate::scm::{self, Row, RowKey, Section};

impl App {
    /// The Source Control view has the keys.
    pub(super) fn scm_focused(&self) -> bool {
        self.ui.focus == Focus::SidePanel && self.ui.side_panel == SidePanel::SourceControl
    }

    /// Ctrl+Shift+G (and its activity bar icon): shows the view and gives it the keys.
    pub(super) fn focus_scm(&mut self) {
        self.ensure_status_loaded();
        self.focus_side(SidePanel::SourceControl);
    }

    /// With nothing selected, selects the first file (else the first row).
    pub(super) fn scm_select_first(&mut self) {
        let Some(status) = self.git.status.as_ref() else { return };
        if self.scm.has_selection() {
            return;
        }
        let rows = self.scm.rows(status);
        let first = rows.iter().position(|r| matches!(r, Row::File { .. })).unwrap_or(0);
        self.scm.select(&rows, first);
    }

    /// After every status read: the selection follows its file (or what took its place).
    pub(super) fn scm_status_changed(&mut self) {
        match self.git.status.as_ref() {
            Some(status) => {
                let rows = self.scm.rows(status);
                self.scm.settle(&rows);
                self.scm.clamp(&rows);
            }
            None => self.scm.settle(&[]),
        }
    }

    pub(super) fn scm_command(&mut self, cmd: C) {
        let Some(status) = self.git.status.as_ref() else {
            if cmd == C::FocusEditor {
                self.scm_focus_editor();
            }
            return;
        };
        let rows = self.scm.rows(status);
        let page = self.scm.height.max(1) as isize;
        let view = &mut self.scm;
        match cmd {
            C::Up => view.move_selection(&rows, -1),
            C::Down => view.move_selection(&rows, 1),
            C::PageUp => view.move_selection(&rows, -page),
            C::PageDown => view.move_selection(&rows, page),
            C::First => view.select(&rows, 0),
            C::Last => view.select(&rows, rows.len().saturating_sub(1)),
            C::Collapse => match view.selected(&rows) {
                Some(Row::Header { section, .. }) => view.set_collapsed(section, true),
                Some(Row::File { section, .. }) => {
                    view.select_key(&rows, &RowKey::Header(section));
                }
                None => {}
            },
            C::Expand => {
                if let Some(Row::Header { section, .. }) = view.selected(&rows) {
                    view.set_collapsed(section, false);
                }
            }
            C::Open => self.scm_open(true),
            C::Preview => self.scm_open(false),
            C::FocusEditor => self.scm_focus_editor(),
        }
    }

    fn scm_focus_editor(&mut self) {
        if !self.group().tabs.is_empty() {
            self.ui.focus = Focus::Editor;
        }
    }

    /// The selected row: a file's diff opens (the editor focused if `focus`, else the keys stay
    /// in the list) — a conflicted file opens the file itself, with its markers — a header
    /// collapses / expands its section.
    fn scm_open(&mut self, focus: bool) {
        let Some(status) = self.git.status.as_ref() else { return };
        let rows = self.scm.rows(status);
        match self.scm.selected(&rows) {
            Some(Row::Header { section, .. }) => self.scm.toggle(section),
            Some(Row::File {
                section: Section::MergeChanges,
                entry,
                ..
            }) => {
                // VS Code opens a conflicted file, not a diff: its markers are resolved in it.
                let path = status.repo.workdir.join(&entry.path);
                self.open_file(path, focus, None);
                self.side_opened();
            }
            Some(Row::File { section, entry, .. }) => {
                let path = status.repo.workdir.join(&entry.path);
                self.open_diff(path, section.pair());
                if !focus {
                    self.ui.focus = Focus::SidePanel;
                }
                self.side_opened();
            }
            None => {}
        }
    }

    /// The picked files (relative to the work tree, each once) when the selected row is one
    /// of two or more: what its buttons and the stage / discard commands act on.
    pub(super) fn scm_picked_files(&self) -> Option<Vec<PathBuf>> {
        let status = self.git.status.as_ref()?;
        let rows = self.scm.rows(status);
        let key = self.scm.selected(&rows)?.key();
        let picked = self.scm.picked(&rows);
        if !self.scm.is_picked(&key) || picked.len() < 2 {
            return None;
        }
        let mut files: Vec<PathBuf> = picked
            .iter()
            .filter_map(|r| match r {
                Row::File { entry, .. } => Some(entry.path.clone()),
                Row::Header { .. } => None,
            })
            .collect();
        files.sort();
        files.dedup();
        Some(files)
    }

    /// `git.stageFile` / `git.unstageFile` while the view has the keys: the selected file (a
    /// header: its whole section, as Stage All / Unstage All; one of the picked files: all of
    /// them that can be — conflicted ones are staged on their own, after the markers check).
    pub(super) fn scm_stage(&mut self, unstage: bool) {
        let Some(status) = self.git.status.as_ref() else {
            self.notify(Level::Info, "Not in a git repository".into());
            return;
        };
        if let Some(files) = self.scm_picked_files() {
            return match unstage {
                true => self.unstage_files(files),
                false => self.stage_files(files, false),
            };
        }
        let rows = self.scm.rows(status);
        match self.scm.selected(&rows) {
            Some(Row::Header { .. }) if unstage => self.unstage_all(),
            Some(Row::Header { .. }) => self.stage_all(),
            Some(Row::File { entry, .. }) => {
                let path = status.repo.workdir.join(&entry.path);
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                match (unstage, entry.staged.is_some(), entry.unstaged.is_some()) {
                    (true, false, _) => self.notify(Level::Info, format!("{name} has no staged changes")),
                    (false, _, false) => self.notify(Level::Info, format!("{name} has no changes to stage")),
                    _ => self.stage_path(path, unstage),
                }
            }
            None => {}
        }
    }

    /// `git.stageAll`: every file in Changes (untracked as added, deleted as removed), from
    /// disk. Dirty buffers among them are saved first (one prompt), saves in flight waited for.
    /// Conflicted files are in Merge Changes, not here: each is staged (resolved) on its own.
    pub(super) fn stage_all(&mut self) {
        let Some(status) = self.git.status.as_ref() else {
            self.notify(Level::Info, "Not in a git repository".into());
            return;
        };
        let files: Vec<PathBuf> = scm::files(status, Section::Changes)
            .map(|(e, _)| e.path.clone())
            .collect();
        self.stage_files(files, true);
    }

    /// Stages `files` (relative to the work tree) as one job, those of them still in Changes:
    /// Stage All's (`all`) or the picked files'. Dirty buffers among them are saved first (one
    /// prompt), saves in flight waited for.
    fn stage_files(&mut self, files: Vec<PathBuf>, all: bool) {
        let Some(status) = self.git.status.as_ref() else {
            self.notify(Level::Info, "Not in a git repository".into());
            return;
        };
        let repo = status.repo.clone();
        let files: Vec<PathBuf> = files
            .into_iter()
            .filter(|rel| status.entry(rel).is_some_and(|e| Section::Changes.change(e).is_some()))
            .collect();
        if files.is_empty() {
            self.notify(Level::Info, "No changes to stage".into());
            return;
        }
        let (mut dirty, mut saving) = (Vec::new(), HashSet::new());
        for rel in &files {
            let Some(id) = self.buffer_of(&repo.workdir.join(rel)) else {
                continue;
            };
            let buf = self.buffers.get(id).unwrap();
            if buf.save_in_flight() {
                saving.insert(id);
            } else if buf.is_dirty() && !buf.deleted {
                // (A deleted file's tab isn't saved: its deletion is staged.)
                dirty.push(id);
            }
        }
        if !dirty.is_empty() {
            self.prompt = Some(Prompt::SaveBeforeStageAll {
                buffers: dirty,
                files,
                all,
            });
            return;
        }
        if !saving.is_empty() {
            self.stage_all_after_save = Some((saving, files));
            self.notify(Level::Info, "Waiting for the saves to finish…".into());
            return;
        }
        let done = match files.len() {
            1 => format!("Staged {}", files[0].display()),
            n => format!("Staged {n} files"),
        };
        self.spawn_diff_stage(done, "Staging failed", move || stage::stage_files(&repo, &files));
    }

    /// The Stage All prompt's Save: saves each, then stages `files` once every save is done.
    pub(super) fn save_then_stage_all(&mut self, buffers: Vec<BufferId>, files: Vec<PathBuf>) {
        let mut waiting = HashSet::new();
        for id in buffers {
            self.save(id);
            if !self.buffers.get(id).is_some_and(|b| b.save_in_flight()) {
                // Refused (read-only): said so; nothing is staged.
                self.stage_all_after_save = None;
                return;
            }
            waiting.insert(id);
        }
        self.stage_all_after_save = Some((waiting, files));
    }

    /// A save ended (`save_done`): Stage All waiting for it goes ahead once the last one is
    /// done (checking again), or is dropped when one failed.
    pub(super) fn stage_all_save_done(&mut self, id: BufferId, saved: bool) {
        let Some((waiting, _)) = &mut self.stage_all_after_save else {
            return;
        };
        if !waiting.remove(&id) {
            return;
        }
        if !saved {
            tracing::info!("not staging all: a save didn't happen");
            self.stage_all_after_save = None;
        } else if waiting.is_empty()
            && let Some((_, files)) = self.stage_all_after_save.take()
        {
            self.stage_files(files, false);
        }
    }

    /// `git.unstageAll`: every staged file back to HEAD's (no save needed).
    pub(super) fn unstage_all(&mut self) {
        let Some(status) = self.git.status.as_ref() else {
            self.notify(Level::Info, "Not in a git repository".into());
            return;
        };
        let files: Vec<PathBuf> = scm::files(status, Section::Staged)
            .map(|(e, _)| e.path.clone())
            .collect();
        self.unstage_files(files);
    }

    /// Unstages `files` (relative to the work tree) as one job, those of them with staged
    /// changes.
    fn unstage_files(&mut self, files: Vec<PathBuf>) {
        let Some(status) = self.git.status.as_ref() else {
            return;
        };
        let repo = status.repo.clone();
        let files: Vec<PathBuf> = files
            .into_iter()
            .filter(|rel| status.entry(rel).is_some_and(|e| e.staged.is_some()))
            .collect();
        if files.is_empty() {
            self.notify(Level::Info, "No staged changes".into());
            return;
        }
        let done = match files.len() {
            1 => format!("Unstaged {}", files[0].display()),
            n => format!("Unstaged {n} files"),
        };
        self.spawn_diff_stage(done, "Unstaging failed", move || stage::unstage_files(&repo, &files));
    }

    /// `git.refresh` (the view's `↻`): the git status read again whatever the file watcher saw
    /// (a full walk, finding a repository made since too), and every open file and hex tab
    /// checked against disk (reloaded, or the changed-on-disk banner over unsaved edits).
    pub(super) fn scm_refresh(&mut self) {
        self.refresh_git(None);
        self.check_open_files(|_| true);
    }

    /// A click in the view: a button stages / unstages (or discards) its row — all the picked
    /// files when it's one of them; a header collapses / expands; a file is selected and its
    /// diff opens with the keys staying in the list (a double click focuses the editor).
    /// Ctrl+click (Cmd+click: mouse reports carry Ctrl, rarely Cmd) picks a file or unpicks it,
    /// Shift+click picks the files from the last one clicked; neither opens a diff. The view
    /// takes the keys.
    pub(super) fn scm_click(&mut self, m: MouseEvent, count: u8) {
        self.ui.focus = Focus::SidePanel;
        self.branch_history.set_focused(false);
        let Some(hit) = self.hits.scm.clone() else { return };
        let at = Position { x: m.column, y: m.row };
        if hit.refresh.contains(at) {
            self.execute(crate::command::Command::ScmRefresh);
            return;
        }
        let Some(status) = self.git.status.as_ref() else { return };
        let rows = self.scm.rows(status);
        if let Some((_, key)) = hit.discards.iter().find(|(r, _)| r.contains(at)) {
            if self.scm.focus_key(&rows, key) {
                self.scm_discard();
            }
            return;
        }
        if let Some((_, key, unstage)) = hit.buttons.iter().find(|(r, ..)| r.contains(at)) {
            if self.scm.focus_key(&rows, key) {
                self.scm_stage(*unstage);
            }
            return;
        }
        if !hit.list.contains(at) {
            return;
        }
        let Some(key) = hit.rows.get((at.y - hit.list.y) as usize) else {
            return;
        };
        if matches!(key, RowKey::File(..)) {
            if m.modifiers.contains(KeyModifiers::SHIFT) {
                self.scm.pick_range(&rows, key);
                return;
            }
            if m.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) {
                self.scm.toggle_pick(&rows, key);
                return;
            }
        }
        if !self.scm.select_key(&rows, key) {
            return;
        }
        match key {
            RowKey::Header(section) => self.scm.toggle(*section),
            RowKey::File(..) => self.scm_open(count >= 2),
        }
    }

    /// A right-click in the view: on a Merge Changes file, its row is selected, the view takes the
    /// keys and the menu to resolve it using ours / theirs opens below the pointer. Elsewhere
    /// nothing.
    pub(super) fn scm_menu(&mut self, at: (u16, u16)) {
        let Some(hit) = self.hits.scm.clone() else { return };
        let Some(status) = self.git.status.as_ref() else { return };
        let p = Position { x: at.0, y: at.1 };
        if !hit.list.contains(p) {
            return;
        }
        let Some(key @ RowKey::File(Section::MergeChanges, _)) = hit.rows.get((p.y - hit.list.y) as usize) else {
            return;
        };
        let rows = self.scm.rows(status);
        if !self.scm.select_key(&rows, key) {
            return;
        }
        self.ui.focus = Focus::SidePanel;
        self.branch_history.set_focused(false);
        self.open_context_menu(
            (at.0, at.1.saturating_add(1)),
            super::context_menu::menu_for(super::context_menu::MenuTarget::Conflict),
        );
    }

    /// The mouse wheel over the view scrolls its list.
    pub(super) fn scm_scroll(&mut self, delta: isize) {
        if let Some(status) = self.git.status.as_ref() {
            let rows = self.scm.rows(status);
            self.scm.scroll_by(&rows, delta);
        }
    }

    /// The pointer moved: the row under it (its buttons are drawn), if over the view's list.
    pub(super) fn scm_hover(&mut self, m: &MouseEvent, hit: Hit) {
        let over = match (&self.hits.scm, hit) {
            (Some(h), Hit::SidePanel) if h.list.contains(Position { x: m.column, y: m.row }) => {
                h.rows.get((m.row - h.list.y) as usize).cloned()
            }
            _ => None,
        };
        if over != self.scm.hover {
            self.scm.hover = over;
            self.needs_redraw = true;
        }
    }
}
