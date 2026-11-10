//! Quick diff in the app: keeping each buffer's gutter markers current (computed off-thread,
//! at once for a new buffer or a changed index, `quick_diff::DELAY` after edits), the peek a
//! click on a marker opens, next / previous change, and staging / unstaging the change at a
//! line. Staging works on the file on disk, never the buffer: a dirty buffer is saved first
//! (after asking), a save in flight is waited for, then the change is staged off-thread from a
//! fresh diff of the disk file (`git::stage::stage_line`).

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::{App, AppEvent, Level, Prompt};
use crate::buffer::BufferId;
use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::git::{Change, Hunk, Repo, diff, stage};
use crate::layout::{GroupId, Hit};
use crate::quick_diff::{self, Computed, Job, Mark, Want};
use crate::selection::{Selection, Selections};

/// "1 changed line" / "3 changed lines".
fn changed_lines(n: usize) -> String {
    format!("{n} changed line{}", if n == 1 { "" } else { "s" })
}

/// The lines selections cover, as sorted, merged line ranges: an empty selection is its line, a
/// selection ending at column 0 doesn't take that line. `ranges` are in order (`Selections`);
/// the rope is walked with a `LineWalker`, not looked up per selection.
fn line_ranges(rope: &ropey::Rope, ranges: &[(ByteIdx, ByteIdx)]) -> Vec<Range<usize>> {
    let mut walker = pos::LineWalker::new(rope);
    let mut out: Vec<Range<usize>> = Vec::new();
    for &(from, to) in ranges {
        let first = walker.line_at(from).index;
        let end = if to > from {
            let last = walker.line_at(to);
            if last.start() == to { last.index } else { last.index + 1 }
        } else {
            first + 1
        };
        match out.last_mut() {
            Some(prev) if first <= prev.end => prev.end = prev.end.max(end),
            _ => out.push(first..end),
        }
    }
    out
}

/// Blocking: the edits of `rope` (the buffer's text, `bom`: it has a BOM) that put the index's
/// lines back for the changed lines in `lines` (a line range per selection): the index -> buffer
/// diff, [`diff::Pick::from_new_lines`], [`diff::revert_line_edits`]. A refusal is the reason.
fn plan_revert(
    repo: &Repo,
    path: &std::path::Path,
    rope: &ropey::Rope,
    bom: bool,
    lines: &[Range<usize>],
) -> Result<RevertPlan, String> {
    let Some(index) = stage::index_text(repo, path).map_err(|e| e.to_string())? else {
        return Err("it isn't in the index".into());
    };
    let link = std::fs::symlink_metadata(repo.workdir.join(path)).is_ok_and(|m| m.file_type().is_symlink());
    if index.mode == 0o120000 || link {
        return Err("it is a symbolic link".into());
    }
    let text = crate::buffer::file::encode(rope, bom);
    let found = diff::diff(&index.bytes, &text, 0).map_err(|e| e.to_string())?;
    if found.binary {
        return Err("it is binary".into());
    }
    let pick = diff::Pick::from_new_lines(&found.hunks, lines.iter().cloned());
    let shift = if bom { 3 } else { 0 };
    let mut edits = Vec::new();
    for (range, mut with) in diff::revert_line_edits(&index.bytes, &text, &found.hunks, &pick) {
        // The buffer keeps its own BOM: the index's isn't text.
        if range.start == 0 && with.starts_with(b"\xEF\xBB\xBF") {
            with.drain(..3);
        }
        let with = String::from_utf8(with).map_err(|_| "the index's text isn't UTF-8".to_string())?;
        let (start, end) = (range.start.saturating_sub(shift), range.end.saturating_sub(shift));
        edits.push((ByteIdx(start), ByteIdx(end), with));
    }
    Ok(RevertPlan {
        edits,
        lines: pick.count(&found.hunks),
    })
}

