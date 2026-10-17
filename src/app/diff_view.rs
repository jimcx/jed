//! Diff tabs in the app: opening them (Git: Open Changes, the changed-on-disk banner's Show
//! Diff; the Source Control panel calls `open_diff`), loading both sides off-thread — again when
//! the index or the file changes, and `diff_view::DELAY` after edits to an open buffer shown on
//! a live side — the keys and the mouse in them, and staging / unstaging from them. Staging works
//! as the gutter's (`app/quick_diff.rs`): a hunk of the working tree is staged from the file on
//! disk, after saving a dirty buffer (asking) and waiting for a save in flight. Reverting a
//! change of an unstaged view edits the open buffer (one undo step), else the file on disk.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::quick_diff::StageAt;
use super::{App, AppEvent, Focus, Level};
use crate::buffer::BufferId;
use crate::buffer::change::ChangeSet;
use crate::command::{Command, Motion};
use crate::diff_view::{self, DiffTab, Doc, Job, Pair};
use crate::git::stage::{self, Side};
use crate::git::{Change, Repo, diff};
use crate::group::Tab;
use crate::layout::Hit;
use crate::quick_diff::Want;

/// Columns the view scrolls sideways per Left / Right.
const SIDE_STEP: isize = 4;
const WHEEL_ROWS: isize = 3;

/// The app's side of diff tabs.
#[derive(Default)]
pub struct DiffViews {
    /// The next diff tab's id.
    next_id: u64,
    /// When diffs of edited buffers are taken again.
    deadline: Option<Instant>,
    /// Stage / unstage tasks started here running (tests wait for them).
    pub staging: usize,
}

impl DiffViews {
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

impl App {
    pub(super) fn next_diff_id(&mut self) -> u64 {
        self.diffs.next_id += 1;
        self.diffs.next_id
    }

    /// Opens the diff of `path` (absolute) comparing `pair` in the active group, or shows the
    /// group's tab already showing it, and focuses it. The Source Control panel's entry point.
    pub fn open_diff(&mut self, path: PathBuf, pair: Pair) {
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let existing = self
            .group()
            .tabs
            .iter()
            .position(|t| t.diff().is_some_and(|d| d.path == path && d.pair == pair));
        match existing {
            Some(i) => self.group_mut().active = i,
            None => {
                let id = self.next_diff_id();
                self.group_mut()
                    .open_tab(Tab::Diff(Box::new(DiffTab::new(id, path, pair))));
            }
        }
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
        self.sync_diffs();
    }

    /// The file the active tab is about: a text tab's buffer's, or a diff tab's.
    pub(super) fn active_path(&self) -> Option<PathBuf> {
        if let Some(d) = self.group().active_diff() {
            return Some(d.path.clone());
        }
        self.active_buffer().map(|b| b.path.clone())
    }

    /// The buffer open on `path`.
    pub(super) fn buffer_of(&self, path: &Path) -> Option<BufferId> {
        self.buffers.iter().find(|(_, b)| b.path == path).map(|(id, _)| id)
    }

    /// The buffer the active tab is about: a text tab's, or the open buffer of a diff tab's file
    /// (what Save, Reload and Keep Mine act on).
    pub(super) fn active_file_buffer(&self) -> Option<BufferId> {
        match self.group().active_diff() {
            Some(d) => self.buffer_of(&d.path),
            None => self.group().active_tab().map(|t| t.buffer),
        }
    }

    /// The repository and `path` in it; `None` (with a message) outside one.
    pub(super) fn repo_path(&mut self, path: &Path) -> Option<(Repo, PathBuf)> {
        let found = self
            .git
            .status
            .as_ref()
            .and_then(|s| Some((s.repo.clone(), s.repo.relative(path)?)));
        if found.is_none() {
            self.notify(Level::Info, "Not in a git repository".into());
        }
        found
    }

