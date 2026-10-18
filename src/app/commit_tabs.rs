//! Commit tabs in the app (the model is `commit_view::CommitTab`, drawing `ui::commit`): opening
//! a commit from the Explorer's "Git History" section (reusing a tab already showing it),
//! reading `git show` off the UI thread once per commit, the keys (movement, copy, Enter on a
//! stat row) and the mouse (the wheel, a click's cursor, a drag's selection, a click on a stat
//! row), and opening a stat row's change as a diff in the group to the right (splitting one
//! when there is none, reusing the group's commit diff).

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use git2::Oid;
use ratatui::layout::Position;

use super::mouse::Drag;
use super::{App, AppEvent, Focus, Level};
use crate::command::{Command, CommitCommand};
use crate::commit_view::{CommitTab, TextPos};
use crate::diff_view::{DiffTab, Pair, Text};
use crate::git;
use crate::git::show::Show;
use crate::group::Tab;
use crate::layout::{GroupId, Hit, SplitDir};
use crate::quick_diff::Want;

/// Lines a wheel step scrolls.
const WHEEL_ROWS: isize = 3;

impl App {
    /// Each loop turn (`start`, `run`, `pump` / `settle`): starts the loads due (a new tab).
    /// A commit never changes, so a loaded or failed one is never read again. Nothing without a
    /// tokio runtime.
    pub(super) fn sync_commit_tabs(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let repo = self.git.status.as_ref().map(|s| s.repo.clone());
        // The first status read may still be running (a session's commit tabs): wait for it
        // rather than calling it no repository for good.
        let waiting = repo.is_none() && self.git.running();
        let mut jobs = Vec::new();
        let mut redraw = false;
        for (_, c) in self.groups.commit_tabs_mut() {
            if c.want() != Want::Now {
                continue;
            }
            match &repo {
                None if waiting => continue,
                None => {
                    // Not in a repository: no retry until the tab is reopened.
                    c.error = Some("Not in a git repository".into());
                    redraw = true;
                }
                Some(repo) => {
                    c.begin();
                    jobs.push((c.commit.clone(), repo.clone()));
                }
            }
        }
        if redraw {
            self.needs_redraw = true;
        }
        for (commit, repo) in jobs {
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = git::show::read(&repo, &commit).map_err(|e| e.message().to_string());
                let _ = tx.send(AppEvent::CommitLoaded { commit, result });
            });
        }
    }

    /// `AppEvent::CommitLoaded`: every tab showing the commit takes it (there may be several:
    /// a split, or two histories of one commit).
    pub(super) fn commit_loaded(&mut self, commit: String, result: Result<Show, String>) {
        let mut done = false;
        for (_, c) in self.groups.commit_tabs_mut() {
            if c.commit == commit && c.finish(result.clone()) {
                done = true;
            }
        }
        if done {
            self.needs_redraw = true;
        }
    }

    /// The Explorer's Enter (or a click) on commit `id`: shows a tab already showing it (in
    /// whichever group, activated), else opens one beside the active tab. `file` is the
    /// work-tree-relative path of the file whose history it came from.
    pub(super) fn open_commit(&mut self, id: &str, file: Option<String>) {
        if let Some((group, index)) = self.find_commit_tab(id) {
            self.focus_group(group);
            self.group_mut().active = index;
            if let Some(c) = self.group_mut().active_commit_mut() {
                c.file = file;
            }
        } else {
            if let Some((buf, _)) = self.active_mut() {
                buf.seal_undo();
            }
            let tab = Tab::Commit(Box::new(CommitTab::new(id.to_string(), file)));
            self.group_mut().open_tab(tab);
        }
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
        self.sync_commit_tabs();
    }

    /// A tab showing `commit`, in reading order.
    fn find_commit_tab(&self, commit: &str) -> Option<(GroupId, usize)> {
        for group in self.groups.ids() {
            let g = self.groups.get(group)?;
            if let Some(index) = g
                .tabs
                .iter()
                .position(|t| t.commit().is_some_and(|c| c.commit == commit))
            {
                return Some((group, index));
            }
        }
        None
    }

    /// A command while the active tab is a commit: movement, select all, Esc (all read only) and
    /// Enter on a stat row. Other editor commands do nothing there. False when the active tab
    /// isn't a commit.
    pub(super) fn commit_command(&mut self, cmd: Command) -> bool {
        if cmd == Command::Commit(CommitCommand::Open) {
            let Some(c) = self.group().active_commit() else {
                return false;
            };
            match c.file_at(c.cursor.row) {
                Some(k) => self.open_commit_diff(self.groups.active, k),
                None => self.notify(Level::Info, "No file on this line".into()),
            }
            return true;
        }
        let Some(c) = self.group_mut().active_commit_mut() else {
            return false;
        };
        match cmd {
            Command::Move { motion, select } => c.move_motion(motion, select),
            Command::SelectAll => c.select_all(),
            Command::CancelSelection => c.cancel_selection(),
            _ => tracing::debug!(command = %cmd.name(), "not in a commit tab"),
        }
        self.needs_redraw = true;
        true
    }

    /// Opens the change of `show.files[k]` in `group`'s commit tab as a diff in the group to
    /// its right (splitting one when there is none), reusing that group's diff of the same
    /// commit (a stat row of another file) when it has one.
    fn open_commit_diff(&mut self, group: GroupId, k: usize) {
        let Some(c) = self.groups.get(group).and_then(|g| g.active_commit()) else {
            return;
        };
        let Some(show) = c.show.clone() else {
            return; // still loading, or failed
        };
        let Some(file) = show.files.get(k) else { return };
        let Ok(commit) = Oid::from_str(&c.commit) else { return };
        let Some(status) = &self.git.status else {
            self.notify(Level::Info, "Not in a git repository".into());
            return;
        };
        let path = status.repo.workdir.join(&file.path);
        let pair = Pair::commit(commit, show.first_parent);
        let target = self.group_to_the_right(group);
        let id = self.next_diff_id();
        let tab = Tab::Diff(Box::new(DiffTab::new(id, path, pair)));
        let g = self.groups.get_mut(target).expect("a split or a listed group");
        let existing = g.tabs.iter().position(|t| {
            t.diff()
                .is_some_and(|d| d.pair.right == Text::Commit(commit) || d.pair.left == Text::Commit(commit))
        });
        match existing {
            Some(i) => {
                g.tabs[i] = tab;
                g.active = i;
            }
            None => {
                g.open_tab(tab);
            }
        }
        self.focus_group(target);
        self.sync_diffs();
    }

    /// A group drawn strictly to the right of `group` (sharing rows with it, nearest first).
    /// With a frame and nothing there, a new group splits to the right (a group below is not
    /// "to the right"); before the first frame, a sensible guess is the next group in reading
    /// order, else a new group to the right.
    fn group_to_the_right(&mut self, group: GroupId) -> GroupId {
        if let Some(cells) = self.drawn_groups()
            && let Some(&here) = cells.iter().find(|(g, _)| *g == group).map(|(_, r)| r)
        {
            let right = cells
                .iter()
                .filter(|(g, r)| {
                    *g != group && r.x >= here.x + here.width && r.y < here.y + here.height && here.y < r.y + r.height
                })
                .min_by_key(|(_, r)| r.x)
                .map(|(g, _)| *g);
            return right.unwrap_or_else(|| {
                self.groups.active = group;
                self.groups.split_empty(SplitDir::Right)
            });
        }
        let ids = self.groups.ids();
        if let Some(next) = ids.iter().position(|&g| g == group).and_then(|i| ids.get(i + 1)) {
            return *next;
        }
        self.groups.active = group;
        self.groups.split_empty(SplitDir::Right)
    }

    /// The groups as the last frame laid them out, `None` before the first frame.
    fn drawn_groups(&self) -> Option<Vec<(GroupId, ratatui::layout::Rect)>> {
        let area = self.hits.regions.editor;
        (area.width > 0 && area.height > 0).then(|| self.groups.tree.layout(area).0)
    }

    /// The mouse over a group's editor showing a commit: the wheel scrolls it (without
    /// focusing); a left click focuses it, puts the cursor at the cell and, on a stat row, opens
    /// that file's change; a left drag extends the selection. False for other groups and events
    /// (moves, releases), which go on to the rest of `handle_mouse`.
    pub(super) fn commit_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        let Hit::Editor(group) = hit else { return false };
        if self.groups.get(group).and_then(|g| g.active_commit()).is_none() {
            return false;
        }
        match m.kind {
            MouseEventKind::ScrollUp => {
                if let Some(c) = self.commit_tab_mut(group) {
                    c.scroll_by(-WHEEL_ROWS);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(c) = self.commit_tab_mut(group) {
                    c.scroll_by(WHEEL_ROWS);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(pos) = self.commit_position(group, m.column, m.row) else {
                    self.focus_group(group);
                    return true;
                };
                self.focus_group(group);
                let mut file = None;
                if let Some(c) = self.group_mut().active_commit_mut() {
                    c.set_cursor(pos);
                    file = c.file_at(pos.row);
                }
                self.drag = Some(Drag::Commit { group, anchor: pos });
                if let Some(k) = file {
                    self.open_commit_diff(group, k);
                }
                self.needs_redraw = true;
            }
            // A divider or panel border dragged over it, a release, a move: not its.
            _ => return false,
        }
        true
    }

    /// The commit tab at the pointer (a click's cursor): `None` off its text.
    fn commit_position(&self, group: GroupId, x: u16, y: u16) -> Option<TextPos> {
        let rows = self.hits.group(group)?.commit?.rows;
        if !rows.contains(Position { x, y }) {
            return None;
        }
        let c = self.groups.get(group)?.active_commit()?;
        let row = c.top + (y - rows.y) as usize;
        if row >= c.rows() {
            return None;
        }
        Some(TextPos::new(row, c.col_at(row, (x - rows.x) as usize)))
    }

    /// The place under the pointer while dragging a commit selection, clamped to its text.
    pub(super) fn commit_drag_position(&self, group: GroupId, x: u16, y: u16) -> Option<TextPos> {
        let rows = self.hits.group(group)?.commit?.rows;
        let c = self.groups.get(group)?.active_commit()?;
        if c.rows() == 0 {
            return None;
        }
        let row = (c.top + y.saturating_sub(rows.y) as usize).min(c.rows() - 1);
        let x = x.saturating_sub(rows.x) as usize;
        Some(TextPos::new(row, c.col_at(row, x)))
    }

    fn commit_tab_mut(&mut self, group: GroupId) -> Option<&mut CommitTab> {
        self.groups.get_mut(group)?.active_commit_mut()
    }
}