/// The app's side of quick diff.
#[derive(Default)]
pub struct QuickDiffState {
    /// When the markers of edited buffers are computed again.
    deadline: Option<Instant>,
    /// A stage waiting for the buffer's save in flight to end, and the start of its line,
    /// followed through edits made meanwhile (`follow`): the line it stages is that one's.
    after_save: Option<(StageAt, ByteIdx)>,
    /// The selections a `What::Ranges` stage works on (start, end), in order, followed through
    /// edits while it waits (`follow`); taken when it runs.
    ranges: Vec<(ByteIdx, ByteIdx)>,
    /// The peek (a click on a gutter marker), while open.
    pub peek: Option<Peek>,
    /// Stage / unstage tasks running (tests wait for them).
    pub staging: usize,
}

impl QuickDiffState {
    /// Tests: a deadline at `at`, as the module's own code would arm it.
    #[cfg(test)]
    pub fn test_arm(&mut self, at: Instant) {
        self.deadline = Some(at);
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// `buffer`'s change sets applied since the last call (`App::follow_edits`): a stage waiting
    /// for a save keeps to its line.
    pub fn follow(&mut self, buffer: BufferId, changes: &[Arc<ChangeSet>]) {
        if let Some((at, start)) = &mut self.after_save
            && at.buffer == buffer
        {
            for set in changes {
                *start = set.map(*start, Assoc::After);
                let map = set.mapper();
                for (from, to) in &mut self.ranges {
                    (*from, *to) = (map.map(*from, Assoc::After), map.map(*to, Assoc::Before));
                }
            }
        }
    }
}

/// What a stage works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// The change the gutter shows on `StageAt::line`.
    Line,
    /// The whole file (`git.stageFile`).
    File,
    /// The changed lines in the selections (`QuickDiffState::ranges`; `git.stageSelectedRanges`).
    Ranges,
}

/// Which change to stage: `what` of `buffer`, for `Line` the change on `line` (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageAt {
    pub buffer: BufferId,
    pub line: usize,
    pub what: What,
}

/// The peek: one change's removed lines and a Stage button, below the change in `group`.
#[derive(Debug, Clone)]
pub struct Peek {
    pub group: GroupId,
    pub buffer: BufferId,
    /// The buffer's revision when it opened: any edit closes it.
    pub revision: u64,
    /// The lines the change is drawn on.
    pub lines: Range<usize>,
    pub mark: Mark,
}

/// What a revert of selected lines found, off-thread: the edits of the buffer's text (byte
/// ranges, its BOM not counted) and how many changed lines they take back.
pub(super) struct RevertPlan {
    edits: Vec<(ByteIdx, ByteIdx, String)>,
    lines: usize,
}

/// Results of gutter background work (`AppEvent::Gutter`).
pub(super) enum GutterEvent {
    /// A buffer's gutter markers were computed.
    Markers { buffer: BufferId, computed: Computed },
    /// A buffer's inline blame was computed (`app/blame.rs`).
    Blame {
        buffer: BufferId,
        computed: crate::blame::Computed,
    },
    /// Selected lines were staged or unstaged: how many changed lines.
    StagedLines {
        at: StageAt,
        unstage: bool,
        result: Result<usize, String>,
    },
    /// Selected lines' revert was worked out against the buffer text as of `revision`.
    Reverted {
        buffer: BufferId,
        revision: u64,
        result: Result<RevertPlan, String>,
    },
    /// A change was staged or unstaged (or wasn't).
    Staged {
        at: StageAt,
        unstage: bool,
        result: Result<Option<Hunk>, String>,
    },
}

impl App {
    pub(super) fn handle_gutter_event(&mut self, ev: GutterEvent) {
        match ev {
            GutterEvent::Markers { buffer, computed } => self.quick_diff_done(buffer, computed),
            GutterEvent::Blame { buffer, computed } => self.blame_done(buffer, computed),
            GutterEvent::Staged { at, unstage, result } => self.staged(at, unstage, result),
            GutterEvent::StagedLines { at, unstage, result } => self.staged_lines(at, unstage, result),
            GutterEvent::Reverted {
                buffer,
                revision,
                result,
            } => self.reverted(buffer, revision, result),
        }
    }
}

