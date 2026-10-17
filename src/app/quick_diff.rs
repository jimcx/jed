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
use crate::git::{Hunk, Repo, stage};
use crate::layout::{GroupId, Hit};
use crate::quick_diff::{self, Computed, Job, Mark, Want};
use crate::selection::{Selection, Selections};

/// The app's side of quick diff.
#[derive(Default)]
pub struct QuickDiffState {
    /// When the markers of edited buffers are computed again.
    deadline: Option<Instant>,
    /// A stage waiting for the buffer's save in flight to end, and the start of its line,
    /// followed through edits made meanwhile (`follow`): the line it stages is that one's.
    after_save: Option<(StageAt, ByteIdx)>,
    /// The peek (a click on a gutter marker), while open.
    pub peek: Option<Peek>,
    /// Stage / unstage tasks running (tests wait for them).
    pub staging: usize,
}

impl QuickDiffState {
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
            }
        }
    }
}

/// Which change to stage: the one the gutter shows on `line` (0-based) of `buffer`, or with
/// `file` the whole file (`git.stageFile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageAt {
    pub buffer: BufferId,
    pub line: usize,
    pub file: bool,
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
            let known = buf.quick_diff.begin(revision);
            start.push((
                id,
                Job {
                    repo: repo.clone(),
                    path,
                    rope: buf.rope.clone(),
                    bom: buf.bom,
                    revision,
                    known,
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
                let _ = tx.send(AppEvent::QuickDiff { buffer, computed });
            });
        }
    }

    /// `AppEvent::QuickDiff`: a buffer's new markers.
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
                file: false,
            });
        }
    }

    /// The repository and the buffer's path in it; `None` (with a message) outside one, and
    /// (silently) for a log tab: no file.
    fn git_path(&mut self, buffer: BufferId) -> Option<(Repo, PathBuf)> {
        let path = self.buffers.get(buffer).filter(|b| b.log.is_none())?.path.clone();
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
        if dirty && !at.file && buf.quick_diff.at_line(&buf.rope, at.line).is_none() {
            self.notify(Level::Info, format!("No change to stage at line {}", at.line + 1));
            return;
        }
        if buf.save_in_flight() {
            let start = pos::line_start(&buf.rope, at.line.min(pos::line_count(&buf.rope) - 1));
            self.quick_diff.after_save = Some((at, start));
            self.notify(Level::Info, "Waiting for the save to finish…".into());
            return;
        }
        if dirty {
            self.close_peek();
            self.prompt = Some(Prompt::SaveBeforeStage {
                buffer: at.buffer,
                line: at.line,
                file: at.file,
            });
            return;
        }
        if at.file {
            self.spawn_stage(at, false, move || stage::stage_file(&repo, &path).map(|()| None));
        } else {
            self.spawn_stage(at, false, move || stage::stage_line(&repo, &path, at.line));
        }
    }

    /// The save prompt's Save: saves, then stages once the save is done.
    pub(super) fn save_then_stage(&mut self, at: StageAt) {
        let Some(buf) = self.buffers.get(at.buffer) else { return };
        let start = pos::line_start(&buf.rope, at.line.min(pos::line_count(&buf.rope) - 1));
        self.quick_diff.after_save = Some((at, start));
        self.save(at.buffer);
        if !self.buffers.get(at.buffer).is_some_and(|b| b.save_in_flight()) {
            // Refused (read-only): said so.
            self.quick_diff.after_save = None;
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
            file: false,
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
            let _ = tx.send(AppEvent::Staged { at, unstage, result });
        });
    }

    /// `AppEvent::Staged`: says what happened; the markers and the status are read again.
    pub(super) fn staged(&mut self, at: StageAt, unstage: bool, result: Result<Option<Hunk>, String>) {
        self.quick_diff.staging -= 1;
        let line = at.line + 1;
        let name = self.buffers.get(at.buffer).map(|b| b.name()).unwrap_or_default();
        let (level, text) = match (result, unstage) {
            (Ok(_), false) if at.file => (Level::Info, format!("Staged {name}")),
            (Ok(Some(_)), false) => (Level::Info, format!("Staged the change at line {line}")),
            (Ok(Some(_)), true) => (Level::Info, format!("Unstaged the change at line {line}")),
            (Ok(None), false) => (Level::Info, format!("No change to stage at line {line}")),
            (Ok(None), true) => (Level::Info, format!("No staged change at line {line}")),
            (Err(e), false) => (Level::Error, format!("Staging failed: {e}")),
            (Err(e), true) => (Level::Error, format!("Unstaging failed: {e}")),
        };
        self.notify(level, text);
        if let Some(buf) = self.buffers.get_mut(at.buffer) {
            buf.quick_diff.index_changed();
        }
        self.refresh_git(None);
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
                file: false,
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

    /// The gutter marker under a screen cell: its group and line.
    fn marker_at(&self, x: u16, y: u16) -> Option<(GroupId, usize)> {
        let Hit::Editor(g) = self.hits.hit(x, y) else {
            return None;
        };
        let text = self.hits.group(g)?.text;
        if text.x == 0 || x + 1 != text.x || y < text.y || y >= text.y + text.height {
            return None;
        }
        let tab = self.groups.get(g)?.active_tab()?;
        let buf = self.buffers.get(tab.buffer)?;
        let line = tab.view.top_line() + (y - text.y) as usize;
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
                .is_some_and(|(g, line)| g == peek.group && peek.lines.contains(&line));
            self.close_peek();
            return own;
        }
        false
    }
}
