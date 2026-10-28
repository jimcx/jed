//! Diff tabs in the app: opening them (Git: Open Changes, the changed-on-disk banner's Show
//! Diff; the Source Control panel calls `open_diff`; Compare Current Conflict opens a conflict's
//! blocks), loading both sides off-thread — again when
//! the index or the file changes, and `diff_view::DELAY` after edits to an open buffer shown on
//! a live side — the keys and the mouse in them, and staging / unstaging from them. Staging works
//! as the gutter's (`app/quick_diff.rs`): a hunk of the working tree is staged from the file on
//! disk, after saving a dirty buffer (asking) and waiting for a save in flight. Reverting a
//! change of an unstaged view edits the open buffer (one undo step), else the file on disk.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::files::Goto;
use super::mouse::{Drag, wheel_columns};
use super::quick_diff::StageAt;
use super::{App, AppEvent, Focus, Level, Prompt};
use crate::buffer::BufferId;
use crate::buffer::change::ChangeSet;
use crate::buffer::pos;
use crate::command::{Command, Motion};
use crate::diff_view::{self, DiffTab, Doc, Job, Pair, Source};
use crate::git::stage::{self, Side};
use crate::git::{Change, Repo, diff};
use crate::group::Tab;
use crate::layout::Hit;
use crate::palette::{self, Picker};
use crate::quick_diff::Want;
use crate::syntax;

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
    /// A key waiting for its file to open (`app/diff_edit.rs`).
    pub(super) pending: Option<super::diff_edit::PendingEdit>,
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

    /// Compares two texts in a diff tab of the active group (a file, else an open buffer, saved or
    /// not), or shows the group's tab already comparing them, and focuses it. Comparing a text with
    /// itself says so and opens nothing. The tab is transient: sessions don't keep it.
    pub fn open_compare(&mut self, left: Source, right: Source) {
        let key = |app: &App, s: &Source| match s {
            Source::Open(id) => match app.buffers.get(*id) {
                Some(b) if b.has_file() => Source::File(b.path.clone()),
                _ => s.clone(),
            },
            Source::File(_) => s.clone(),
        };
        if key(self, &left) == key(self, &right) {
            return self.notify(Level::Info, "Cannot compare a file with itself".into());
        }
        // The right side's file names the tab for the syntax colours; an untitled buffer its title.
        let path = match &right {
            Source::File(p) => p.clone(),
            Source::Open(id) => match self.buffers.get(*id) {
                Some(b) if b.has_file() => b.path.clone(),
                Some(b) => PathBuf::from(b.name()),
                None => PathBuf::new(),
            },
        };
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let existing = self.group().tabs.iter().position(|t| {
            t.diff()
                .and_then(|d| d.compare.as_ref())
                .is_some_and(|c| c.sides[0].source == left && c.sides[1].source == right)
        });
        match existing {
            Some(i) => self.group_mut().active = i,
            None => {
                let id = self.next_diff_id();
                self.group_mut()
                    .open_tab(Tab::Diff(Box::new(DiffTab::compare(id, path, left, right))));
            }
        }
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
        self.sync_diffs();
    }

    /// "File: Compare Active File With…": a picker of the other open texts, then the workspace's
    /// files (none in single file mode); the pick is the left side, the active buffer the right.
    pub(super) fn compare_active_file_with(&mut self) {
        let Some(right) = self
            .group()
            .active_tab()
            .map(|t| t.buffer)
            .filter(|&id| self.comparable(id))
        else {
            return self.notify(Level::Info, "Open a file or an untitled buffer to compare".into());
        };
        if !self.mode.allows(Command::QuickOpen) && self.compare_texts(Some(right)).is_empty() {
            return self.notify(Level::Info, "Open two files to compare them".into());
        }
        self.open_palette(Picker::Compare { right }, "");
    }

    /// "File: Compare Open Editors…": picks two open texts in turn (the left, then the right).
    pub(super) fn compare_open_editors(&mut self) {
        if self.compare_texts(None).len() < 2 {
            return self.notify(Level::Info, "Open two files to compare them".into());
        }
        self.open_palette(Picker::CompareEditors { first: None }, "");
    }

    /// Whether buffer `id` is a text a compare tab can name: a file's or an untitled buffer's.
    fn comparable(&self, id: BufferId) -> bool {
        self.buffers.get(id).is_some_and(|b| b.has_file() || b.is_untitled())
    }

    /// The open texts the compare pickers offer except `skip`, by buffer id.
    fn compare_texts(&self, skip: Option<BufferId>) -> Vec<palette::OpenText> {
        let mut texts: Vec<palette::OpenText> = self
            .buffers
            .iter()
            .filter(|(id, b)| Some(*id) != skip && (b.has_file() || b.is_untitled()))
            .map(|(id, b)| {
                let folder = if b.is_untitled() {
                    "untitled".to_string()
                } else {
                    let dir = b.path.parent().unwrap_or(Path::new(""));
                    dir.strip_prefix(&self.workspace.root)
                        .unwrap_or(dir)
                        .display()
                        .to_string()
                };
                palette::OpenText {
                    id,
                    name: b.name(),
                    folder,
                    dirty: b.is_dirty(),
                }
            })
            .collect();
        texts.sort_by_key(|t| t.id);
        texts
    }

    /// The compare pickers' open-text rows for `query` (the picked first text and the active
    /// buffer left out).
    pub(super) fn compare_buffer_rows(&self, query: &str) -> Vec<palette::Row> {
        let skip = match self.palette.as_ref().map(|p| p.picker) {
            Some(Picker::Compare { right }) => Some(right),
            Some(Picker::CompareEditors { first }) => first,
            _ => None,
        };
        palette::compare_rows(query, &self.compare_texts(skip))
    }

    /// The compare pickers' hint above the rows.
    pub(super) fn compare_hint(&self, picker: Picker) -> String {
        let name = |id: BufferId| self.buffers.get(id).map(|b| b.name()).unwrap_or_default();
        match picker {
            Picker::Compare { right } => format!("Select a file or editor to compare with {}", name(right)),
            Picker::CompareEditors { first: Some(first) } => {
                format!("Select the second editor to compare with {}", name(first))
            }
            _ => "Select the first editor to compare".into(),
        }
    }

    /// A compare picker's pick: an open text (`open`) or a file (workspace-relative `rel`).
    pub(super) fn compare_picked(&mut self, picker: Picker, open: Option<BufferId>, rel: Option<PathBuf>) {
        let picked = match (open, rel) {
            (Some(id), _) => Source::Open(id),
            (None, Some(rel)) => Source::File(self.workspace.root.join(rel)),
            (None, None) => return,
        };
        match picker {
            Picker::Compare { right } => self.open_compare(picked, Source::Open(right)),
            Picker::CompareEditors { first: None } => {
                if let Source::Open(id) = picked {
                    self.open_palette(Picker::CompareEditors { first: Some(id) }, "");
                }
            }
            Picker::CompareEditors { first: Some(first) } => self.open_compare(Source::Open(first), picked),
            _ => {}
        }
    }

    /// Opens a merge conflict's current and incoming blocks in a new read-only diff tab
    /// (Compare Current Conflict). Each conflict of a file has the same `Pair::conflict()`, so
    /// this never reuses a tab: the blocks are the tab's own bytes, not the file's.
    pub(super) fn open_conflict_diff(&mut self, path: PathBuf, current: Vec<u8>, incoming: Vec<u8>) {
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let id = self.next_diff_id();
        self.group_mut()
            .open_tab(Tab::Diff(Box::new(DiffTab::conflict(id, path, current, incoming))));
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
        self.sync_diffs();
    }

    /// The file the active tab is about: a text tab's buffer's, a diff or hex tab's (`None` for
    /// a log tab: no file).
    pub(super) fn active_path(&self) -> Option<PathBuf> {
        if let Some(d) = self.group().active_diff() {
            return Some(d.path.clone());
        }
        if let Some(h) = self.group().active_hex() {
            return Some(h.path.clone());
        }
        self.active_buffer().filter(|b| b.has_file()).map(|b| b.path.clone())
    }

    /// The buffer open on `path`.
    pub(super) fn buffer_of(&self, path: &Path) -> Option<BufferId> {
        self.buffers
            .iter()
            .find(|(_, b)| b.has_file() && b.path == path)
            .map(|(id, _)| id)
    }

    /// The buffer the active tab is about: a text tab's, or the open buffer of a diff tab's file
    /// (what Save, Reload and Keep Mine act on); `None` for a log tab (no file) and a hex tab
    /// (no buffer).
    pub(super) fn active_file_buffer(&self) -> Option<BufferId> {
        match self.group().active_diff() {
            Some(d) => self.buffer_of(&d.path),
            None => self
                .group()
                .active_tab()
                .map(|t| t.buffer)
                .filter(|&id| self.buffers.get(id).is_some_and(|b| b.has_file())),
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
        self.ensure_status_loaded();
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

    /// `git.toggleChanges`: from a diff tab its file, else the active file's diff.
    pub(super) fn toggle_changes(&mut self) {
        if self.group().active_diff().is_some() {
            self.open_diff_source();
            return;
        }
        let Some(path) = self.active_path() else { return };
        let Some(line) = self
            .active_buffer()
            .zip(self.group().active_tab())
            .map(|(b, t)| pos::line_of(&b.rope, t.view.selections.primary().head))
        else {
            return;
        };
        // The group's diff tab of the file, if it has one; else the file's changes.
        let pair = self
            .group()
            .tabs
            .iter()
            .find_map(|t| t.diff().filter(|d| d.path == path).map(|d| d.pair));
        match pair {
            Some(pair) => self.open_diff(path.clone(), pair),
            None => self.open_changes(),
        }
        if let Some(d) = self.group_mut().active_diff_mut()
            && d.path == path
        {
            d.go_to_line(line);
            self.needs_redraw = true;
        }
    }

    /// The diff header's Open File: the active diff tab's file, the cursor on the diff's line
    /// (kept where it is when the group's tab of the file already has it on that line).
    pub(super) fn open_diff_source(&mut self) {
        let Some((path, line)) = (self.group().active_diff())
            .filter(|d| d.compare.is_none())
            .map(|d| (d.path.clone(), d.cursor_line()))
        else {
            return;
        };
        let on_line = self.buffers.find_path(&path).is_some_and(|id| {
            let (Some(buf), Some(i)) = (self.buffers.get(id), self.group().find(id)) else {
                return false;
            };
            let view = &self.group().tabs[i].text().unwrap().view;
            pos::line_of(&buf.rope, view.selections.primary().head) == line
        });
        let goto = (!on_line).then_some(Goto::Cursor(line + 1, 1));
        self.open_file(path, true, goto);
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
            let sides = compare_buffers(buffers, d);
            let open = match sides {
                // A compare's language is its right side's.
                Some([_, right]) => right.and_then(|id| buffers.get(id).map(|b| (id, b))),
                None => buffers.iter().find(|(_, b)| b.path == d.path),
            };
            let mut live = [open.map(|(id, b)| (id, b.revision())), None];
            if let Some(sides) = sides {
                let buf = |id: Option<BufferId>| id.and_then(|id| buffers.get(id).map(|b| (id, b)));
                let now = sides.map(|id| {
                    buf(id).map(|(_, b)| {
                        let dirty = b.is_dirty();
                        (b.name(), dirty, b.rope.clone(), b.bom)
                    })
                });
                if d.sync_sides(now) {
                    self.needs_redraw = true;
                }
                live = sides.map(|id| buf(id).map(|(id, b)| (id, b.revision())));
            }
            match d.want_sides(live) {
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
            let generation = d.begin_sides(live);
            let job = Job {
                repo: repo.clone(),
                root: self.workspace.root.clone(),
                path: d.path.clone(),
                pair: d.pair,
                live: open.map(|(_, b)| (b.rope.clone(), b.bom)),
                compare: d.compare_reads(live.map(|l| l.is_some())),
                // The open buffer's language; else its path's, its text's shebang off-thread.
                language: match open {
                    Some((_, b)) => Some(b.syntax.language()),
                    None => syntax::detect_path(&d.path, &self.workspace.root, &self.associations),
                },
                syntax_max_bytes: self.settings.syntax_max_bytes,
                replace: d.replace.clone(),
                conflict: d.conflict.clone(),
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

    /// Files changed on disk: diff tabs of them (or of files inside them; a compare's either
    /// file) read them again.
    pub(super) fn diffs_paths_changed(&mut self, paths: &[PathBuf]) {
        for (_, d) in self.groups.diff_tabs_mut() {
            let hit = |f: &Path| paths.iter().any(|p| f.starts_with(p));
            let compared = (d.compare.as_ref())
                .is_some_and(|c| c.sides.iter().any(|s| matches!(&s.source, Source::File(f) if hit(f))));
            if hit(&d.path) || compared {
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
        let page = d.height.saturating_sub(1).max(1);
        let mut none = false;
        let mut editing = false;
        match cmd {
            Command::Move { motion, select } => {
                d.caret_move(motion, select, page);
                match motion {
                    Motion::PageUp => d.scroll_by(-(page as isize)),
                    Motion::PageDown => d.scroll_by(page as isize),
                    _ => {}
                }
            }
            Command::SelectAll => d.select_all(),
            Command::CancelSelection => {
                d.clear_selection();
            }
            Command::NextChange | Command::PreviousChange => {
                none = d.doc.is_some() && !d.step(cmd == Command::NextChange);
            }
            _ if crate::edit::is_edit(cmd) => editing = true,
            _ => tracing::debug!(command = %cmd.name(), "not in a diff view"),
        }
        if editing {
            self.diff_edit(super::diff_edit::DiffEdit::Command(cmd));
        }
        if none {
            self.notify(Level::Info, "No changes".into());
        }
        self.needs_redraw = true;
        true
    }

    /// Cmd+C (Cmd+X: `cut`, an edit of an editable side (`diff_edit`)) on a diff
    /// tab: the selection of either side, nothing when it is empty. False when the active tab
    /// isn't a diff.
    pub(super) fn diff_copy(&mut self, cut: bool) -> bool {
        if cut && self.group().active_diff().is_some() {
            self.diff_edit(super::diff_edit::DiffEdit::Cut);
            return true;
        }
        let Some(d) = self.group_mut().active_diff_mut() else {
            return false;
        };
        if let Some(text) = d.selection_text() {
            self.clipboard.write(crate::clipboard::Copied {
                text,
                whole_line: false,
                pieces: Vec::new(),
            });
        }
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
        let k = hunk.or_else(|| d.hunk_at_cursor());
        self.ensure_status_loaded();
        let Some(k) = k else {
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

    /// `diff.applyChangeToRight` / `ToLeft` on a compare tab (or a hunk's arrow button: `hunk`):
    /// the change at the cursor moved to the other side: the source side's lines replace the
    /// hunk's range in the target side's buffer, one undo step, left unsaved; the diff then
    /// re-aligns. Refused (a notification, nothing edited) for a target that is read-only, whose
    /// buffer is closed or whose file isn't open, or when either side's text isn't what the diff shows.
    pub(super) fn diff_apply(&mut self, to_right: bool, hunk: Option<usize>) {
        let Some(d) = self.group().active_diff().filter(|d| d.compare.is_some()) else {
            let text = "Compare two files first: File: Compare Active File With…";
            return self.notify(Level::Info, text.into());
        };
        let Some(k) = hunk.or_else(|| d.hunk_at_cursor()) else {
            if d.doc.is_some() {
                self.notify(Level::Info, "No change at the cursor".into());
            }
            return;
        };
        let Some(doc) = d.doc.clone() else { return };
        let Some(c) = &d.compare else { return };
        let side = &c.sides[usize::from(to_right)];
        let name = if side.name.is_empty() {
            "the file".to_string()
        } else {
            side.name.clone()
        };
        let refuse = |why: &str| format!("Can't move the change into {name}: {why}");
        let buffers = compare_buffers(&self.buffers, d).unwrap_or_default();
        // Both sides as the diff shows them (a second move before the diff re-aligns would move
        // a stale change).
        let stale = buffers.iter().zip([&doc.shown.old, &doc.shown.new]).any(|(b, shown)| {
            b.and_then(|b| self.buffers.get(b))
                .is_some_and(|b| crate::buffer::file::encode(&b.rope, b.bom) != *shown)
        });
        let Some(buffer) = buffers[usize::from(to_right)] else {
            let why = match side.source {
                Source::File(_) => "open the file first (File: Open)",
                Source::Open(_) => "its buffer is closed",
            };
            return self.notify(Level::Info, refuse(why));
        };
        let buf = self.buffers.get(buffer).expect("an open buffer");
        if let Some(reason) = &buf.read_only {
            return self.notify(Level::Error, format!("Read-only: {reason}"));
        }
        if stale {
            return self.notify(Level::Error, refuse("the text changed, try again"));
        }
        let Some((start, end, text)) = diff_view::hunk_edit(&doc.shown, k, to_right, buf.bom) else {
            return self.notify(Level::Error, refuse("the other side's text isn't UTF-8"));
        };
        let mut changes = ChangeSet::with_capacity(1, text.len());
        changes.push(start, end, &text);
        self.apply_changes(buffer, changes);
        self.follow_edits();
        self.notify(Level::Info, format!("Moved the change into {name} (not saved)"));
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
    /// for), else — also a deleted file's open tab — off-thread at once. Staging a conflicted
    /// file resolves it, so a still-open buffer holding markers asks first (VS Code's "stage as
    /// resolved" question): `Prompt::ConfirmStageConflict`.
    pub(super) fn stage_path(&mut self, path: PathBuf, unstage: bool) {
        if self.repo_path(&path).is_none() {
            return;
        }
        if !unstage
            && let Some(buffer) = self.buffer_of(&path)
            && let Some(buf) = self.buffers.get_mut(buffer)
            && !buf.deleted
        {
            let revision = buf.revision();
            if !buf.conflicts.scan(&buf.rope, revision).is_empty() {
                self.prompt = Some(Prompt::ConfirmStageConflict { path });
                return;
            }
        }
        self.stage_path_now(path, unstage);
    }

    /// [`Self::stage_path`] without the markers question (the prompt's "Stage anyway").
    pub(super) fn stage_path_now(&mut self, path: PathBuf, unstage: bool) {
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
        if let (MouseEventKind::Drag(MouseButton::Left), Some(Drag::Diff(g))) = (&m.kind, &self.drag) {
            let g = *g;
            let rect = self.hits.group(g).and_then(|h| h.diff.clone());
            if let (Some(h), Some(d)) = (rect, self.groups.get_mut(g).and_then(|g| g.active_diff_mut())) {
                // Rows above / below the view clamp to its first / last; the caret's side only.
                let row = (m.row.clamp(h.rows.y, (h.rows.y + h.rows.height).saturating_sub(1)) - h.rows.y) as usize;
                // Columns past the side's text (the divider, the other side) clamp to its edge: no
                // sideways scroll while dragging.
                let right = d.caret.right;
                let col = diff_col(d, &h, right, m.column).min(d.left_col + d.text_width);
                d.caret_click(d.top + row, right, col, true);
            }
            return true;
        }
        let Hit::Editor(g) = hit else { return false };
        let Some(d) = self.groups.get_mut(g).and_then(|g| g.active_diff_mut()) else {
            return false;
        };
        if let Some(delta) = wheel_columns(m) {
            d.scroll_columns(delta);
            return true;
        }
        match m.kind {
            MouseEventKind::ScrollUp => d.scroll_by(-WHEEL_ROWS),
            MouseEventKind::ScrollDown => d.scroll_by(WHEEL_ROWS),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(h) = self.hits.group(g).and_then(|h| h.diff.clone()) else {
                    return true;
                };
                let p = ratatui::layout::Position { x: m.column, y: m.row };
                if h.open_file.is_some_and(|r| r.contains(p)) {
                    self.focus_group(g);
                    self.open_diff_source();
                    return true;
                }
                let button = h.buttons.iter().find(|(r, _)| r.contains(p)).map(|(_, k)| *k);
                let revert = h.reverts.iter().find(|(r, _)| r.contains(p)).map(|(_, k)| *k);
                let apply = h
                    .moves
                    .iter()
                    .find(|(r, _, _)| r.contains(p))
                    .map(|(_, k, right)| (*k, *right));
                let on_action = button.is_some() || revert.is_some() || apply.is_some();
                if h.rows.contains(p) {
                    let row = d.top + (m.row - h.rows.y) as usize;
                    if row < d.rows().len() {
                        if on_action {
                            d.cursor = row;
                        } else {
                            let right = !d.inline && h.divider.is_some_and(|x| m.column > x);
                            let col = diff_col(d, &h, right, m.column);
                            let extend = m.modifiers.contains(KeyModifiers::SHIFT);
                            d.caret_click(row, right, col, extend);
                            let count = self.clicks.click(Instant::now(), m.column, m.row);
                            if let Some(d) = self.groups.get_mut(g).and_then(|g| g.active_diff_mut()) {
                                match count {
                                    2 => d.select_word(),
                                    3 => d.select_line(),
                                    _ => {}
                                }
                            }
                            self.drag = Some(Drag::Diff(g));
                        }
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
                } else if let Some((k, to_right)) = apply {
                    self.diff_apply(to_right, Some(k));
                }
            }
            // A divider or panel border dragged over the diff, a release, a move: not its.
            _ => return false,
        }
        true
    }
}

/// The display column of the text under screen column `x` on side `right` (0 left of the text).
fn diff_col(d: &DiffTab, h: &crate::layout::DiffHit, right: bool, x: u16) -> usize {
    x.saturating_sub(h.text_x[right as usize]) as usize + d.left_col
}

/// The open buffer of each side of a compare tab (`None`: not a compare; per side `None`: the
/// file isn't open, or the buffer closed).
fn compare_buffers(buffers: &crate::buffer::Buffers, d: &DiffTab) -> Option<[Option<BufferId>; 2]> {
    d.compare.as_ref().map(|c| {
        c.sides.each_ref().map(|s| match &s.source {
            Source::File(p) => buffers.find_path(p),
            Source::Open(id) => buffers.get(*id).map(|_| *id),
        })
    })
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
                    let rev = |b: BufferId| (b, self.buffers.get(b).unwrap().revision());
                    let live = match compare_buffers(&self.buffers, d) {
                        Some(sides) => sides.map(|b| b.map(rev)),
                        None => [self.buffer_of(&d.path).map(rev), None],
                    };
                    current &= d.doc.is_some() && !d.loading() && d.want_sides(live) == Want::Nothing;
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