impl App {
    /// Each loop turn: starts the computations due (new buffers, a changed index; edited
    /// buffers once the delay passed) and sets the delay's deadline for the others.
    pub(super) fn sync_quick_diff(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let due = self.quick_diff.deadline.is_some_and(|d| d <= Instant::now());
        if due {
            self.quick_diff.deadline = None;
        }
        let repo = self.git.status.as_ref().map(|s| &s.repo);
        let mut later = false;
        let mut start = Vec::new();
        for (id, buf) in self.buffers.iter_mut() {
            let revision = buf.revision();
            let want = buf.quick_diff.want(revision);
            if want == Want::Nothing {
                continue;
            }
            let Some((repo, path)) = repo.and_then(|r| Some((r, r.relative(&buf.path)?))) else {
                // No repository, or the file is outside it.
                buf.quick_diff.clear(revision);
                self.needs_redraw = true;
                continue;
            };
            if want == Want::Later && !due {
                later = true;
                continue;
            }
            let (generation, known, staged) = buf.quick_diff.begin(revision);
            start.push((
                id,
                Job {
                    repo: repo.clone(),
                    path,
                    rope: buf.rope.clone(),
                    bom: buf.bom,
                    revision,
                    generation,
                    known,
                    staged,
                },
            ));
        }
        if later && self.quick_diff.deadline.is_none() {
            self.quick_diff.deadline = Some(Instant::now() + quick_diff::DELAY);
        }
        for (buffer, job) in start {
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let computed = quick_diff::compute(job);
                let _ = tx.send(AppEvent::Gutter(GutterEvent::Markers { buffer, computed }));
            });
        }
    }

    /// `AppEvent::Gutter(GutterEvent::Markers)`: a buffer's new markers.
    pub(super) fn quick_diff_done(&mut self, buffer: BufferId, computed: Computed) {
        if let Some(buf) = self.buffers.get_mut(buffer) {
            buf.quick_diff.finish(computed);
        }
    }

    /// The index (or the repository) may have changed: every buffer's markers are computed again.
    pub(super) fn quick_diff_index_changed(&mut self) {
        for (_, buf) in self.buffers.iter_mut() {
            buf.quick_diff.index_changed();
        }
    }

    /// HEAD moved (a commit, a checkout, a reset): the markers' staged half (index → HEAD) is
    /// computed again too, even where the index is still the one they came from.
    pub(super) fn quick_diff_head_changed(&mut self) {
        for (_, buf) in self.buffers.iter_mut() {
            buf.quick_diff.head_changed();
        }
    }

    /// `editor.nextChange` / `editor.previousChange`: the cursor to the first line of the next /
    /// previous change in the gutter, wrapping around.
    pub(super) fn go_to_change(&mut self, forward: bool) {
        let Some((buf, view)) = self.active_mut() else { return };
        let line = pos::line_of(&buf.rope, view.selections.primary().head);
        let Some(to) = buf.quick_diff.step(&buf.rope, line, forward) else {
            return;
        };
        buf.seal_undo();
        let at = pos::line_start(&buf.rope, to);
        view.selections = Selections::single(Selection::cursor(at));
        view.reveal_range = Some((at, at));
    }

    /// `git.stageHunk` / `git.unstageHunk`: the change in the peek, else at the active view's
    /// cursor line.
    pub(super) fn stage_command(&mut self, unstage: bool) {
        let active = self
            .group()
            .active_tab()
            .map(|t| (t.buffer, t.view.selections.primary().head));
        let Some((buffer, head)) = active else { return };
        let line = match &self.quick_diff.peek {
            Some(p) if p.buffer == buffer && p.group == self.groups.active => p.lines.start,
            _ => match self.buffers.get(buffer) {
                Some(b) => pos::line_of(&b.rope, head),
                None => return,
            },
        };
        if unstage {
            self.unstage_at(buffer, line);
        } else {
            self.stage_at(StageAt {
                buffer,
                line,
                what: What::Line,
            });
        }
    }

    /// The repository and the buffer's path in it; `None` (with a message) outside one, and
    /// (silently) for a log tab: no file.
    fn git_path(&mut self, buffer: BufferId) -> Option<(Repo, PathBuf)> {
        let path = self.buffers.get(buffer).filter(|b| b.has_file())?.path.clone();
        let found = self
            .git
            .status
            .as_ref()
            .and_then(|s| Some((s.repo.clone(), s.repo.relative(&path)?)));
        if found.is_none() {
            self.notify(Level::Info, "Not in a git repository".into());
        }
        found
    }

    /// Stages the change at `at.line` from the file on disk: a dirty buffer is saved first
    /// (asking; nothing without a marker on the line), a save in flight is waited for.
    pub(super) fn stage_at(&mut self, at: StageAt) {
        let Some((repo, path)) = self.git_path(at.buffer) else {
            return;
        };
        let Some(buf) = self.buffers.get(at.buffer) else { return };
        let dirty = buf.is_dirty();
        if dirty && at.what == What::Line && buf.quick_diff.at_line(&buf.rope, at.line).is_none() {
            self.notify(Level::Info, format!("No change to stage at line {}", at.line + 1));
            return;
        }
        if buf.save_in_flight() {
            let start = self.resume_point(at);
            self.quick_diff.after_save = Some((at, start));
            self.notify(Level::Info, "Waiting for the save to finish…".into());
            return;
        }
        if dirty {
            self.close_peek();
            self.prompt = Some(Prompt::SaveBeforeStage {
                buffer: at.buffer,
                line: at.line,
                what: at.what,
            });
            return;
        }
        match at.what {
            What::File => self.spawn_stage(at, false, move || stage::stage_file(&repo, &path).map(|()| None)),
            What::Line => self.spawn_stage(at, false, move || stage::stage_line(&repo, &path, at.line)),
            What::Ranges => {
                // The buffer is the file now: its lines are the disk's.
                let (rope, ranges) = (buf.rope.clone(), std::mem::take(&mut self.quick_diff.ranges));
                self.spawn_stage_lines(at, false, move || {
                    stage::stage_ranges(&repo, &path, &line_ranges(&rope, &ranges))
                });
            }
        }
    }

    /// Where a stage waiting for a save is kept: the start of its line, or of its first selection.
    fn resume_point(&self, at: StageAt) -> ByteIdx {
        let Some(buf) = self.buffers.get(at.buffer) else {
            return ByteIdx(0);
        };
        match (at.what, self.quick_diff.ranges.first()) {
            (What::Ranges, Some(&(from, _))) => from,
            _ => pos::line_start(&buf.rope, at.line.min(pos::line_count(&buf.rope) - 1)),
        }
    }

    /// The save prompt's Save: saves, then stages once the save is done.
    pub(super) fn save_then_stage(&mut self, at: StageAt) {
        let start = self.resume_point(at);
        self.quick_diff.after_save = Some((at, start));
        self.save(at.buffer);
        if !self.buffers.get(at.buffer).is_some_and(|b| b.save_in_flight()) {
            // Refused (read-only): said so.
            self.quick_diff.after_save = None;
            self.quick_diff.ranges.clear();
        }
    }

    /// A save ended (`save_done`): a stage waiting for it goes ahead at its line as edits made
    /// meanwhile moved it (checking again: such edits ask again), or is dropped when the save
    /// failed or was refused.
    pub(super) fn stage_save_done(&mut self, buffer: BufferId, saved: bool) {
        let Some((at, start)) = self.quick_diff.after_save.filter(|(a, _)| a.buffer == buffer) else {
            return;
        };
        self.quick_diff.after_save = None;
        let Some(buf) = self.buffers.get(buffer) else { return };
        let line = pos::line_of(&buf.rope, start.min(ByteIdx(buf.rope.len())));
        if saved {
            self.stage_at(StageAt { line, ..at });
        } else {
            self.quick_diff.ranges.clear();
            tracing::info!(line = at.line, "not staging: the save didn't happen");
        }
    }

    /// Unstages the staged change at `line` of the buffer (mapped to the index's lines through
    /// the buffer's text, so the buffer needn't be saved).
    fn unstage_at(&mut self, buffer: BufferId, line: usize) {
        let Some((repo, path)) = self.git_path(buffer) else {
            return;
        };
        let Some(buf) = self.buffers.get(buffer) else { return };
        let (rope, bom) = (buf.rope.clone(), buf.bom);
        let at = StageAt {
            buffer,
            line,
            what: What::Line,
        };
        self.spawn_stage(at, true, move || {
            let text = crate::buffer::file::encode(&rope, bom);
            stage::unstage_line(&repo, &path, &text, line)
        });
    }

    fn spawn_stage(
        &mut self,
        at: StageAt,
        unstage: bool,
        work: impl FnOnce() -> Result<Option<Hunk>, stage::Error> + Send + 'static,
    ) {
        self.close_peek();
        self.quick_diff.staging += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = work().map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::Gutter(GutterEvent::Staged { at, unstage, result }));
        });
    }

    /// `AppEvent::Gutter(GutterEvent::Staged)`: says what happened; the markers and the status are read again.
    pub(super) fn staged(&mut self, at: StageAt, unstage: bool, result: Result<Option<Hunk>, String>) {
        self.quick_diff.staging -= 1;
        let line = at.line + 1;
        let name = self.buffers.get(at.buffer).map(|b| b.name()).unwrap_or_default();
        let (level, text) = match (result, unstage) {
            (Ok(_), false) if at.what == What::File => (Level::Info, format!("Staged {name}")),
            (Ok(Some(_)), false) => (Level::Info, format!("Staged the change at line {line}")),
            (Ok(Some(_)), true) => (Level::Info, format!("Unstaged the change at line {line}")),
            (Ok(None), false) => (Level::Info, format!("No change to stage at line {line}")),
            (Ok(None), true) => (Level::Info, format!("No staged change at line {line}")),
            (Err(e), false) => (Level::Error, format!("Staging failed: {e}")),
            (Err(e), true) => (Level::Error, format!("Unstaging failed: {e}")),
        };
        self.notify(level, text);
        self.staged_refresh(at.buffer);
    }

    /// The index changed: the buffer's markers and the git status are read again.
    fn staged_refresh(&mut self, buffer: BufferId) {
        if let Some(buf) = self.buffers.get_mut(buffer) {
            buf.quick_diff.index_changed();
        }
        self.refresh_git(None);
    }

    fn spawn_stage_lines(
        &mut self,
        at: StageAt,
        unstage: bool,
        work: impl FnOnce() -> Result<usize, stage::Error> + Send + 'static,
    ) {
        self.close_peek();
        self.quick_diff.staging += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = work().map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::Gutter(GutterEvent::StagedLines { at, unstage, result }));
        });
    }

    /// `AppEvent::Gutter(GutterEvent::StagedLines)`: says how many lines; markers and status are read again.
    fn staged_lines(&mut self, at: StageAt, unstage: bool, result: Result<usize, String>) {
        self.quick_diff.staging -= 1;
        let (level, text) = match (result, unstage) {
            (Ok(0), _) => (Level::Info, "No changes in the selection".to_string()),
            (Ok(n), false) => (Level::Info, format!("Staged {}", changed_lines(n))),
            (Ok(n), true) => (Level::Info, format!("Unstaged {}", changed_lines(n))),
            (Err(e), false) => (Level::Error, format!("Staging failed: {e}")),
            (Err(e), true) => (Level::Error, format!("Unstaging failed: {e}")),
        };
        self.notify(level, text);
        self.staged_refresh(at.buffer);
    }

    /// The active text tab's buffer and its selections' (start, end) in order; a message when
    /// the active tab isn't a text editor.
    fn selected_ranges(&mut self) -> Option<(BufferId, Vec<(ByteIdx, ByteIdx)>)> {
        let found = self.group().active_tab().and_then(|t| {
            let ranges = t.view.selections.iter().map(|s| (s.start(), s.end())).collect();
            self.buffers.get(t.buffer).map(|_| (t.buffer, ranges))
        });
        if found.is_none() {
            self.notify(Level::Info, "Select lines in a text tab".into());
        }
        found
    }

    /// `git.stageSelectedRanges` / `git.unstageSelectedRanges`: just the changed lines the
    /// active text tab's selections touch. Staging goes through `stage_at` (a dirty buffer saved
    /// first, a save in flight waited for); unstaging maps the buffer's lines to the index's
    /// through the buffer's text.
    pub(super) fn stage_selected(&mut self, unstage: bool) {
        let Some((buffer, ranges)) = self.selected_ranges() else {
            return;
        };
        self.quick_diff.ranges = ranges;
        let at = StageAt {
            buffer,
            line: 0,
            what: What::Ranges,
        };
        if !unstage {
            return self.stage_at(at);
        }
        let Some((repo, path)) = self.git_path(buffer) else {
            return self.quick_diff.ranges.clear();
        };
        let Some(buf) = self.buffers.get(buffer) else { return };
        let (rope, bom) = (buf.rope.clone(), buf.bom);
        let ranges = std::mem::take(&mut self.quick_diff.ranges);
        self.spawn_stage_lines(at, true, move || {
            let text = crate::buffer::file::encode(&rope, bom);
            stage::unstage_ranges(&repo, &path, &text, &line_ranges(&rope, &ranges))
        });
    }

    /// `git.revertSelectedRanges`: the changed lines the selections touch go back to the index's
    /// lines in the buffer (one undo step, unsaved). The diff against the index is worked out
    /// off-thread on the buffer's text as it is now; `reverted` applies it if the buffer still
    /// is that text.
    pub(super) fn revert_selected(&mut self) {
        let Some((buffer, ranges)) = self.selected_ranges() else {
            return;
        };
        let Some(buf) = self.buffers.get(buffer) else { return };
        if let Some(reason) = &buf.read_only {
            let text = format!("Read-only: {reason}");
            return self.notify(Level::Error, text);
        }
        let Some((repo, path)) = self.git_path(buffer) else {
            return;
        };
        self.ensure_status_loaded();
        let buf = self.buffers.get(buffer).expect("checked above");
        let name = buf.name();
        let entry = self.git.status.as_ref().and_then(|s| s.entry(&path));
        let why = match entry {
            Some(e) if e.unstaged == Some(Change::Untracked) => Some("it isn't in the index"),
            Some(e) if e.unstaged == Some(Change::Conflicted) || e.staged == Some(Change::Conflicted) => {
                Some("it has merge conflicts")
            }
            _ => None,
        };
        if let Some(why) = why {
            return self.notify(Level::Info, format!("Can't revert lines in {name}: {why}"));
        }
        let (rope, bom, revision) = (buf.rope.clone(), buf.bom, buf.revision());
        self.quick_diff.staging += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = plan_revert(&repo, &path, &rope, bom, &line_ranges(&rope, &ranges))
                .map_err(|why| format!("Can't revert lines in {name}: {why}"));
            let _ = tx.send(AppEvent::Gutter(GutterEvent::Reverted {
                buffer,
                revision,
                result,
            }));
        });
    }

    /// `AppEvent::Gutter(GutterEvent::Reverted)`: the plan's edits go into the buffer as one
    /// undo step, unless it changed meanwhile.
    fn reverted(&mut self, buffer: BufferId, revision: u64, result: Result<RevertPlan, String>) {
        self.quick_diff.staging -= 1;
        let Some(buf) = self.buffers.get(buffer) else { return };
        let plan = match result {
            Ok(plan) => plan,
            Err(e) => return self.notify(Level::Error, e),
        };
        if plan.edits.is_empty() {
            return self.notify(Level::Info, "No changes in the selection".into());
        }
        let name = buf.name();
        if buf.revision() != revision {
            let text = format!("Can't revert lines in {name}: the file changed, try again");
            return self.notify(Level::Error, text);
        }
        if let Some(reason) = &buf.read_only {
            return self.notify(Level::Error, format!("Read-only: {reason}"));
        }
        let mut changes = ChangeSet::with_capacity(plan.edits.len(), plan.edits.iter().map(|e| e.2.len()).sum());
        for (start, end, text) in &plan.edits {
            changes.push(*start, *end, text);
        }
        self.apply_changes(buffer, changes);
        self.follow_edits();
        self.notify(
            Level::Info,
            format!("Reverted {} (not saved)", changed_lines(plan.lines)),
        );
    }

    /// Opens the peek for the change drawn on `line` in `group` (closes it when it's already
    /// open there). False when no change is drawn there.
    pub(super) fn open_peek(&mut self, group: GroupId, line: usize) -> bool {
        let Some(tab) = self.groups.get(group).and_then(|g| g.active_tab()) else {
            return false;
        };
        let Some(buf) = self.buffers.get(tab.buffer) else {
            return false;
        };
        let Some((_, mark)) = buf.quick_diff.at_line(&buf.rope, line) else {
            return false;
        };
        let Some(lines) = buf.quick_diff.lines_of(&buf.rope, mark) else {
            return false;
        };
        let peek = Peek {
            group,
            buffer: tab.buffer,
            revision: buf.revision(),
            lines,
            mark: mark.clone(),
        };
        let same = self
            .quick_diff
            .peek
            .as_ref()
            .is_some_and(|p| (p.group, p.buffer, &p.lines) == (group, peek.buffer, &peek.lines));
        self.quick_diff.peek = (!same).then_some(peek);
        self.needs_redraw = true;
        true
    }

    pub(super) fn close_peek(&mut self) {
        if self.quick_diff.peek.take().is_some() {
            self.needs_redraw = true;
        }
    }

    /// The peek's Stage (its button, Enter).
    pub(super) fn stage_peek(&mut self) {
        if let Some(p) = self.quick_diff.peek.take() {
            self.stage_at(StageAt {
                buffer: p.buffer,
                line: p.lines.start,
                what: What::Line,
            });
        }
    }

    /// After every event: the peek closes when its buffer was edited or its group shows another tab.
    pub(super) fn check_peek(&mut self) {
        let Some(p) = &self.quick_diff.peek else { return };
        let fits = self
            .groups
            .get(p.group)
            .and_then(|g| g.active_tab())
            .is_some_and(|t| t.buffer == p.buffer)
            && self.buffers.get(p.buffer).is_some_and(|b| b.revision() == p.revision);
        if !fits {
            self.close_peek();
        }
    }

    /// The gutter marker under a screen cell: its group and line. The marker is drawn on the
    /// line's first row, so while wrapping the screen row maps through the rows and only a first
    /// row is a hit.
    fn marker_at(&mut self, x: u16, y: u16) -> Option<(GroupId, usize)> {
        let Hit::Editor(g) = self.hits.hit(x, y) else {
            return None;
        };
        let text = self.hits.group(g)?.text;
        if text.x == 0 || x + 1 != text.x || y < text.y || y >= text.y + text.height {
            return None;
        }
        let tab = self.groups.get_mut(g)?.active_tab_mut()?;
        let buf = self.buffers.get(tab.buffer)?;
        let dy = (y - text.y) as usize;
        let line = if tab.view.wrapping && tab.view.width > 0 {
            let (top, top_row) = (tab.view.top_line(), tab.view.top_row());
            let (line, sub) = tab.view.rows_down(&buf.rope, top, top_row, dy);
            if sub != 0 {
                return None;
            }
            line
        } else {
            tab.view.top_line() + dy
        };
        buf.quick_diff.at_line(&buf.rope, line).map(|_| (g, line))
    }

    /// A plain click in a group's editor: on a gutter marker it opens (or closes) the peek.
    pub(super) fn gutter_click(&mut self, m: &MouseEvent) -> bool {
        let Some((group, line)) = self.marker_at(m.column, m.row) else {
            return false;
        };
        self.focus_group(group);
        self.open_peek(group, line)
    }

    /// First in `handle_mouse` (after the hover's): events over the peek stop there (a click on
    /// Stage stages, on × closes); a click elsewhere closes it (a click on its own marker only
    /// that) and goes on.
    pub(super) fn peek_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        let Some(peek) = &self.quick_diff.peek else {
            return false;
        };
        let (group, lines) = (peek.group, peek.lines.clone());
        let down = matches!(m.kind, MouseEventKind::Down(MouseButton::Left));
        if hit == Hit::QuickDiff {
            if down && let Some(h) = self.hits.quick_diff {
                let p = ratatui::layout::Position { x: m.column, y: m.row };
                if h.stage.contains(p) {
                    self.stage_peek();
                } else if h.close.contains(p) {
                    self.close_peek();
                }
            }
            return true;
        }
        if matches!(m.kind, MouseEventKind::Down(_)) {
            let own = self
                .marker_at(m.column, m.row)
                .is_some_and(|(g, line)| g == group && lines.contains(&line));
            self.close_peek();
            return own;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::single_range_in_vec_init)]
    use super::*;

    fn lines_of(text: &str, spans: &[(usize, usize)]) -> Vec<Range<usize>> {
        line_ranges(
            &ropey::Rope::from_str(text),
            &spans.iter().map(|&(a, b)| (ByteIdx(a), ByteIdx(b))).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn selections_become_merged_line_ranges() {
        let text = "aa\nbb\ncc\ndd\n";
        // A cursor is its line; a selection ending at column 0 doesn't take that line.
        assert_eq!(lines_of(text, &[(1, 1)]), [0..1]);
        assert_eq!(lines_of(text, &[(1, 3)]), [0..1]);
        assert_eq!(lines_of(text, &[(1, 4)]), [0..2]);
        assert_eq!(lines_of(text, &[(0, 6)]), [0..2]);
        // Touching or overlapping ranges merge; others stay apart.
        assert_eq!(lines_of(text, &[(0, 2), (3, 4), (9, 10)]), [0..2, 3..4]);
        // To the end of the text: the empty line after the last break isn't taken.
        assert_eq!(lines_of(text, &[(9, 12)]), [3..4]);
        assert_eq!(lines_of(text, &[(9, 9)]), [3..4]);
        assert_eq!(lines_of("x", &[(0, 1)]), [0..1]);
    }

    #[test]
    fn pending_ranges_follow_edits_of_their_buffer_only() {
        let mut q = QuickDiffState::default();
        let at = StageAt {
            buffer: 1,
            line: 0,
            what: What::Ranges,
        };
        q.after_save = Some((at, ByteIdx(10)));
        q.ranges = vec![(ByteIdx(10), ByteIdx(20))];
        // Two characters typed before the selection, then three inside it.
        let mut a = ChangeSet::with_capacity(1, 2);
        a.push(ByteIdx(0), ByteIdx(0), "xx");
        let mut b = ChangeSet::with_capacity(1, 3);
        b.push(ByteIdx(15), ByteIdx(15), "yyy");
        q.follow(1, &[Arc::new(a), Arc::new(b)]);
        assert_eq!(q.ranges, [(ByteIdx(12), ByteIdx(25))]);
        assert_eq!(q.after_save.map(|(_, s)| s), Some(ByteIdx(12)));
        let mut c = ChangeSet::with_capacity(1, 2);
        c.push(ByteIdx(0), ByteIdx(0), "zz");
        q.follow(2, &[Arc::new(c)]);
        assert_eq!(q.ranges, [(ByteIdx(12), ByteIdx(25))]);
    }
}
