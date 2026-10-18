//! Commit tabs in the app (the model is `commit_view::CommitDoc`): opening a commit from the
//! Explorer's "Git History" section (reusing its buffer and a tab already showing it), reading
//! `git show` off the UI thread once per commit, fitting the text to the width of the view that
//! shows it, and opening a stat line's change (Enter, a click) as a diff in the group to the
//! right (splitting one when there is none, reusing the group's commit diff). A commit tab is a
//! read-only text tab, so everything else — keys, mouse, find, copy, splits — is the editor's.

use std::sync::Arc;

use git2::Oid;
use ropey::Rope;

use super::{App, AppEvent, Focus, Level};
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::{Buffer, BufferId};
use crate::commit_view::CommitDoc;
use crate::diff_view::{DiffTab, Pair, Text};
use crate::disk;
use crate::git;
use crate::git::show::Show;
use crate::group::Tab;
use crate::layout::{GroupId, SplitDir};
use crate::quick_diff::Want;
use crate::selection::{Selection, Selections};
use crate::view::View;

/// The width a commit is rendered at before any frame has given its view one.
const UNDRAWN_WIDTH: usize = 80;

impl App {
    /// Each loop turn (`start`, `run`, `pump` / `settle`): starts the loads due (a new commit
    /// buffer). A commit never changes, so a loaded or failed one is never read again. Nothing
    /// without a tokio runtime.
    pub(super) fn sync_commit_tabs(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let repo = self.git.status.as_ref().map(|s| s.repo.clone());
        // The first status read may still be running (a session's commit tabs): wait for it
        // rather than calling it no repository for good.
        let waiting = repo.is_none() && self.git.running();
        let mut jobs = Vec::new();
        let mut failed = Vec::new();
        for (id, buf) in self.buffers.iter_mut() {
            let Some(c) = buf.commit.as_mut().filter(|c| c.want() == Want::Now) else {
                continue;
            };
            match &repo {
                None if waiting => continue,
                None => {
                    // Not in a repository: no retry until the tab is reopened.
                    c.begin();
                    c.finish(Err("Not in a git repository".into()));
                    failed.push(id);
                }
                Some(repo) => {
                    c.begin();
                    jobs.push((c.commit.clone(), repo.clone()));
                }
            }
        }
        for id in failed {
            self.fit_commit(id, None);
        }
        for (commit, repo) in jobs {
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let result = git::show::read(&repo, &commit).map_err(|e| e.message().to_string());
                let _ = tx.send(AppEvent::CommitLoaded { commit, result });
            });
        }
    }

    /// `AppEvent::CommitLoaded`: the commit's buffer takes it, its text rendered at once.
    pub(super) fn commit_loaded(&mut self, commit: String, result: Result<Show, String>) {
        let Some(id) = self.find_commit_buffer(&commit) else {
            return;
        };
        let done = self
            .buffers
            .get_mut(id)
            .and_then(|b| b.commit.as_mut())
            .is_some_and(|c| c.finish(result.map(Arc::new)));
        if done {
            self.fit_commit(id, None);
        }
    }

    /// The commit buffer of `commit`, made (empty, loading at the next turn) when there is none.
    /// `file`: the work-tree-relative path of the file whose history opens it.
    pub(super) fn commit_buffer(&mut self, commit: &str, file: Option<String>) -> BufferId {
        match self.find_commit_buffer(commit) {
            Some(id) => {
                if let Some(c) = self.buffers.get_mut(id).and_then(|b| b.commit.as_mut())
                    && file.is_some()
                {
                    c.file = file;
                }
                id
            }
            None => self.buffers.insert(Buffer::commit(commit.to_string(), file)),
        }
    }

    fn find_commit_buffer(&self, commit: &str) -> Option<BufferId> {
        self.buffers
            .iter()
            .find(|(_, b)| b.commit.as_ref().is_some_and(|c| c.commit == commit))
            .map(|(id, _)| id)
    }

    /// The Explorer's Enter (or a click) on commit `id`: shows a tab already showing it (in
    /// whichever group, activated), else opens one beside the active tab. `file` is the
    /// work-tree-relative path of the file whose history it came from.
    pub(super) fn open_commit(&mut self, id: &str, file: Option<String>) {
        let buffer = self.commit_buffer(id, file);
        let shown = self
            .groups
            .ids()
            .into_iter()
            .find_map(|g| Some((g, self.groups.get(g)?.find(buffer)?)));
        if let Some((group, index)) = shown {
            self.focus_group(group);
            self.activate_tab(index);
        } else {
            if let Some((buf, _)) = self.active_mut() {
                buf.seal_undo();
            }
            self.group_mut().open(buffer, View::new());
            // A loaded commit (a tab closed and opened again): its text now, at this view's width.
            self.fit_commit(buffer, None);
        }
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
        self.sync_commit_tabs();
    }

    /// The commit the active tab shows, if it's a commit tab.
    pub(crate) fn active_commit(&self) -> Option<&CommitDoc> {
        self.commit_in(self.groups.active)
    }

    /// The commit `group`'s active tab shows, if it's a commit tab.
    pub(crate) fn commit_in(&self, group: GroupId) -> Option<&CommitDoc> {
        let tab = self.groups.get(group)?.active_tab()?;
        self.buffers.get(tab.buffer)?.commit.as_ref()
    }

    /// Enter in a commit tab (`commit.open`): the cursor line's file's change, as a diff.
    pub(super) fn open_commit_line(&mut self) {
        let Some((buf, view)) = self.active_mut() else { return };
        let Some(c) = &buf.commit else { return };
        let line = pos::line_of(&buf.rope, view.selections.primary().head);
        match c.file_at(line) {
            Some(k) => self.open_commit_diff(self.groups.active, k),
            None => self.notify(Level::Info, "No file on this line".into()),
        }
    }

    /// Drawing's pre-pass (`ui::render`): each commit buffer's text for the text width of a
    /// group showing it (`widths`, the active group's preferred). Re-renders only on a change.
    pub(crate) fn fit_commit_tabs(&mut self, widths: &[(GroupId, usize)]) {
        let mut fits: Vec<(BufferId, usize)> = Vec::new();
        for &(group, width) in widths {
            let Some(tab) = self.groups.get(group).and_then(|g| g.active_tab()) else {
                continue;
            };
            if self.buffers.get(tab.buffer).is_none_or(|b| b.commit.is_none()) {
                continue;
            }
            match fits.iter_mut().find(|(id, _)| *id == tab.buffer) {
                Some(fit) if group == self.groups.active => fit.1 = width,
                Some(_) => {}
                None => fits.push((tab.buffer, width)),
            }
        }
        for (id, width) in fits {
            self.fit_commit(id, Some(width));
        }
    }

    /// Re-renders commit buffer `id` for `width` (`None`: the width its views were last drawn
    /// at, the active one's first) when its text changes, every view following (as a log's
    /// lines do). The first text puts the cursor on the tab's own file's stat line.
    fn fit_commit(&mut self, id: BufferId, width: Option<usize>) {
        let width = width.or_else(|| self.drawn_width(id)).unwrap_or(UNDRAWN_WIDTH);
        let Some(buf) = self.buffers.get_mut(id) else { return };
        let Some(c) = buf.commit.as_mut() else { return };
        let Some(text) = c.render(width) else { return };
        let first = buf.rope.len() == 0;
        let changes = disk::changes(&buf.rope, &Rope::from_str(&text));
        buf.append(changes);
        let applied = buf.take_applied();
        let start = buf
            .commit
            .as_ref()
            .and_then(|c| c.own_file_row())
            .filter(|_| first)
            .map(|line| pos::line_start(&buf.rope, line));
        for (_, group) in self.groups.iter_mut() {
            for (_, tab) in group.text_tabs_mut() {
                if tab.buffer != id {
                    continue;
                }
                let view = &mut tab.view;
                view.map_through(&applied, &buf.rope);
                if first {
                    let at = start.unwrap_or(ByteIdx(0));
                    view.selections = Selections::single(Selection::cursor(at));
                    view.cursor_history.reset(&view.selections);
                    view.reveal_cursor = true;
                }
            }
        }
        self.needs_redraw = true;
    }

    /// The text width a view of buffer `id` was last drawn at (the active group's first);
    /// `None` before any frame drew one.
    fn drawn_width(&self, id: BufferId) -> Option<usize> {
        let ids = self.groups.ids();
        let order = std::iter::once(self.groups.active).chain(ids.into_iter().filter(|&g| g != self.groups.active));
        order
            .filter_map(|g| self.groups.get(g)?.active_tab())
            .filter(|t| t.buffer == id)
            .map(|t| t.view.width)
            .find(|&w| w > 1)
    }

    /// Opens the change of `show.files[k]` in `group`'s commit tab as a diff in the group to
    /// its right (splitting one when there is none), reusing that group's diff of the same
    /// commit (a stat row of another file) when it has one.
    pub(super) fn open_commit_diff(&mut self, group: GroupId, k: usize) {
        let Some(c) = self.commit_in(group) else {
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
}