    /// `git.openChange`: the active file's unstaged changes, else its staged ones.
    pub(super) fn open_changes(&mut self) {
        let Some(path) = self.active_path() else { return };
        let Some((_, rel)) = self.repo_path(&path) else { return };
        let entry = self.git.status.as_ref().and_then(|s| s.entry(&rel));
        let side = match entry {
            Some(e) if e.unstaged.is_some() => Side::Unstaged,
            Some(e) if e.staged.is_some() => Side::Staged,
            _ => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                self.notify(Level::Info, format!("{name} has no changes"));
                return;
            }
        };
        self.open_diff(path, Pair::git(side));
    }

    /// `file.compareWithSaved` (the banner's Show Diff): the file on disk against the buffer.
    pub(super) fn compare_with_saved(&mut self) {
        let Some(path) = self
            .active_file_buffer()
            .and_then(|id| self.buffers.get(id))
            .map(|b| b.path.clone())
        else {
            return;
        };
        self.open_diff(path, Pair::SAVED);
    }

    /// Each loop turn: starts the loads due (new tabs, a changed index or file, a buffer opened
    /// or closed; edited buffers once the delay passed) and sets the delay's deadline for the
    /// others. Nothing without a runtime.
    pub(super) fn sync_diffs(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let due = self.diffs.deadline.is_some_and(|d| d <= Instant::now());
        if due {
            self.diffs.deadline = None;
        }
        let repo = self.git.status.as_ref().map(|s| s.repo.clone());
        let mut later = false;
        let mut jobs = Vec::new();
        let buffers = &self.buffers;
        for (_, d) in self.groups.diff_tabs_mut() {
            let open = buffers.iter().find(|(_, b)| b.path == d.path);
            let live = open.map(|(id, b)| (id, b.revision()));
            match d.want(live) {
                Want::Nothing => continue,
                Want::Later if !due => {
                    later = true;
                    continue;
                }
                _ => {}
            }
            if d.pair.needs_repo() && repo.is_none() {
                // A repository may turn up (the first status read): `diffs_index_changed`.
                if d.error.is_none() {
                    d.error = Some("Not in a git repository".into());
                    self.needs_redraw = true;
                }
                continue;
            }
            let generation = d.begin(live);
            let job = Job {
                repo: repo.clone(),
                root: self.workspace.root.clone(),
                path: d.path.clone(),
                pair: d.pair,
                live: open.map(|(_, b)| (b.rope.clone(), b.bom)),
            };
            jobs.push((d.id, generation, job));
        }
        if later && self.diffs.deadline.is_none() {
            self.diffs.deadline = Some(Instant::now() + diff_view::DELAY);
        }
        for (id, generation, job) in jobs {
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = Box::new(diff_view::load(job));
                let _ = tx.send(AppEvent::DiffLoaded { id, generation, result });
            });
        }
    }

    /// `AppEvent::DiffLoaded`: the tab's new diff (dropped when it closed or a newer load runs).
    pub(super) fn diff_loaded(&mut self, id: u64, generation: u64, result: Result<Doc, String>) {
        if let Some((_, d)) = self.groups.diff_tabs_mut().find(|(_, d)| d.id == id) {
            d.finish(generation, result);
        }
    }

    /// The git status was read again (the index, HEAD or work tree files may have changed):
    /// every diff tab reads git's sides again.
    pub(super) fn diffs_index_changed(&mut self) {
        for (_, d) in self.groups.diff_tabs_mut() {
            if d.pair.needs_repo() {
                d.mark_stale();
            }
        }
    }

    /// Files changed on disk: diff tabs of them (or of files inside them) read them again.
    pub(super) fn diffs_paths_changed(&mut self, paths: &[PathBuf]) {
        for (_, d) in self.groups.diff_tabs_mut() {
            if paths.iter().any(|p| d.path.starts_with(p)) {
                d.mark_stale();
            }
        }
    }

    /// A command while the active tab is a diff view: moving its cursor (the editor's movement
    /// keys), next / previous change. Other editor commands do nothing there. False when the
    /// active tab isn't a diff.
    pub(super) fn diff_command(&mut self, cmd: Command) -> bool {
        let Some(d) = self.group_mut().active_diff_mut() else {
            return false;
        };
        let page = d.height.saturating_sub(1).max(1) as isize;
        let mut none = false;
        match cmd {
            Command::Move { motion, .. } => match motion {
                Motion::Up => d.move_cursor(-1),
                Motion::Down => d.move_cursor(1),
                Motion::PageUp => {
                    d.move_cursor(-page);
                    d.scroll_by(-page);
                }
                Motion::PageDown => {
                    d.move_cursor(page);
                    d.scroll_by(page);
                }
                Motion::DocStart => d.cursor_to_end(true),
                Motion::DocEnd => d.cursor_to_end(false),
                Motion::Left => d.scroll_columns(-SIDE_STEP),
                Motion::Right => d.scroll_columns(SIDE_STEP),
                Motion::WordLeft => d.scroll_columns(-4 * SIDE_STEP),
                Motion::WordRight => d.scroll_columns(4 * SIDE_STEP),
                Motion::LineStart => d.left_col = 0,
                Motion::LineEnd => {}
            },
            Command::NextChange | Command::PreviousChange => {
                none = d.doc.is_some() && !d.step(cmd == Command::NextChange);
            }
            _ => tracing::debug!(command = %cmd.name(), "not in a diff view"),
        }
        if none {
            self.notify(Level::Info, "No changes".into());
        }
        self.needs_redraw = true;
        true
    }

    /// `git.stageHunk` / `git.unstageHunk` on a diff tab (or its button: `hunk`): stages the
    /// change at the cursor of an unstaged view, unstages one of a staged view.
    pub(super) fn diff_stage(&mut self, unstage: bool, hunk: Option<usize>) {
        let Some(d) = self.group().active_diff() else { return };
        let (path, pair, doc) = (d.path.clone(), d.pair, d.doc.clone());
        let Some(k) = hunk.or_else(|| d.hunk_at_cursor()) else {
            if doc.is_some() {
                self.notify(Level::Info, "No change at the cursor".into());
            }
            return;
        };
        let Some(doc) = doc else { return };
        let side = match (pair.staging(), unstage) {
            (Some(Side::Unstaged), false) => Side::Unstaged,
            (Some(Side::Staged), true) => Side::Staged,
            (Some(Side::Unstaged), true) => return self.notify(Level::Info, "These changes aren't staged".into()),
            (Some(Side::Staged), false) => return self.notify(Level::Info, "These changes are already staged".into()),
            (None, _) => return self.notify(Level::Info, "This diff isn't git's: nothing to stage".into()),
        };
        let Some(h) = doc.hunks.get(k).cloned() else { return };
        let Some((repo, rel)) = self.repo_path(&path) else {
            return;
        };
        if side == Side::Unstaged
            && let Some(buffer) = self.buffer_of(&path)
        {
            // The file is open: as the gutter does, from disk once the buffer is saved. The
            // line the gutter draws the change on.
            let line = if h.new.is_empty() {
                diff::deletion_line(&h)
            } else {
                h.new.start
            };
            self.stage_at(StageAt {
                buffer,
                line,
                file: false,
            });
            return;
        }
        // Exactly the hunk shown (refused if the repository or the file moved on since).
        let line = h.new.start + 1;
        let (done, failed) = match side {
            Side::Unstaged => (format!("Staged the change at line {line}"), "Staging failed"),
            Side::Staged => (format!("Unstaged the change at line {line}"), "Unstaging failed"),
        };
        self.spawn_diff_stage(done, failed, move || match side {
            Side::Unstaged => stage::stage_hunk(&repo, &rel, &doc.shown, k),
            Side::Staged => stage::unstage_hunk(&repo, &rel, &doc.shown, k),
        });
    }

    /// `git.revertHunk` on a diff tab (or its Revert button: `hunk`): the change at the cursor
    /// of an unstaged view (Index ↔ Working Tree) back to the index's lines in the working tree.
    /// An open file's buffer is edited (one undo step, left unsaved), refused when its text
    /// isn't what the diff shows; a file not open is rewritten on disk off-thread
    /// (`stage::revert_hunk`, refused when the file or index moved on), its text before the
    /// revert then kept in the file history (when it has one) so it can be restored; a failed
    /// record is a warning, the revert stands.
    pub(super) fn diff_revert(&mut self, hunk: Option<usize>) {
        let Some(d) = self.group().active_diff() else {
            let text = "Revert a change from its diff: Git: Open Changes";
            return self.notify(Level::Info, text.into());
        };
        let (path, pair, doc) = (d.path.clone(), d.pair, d.doc.clone());
        let Some(k) = hunk.or_else(|| d.hunk_at_cursor()) else {
            if doc.is_some() {
                self.notify(Level::Info, "No change at the cursor".into());
            }
            return;
        };
        if pair != Pair::UNSTAGED {
            let text = "Only working tree changes can be reverted (Index ↔ Working Tree)";
            return self.notify(Level::Info, text.into());
        }
        let Some(doc) = doc else { return };
        let Some(h) = doc.hunks.get(k).cloned() else { return };
        let Some((repo, rel)) = self.repo_path(&path) else {
            return;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let refuse = |why: &str| refused(&name, why);
        let entry = self.git.status.as_ref().and_then(|s| s.entry(&rel));
        if entry.is_some_and(|e| e.unstaged == Some(Change::Untracked)) {
            return self.notify(Level::Info, refuse("it isn't in the index"));
        }
        if entry.is_some_and(|e| e.unstaged == Some(Change::Conflicted) || e.staged == Some(Change::Conflicted)) {
            return self.notify(Level::Info, refuse("it has merge conflicts"));
        }
        let line = h.new.start + 1;
        if let Some(buffer) = self.buffer_of(&path) {
            let buf = self.buffers.get(buffer).expect("an open buffer");
            if let Some(reason) = &buf.read_only {
                let text = format!("Read-only: {reason}");
                return self.notify(Level::Error, text);
            }
            if crate::buffer::file::encode(&buf.rope, buf.bom) != doc.shown.new {
                return self.notify(Level::Error, refuse("the file changed, try again"));
            }
            let Some((start, end, text)) = diff_view::revert_edit(&doc.shown, k, buf.bom) else {
                return self.notify(Level::Error, refuse("the index's text isn't UTF-8"));
            };
            let mut changes = ChangeSet::with_capacity(1, text.len());
            changes.push(start, end, &text);
            self.apply_changes(buffer, changes);
            self.follow_edits();
            self.notify(
                Level::Info,
                format!("Reverted the change at line {line} of {name} (not saved)"),
            );
            return;
        }
        // Not open: the file on disk, exactly the hunk shown.
        let history = self.history_job(&path);
        if history.is_some() {
            self.history_started();
        }
        self.diffs.staging += 1;
        let tx = self.tx.clone();
        let done = format!("Reverted the change at line {line} of {name}");
        tokio::task::spawn_blocking(move || {
            let result = stage::revert_hunk(&repo, &rel, &doc.shown, k);
            if let Some(job) = history {
                let recorded = match &result {
                    Ok(before) => job.record(before),
                    Err(_) => Ok(()),
                };
                super::history::send_done(&tx, recorded);
            }
            let message = match result {
                Ok(_) => Ok(done),
                Err(stage::Error::Stale) => Err(refused(&name, "the file changed, try again")),
                Err(e) => Err(refused(&name, &e.to_string())),
            };
            let _ = tx.send(AppEvent::DiffStaged(message));
        });
    }

    /// `git.stageFile` / `git.unstageFile`: the active file (a text or diff tab's). Staging an
    /// open buffer's file goes through the gutter's path (saved first).
    pub(super) fn stage_file_command(&mut self, unstage: bool) {
        if let Some(path) = self.active_path() {
            self.stage_path(path, unstage);
        }
    }

    /// Stages / unstages the whole file at `path` (absolute; the Source Control view's rows
    /// too): an open buffer's through the gutter's path (saved first, a save in flight waited
    /// for), else — also a deleted file's open tab — off-thread at once.
    pub(super) fn stage_path(&mut self, path: PathBuf, unstage: bool) {
        let Some((repo, rel)) = self.repo_path(&path) else {
            return;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        // A deleted file's tab isn't saved first: its deletion is what's staged.
        if !unstage
            && let Some(buffer) = self.buffer_of(&path)
            && !self.buffers.get(buffer).is_some_and(|b| b.deleted)
        {
            self.stage_at(StageAt {
                buffer,
                line: 0,
                file: true,
            });
        } else if unstage {
            let done = format!("Unstaged {name}");
            self.spawn_diff_stage(done, "Unstaging failed", move || stage::unstage_file(&repo, &rel));
        } else {
            let done = format!("Staged {name}");
            self.spawn_diff_stage(done, "Staging failed", move || stage::stage_file(&repo, &rel));
        }
    }

    pub(super) fn spawn_diff_stage(
        &mut self,
        done: String,
        failed: &'static str,
        work: impl FnOnce() -> Result<(), stage::Error> + Send + 'static,
    ) {
        self.diffs.staging += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let message = match work() {
                Ok(()) => Ok(done),
                Err(stage::Error::Stale) => Err(format!("{failed}: the file changed, try again")),
                Err(e) => Err(format!("{failed}: {e}")),
            };
            let _ = tx.send(AppEvent::DiffStaged(message));
        });
    }

    /// `AppEvent::DiffStaged`: says what happened; the status (and so every diff and gutter)
    /// is read again.
    pub(super) fn diff_staged(&mut self, message: Result<String, String>) {
        self.diffs.staging -= 1;
        match message {
            Ok(m) => self.notify(Level::Info, m),
            Err(e) => self.notify(Level::Error, e),
        }
        self.quick_diff_index_changed();
        self.refresh_git(None);
    }

    /// The mouse over a group's editor area showing a diff tab: the wheel scrolls it (without
    /// focusing), a click focuses it and puts the cursor on that row, or stages / reverts that change.
    /// False when the group shows no diff tab there, and for other events (moves, drags,
    /// releases), which go on to the rest of `handle_mouse`.
    pub(super) fn diff_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        let Hit::Editor(g) = hit else { return false };
        let Some(d) = self.groups.get_mut(g).and_then(|g| g.active_diff_mut()) else {
            return false;
        };
        match m.kind {
            MouseEventKind::ScrollUp => d.scroll_by(-WHEEL_ROWS),
            MouseEventKind::ScrollDown => d.scroll_by(WHEEL_ROWS),
            MouseEventKind::ScrollLeft => d.scroll_columns(-SIDE_STEP),
            MouseEventKind::ScrollRight => d.scroll_columns(SIDE_STEP),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(h) = self.hits.group(g).and_then(|h| h.diff.clone()) else {
                    return true;
                };
                let p = ratatui::layout::Position { x: m.column, y: m.row };
                let button = h.buttons.iter().find(|(r, _)| r.contains(p)).map(|(_, k)| *k);
                let revert = h.reverts.iter().find(|(r, _)| r.contains(p)).map(|(_, k)| *k);
                if h.rows.contains(p) {
                    let row = d.top + (m.row - h.rows.y) as usize;
                    if row < d.rows().len() {
                        d.cursor = row;
                    }
                }
                self.focus_group(g);
                if let Some(k) = button {
                    let unstage = self
                        .group()
                        .active_diff()
                        .is_some_and(|d| d.pair.staging() == Some(Side::Staged));
                    self.diff_stage(unstage, Some(k));
                } else if let Some(k) = revert {
                    self.diff_revert(Some(k));
                }
            }
            // A divider or panel border dragged over the diff, a release, a move: not its.
            _ => return false,
        }
        true
    }
}

/// A revert's refusal.
fn refused(name: &str, why: &str) -> String {
    format!("Can't revert the change in {name}: {why}")
}

#[cfg(test)]
impl App {
    /// Every diff tab loaded and current (nothing running or due), no stage or git status
    /// refresh running.
    pub fn diffs_settled(&self) -> bool {
        let mut current = true;
        for id in self.groups.ids() {
            for t in &self.groups.get(id).unwrap().tabs {
                if let Some(d) = t.diff() {
                    let live = self
                        .buffer_of(&d.path)
                        .map(|b| (b, self.buffers.get(b).unwrap().revision()));
                    current &= d.doc.is_some() && !d.loading() && d.want(live) == Want::Nothing;
                }
            }
        }
        current
            && self.diffs.deadline.is_none()
            && self.diffs.staging == 0
            && self.quick_diff.staging == 0
            && !self.git.running()
    }
}
