//! The Code Review panel (`app.review`, model `crate::review`, drawn by `ui::review`): when it looks
//! the merge request up, what it does with the answer, and the file list's keys and mouse.
//!
//! A lookup (`review::lookup`) runs in `spawn_blocking` and comes back as
//! `AppEvent::Review(ReviewEvent::Looked)`. It starts at launch and whenever HEAD moves
//! (`review_head_check`, from the git status handlers), when the panel is shown, on "Code Review:
//! Refresh" and every `review.interval` (60 s) while the panel is on screen (`review_deadline`).
//! One runs at a time; a request meanwhile is queued. A failed lookup keeps the last result on
//! screen under its error; the viewed marks are written off-thread when they change.

use crossterm::event::MouseEvent;
use ratatui::layout::Position;

use super::{App, AppEvent, Focus, Level};
use crate::command::ReviewCommand as C;
use crate::diff_view::{DiffTab, Pair};
use crate::git::Head;
use crate::layout::SidePanel;
use crate::logs::Tool;
use crate::review::{Content, Empty, Outcome, Request};

/// Results of the panel's background work.
pub(super) enum ReviewEvent {
    Looked(Outcome),
}

impl App {
    /// The panel is on screen.
    fn review_visible(&self) -> bool {
        self.ui.side_visible && self.ui.side_panel == SidePanel::Review
    }

    /// When the visible panel looks again (`None` while hidden or looking).
    pub(super) fn review_deadline(&self) -> Option<std::time::Instant> {
        if self.review_visible() && !self.review.loading {
            self.review.next_poll
        } else {
            None
        }
    }

    /// The panel was shown (`focus_side`): the first file is selected and the lookup runs again.
    pub(super) fn review_shown_now(&mut self) {
        self.review_refresh();
        let (view, files) = self.review.split();
        if view.selected_row(files).is_none() {
            view.select(files, 0);
        }
    }

    /// Called after every git status: a new branch, commit or repository looks the merge request
    /// up again (and forgets the switched-to one). Tests opt in with `test_review_forge`.
    pub(super) fn review_head_check(&mut self) {
        // Runs after every git status: compare in place, copy only when HEAD moved.
        let unchanged = match (&self.git.status, &self.review.head_key) {
            (Some(s), Some(k)) => k.0 == s.repo.workdir && k.1 == s.head.name() && k.2 == s.head_id,
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        let key = self
            .git
            .status
            .as_ref()
            .map(|s| (s.repo.workdir.clone(), s.head.name().to_string(), s.head_id.clone()));
        let repo_changed = key.as_ref().map(|k| &k.0) != self.review.head_key.as_ref().map(|k| &k.0);
        self.review.head_key = key;
        self.review.choice = None;
        if repo_changed {
            self.review.content = Content::None;
            self.review.error = None;
        }
        if self.review.auto {
            self.review_refresh();
        }
    }

    /// Looks the merge request of the checked-out branch up (a lookup already running runs once
    /// more afterwards).
    pub(super) fn review_refresh(&mut self) {
        if !self.mode.allows(crate::command::Command::ShowReview) {
            return;
        }
        self.review.next_poll = None;
        let Some(status) = &self.git.status else {
            self.review.content = Content::Empty(Empty::NoRepo);
            self.review.error = None;
            return;
        };
        if matches!(status.head, Head::Detached(_) | Head::Unborn(_)) {
            self.review.content = Content::Empty(Empty::Detached);
            self.review.error = None;
            return;
        }
        if self.review.loading {
            self.review.queued = true;
            return;
        }
        self.review.loading = true;
        let req = Request {
            repo: status.repo.clone(),
            config: self.review.config.clone(),
            env: self.review.env.clone(),
            choose: self.review.choice,
            viewed_from: self.review.viewed.is_none().then(|| self.state_dir.clone()).flatten(),
            log: self.logs.tool(Tool::Review),
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(AppEvent::Review(ReviewEvent::Looked(crate::review::lookup(req))));
        });
    }

    pub(super) fn handle_review_event(&mut self, ev: ReviewEvent) {
        let ReviewEvent::Looked(outcome) = ev;
        let r = &mut self.review;
        r.loading = false;
        r.next_poll = Some(std::time::Instant::now() + r.interval);
        match outcome {
            Outcome::Empty(e) => {
                r.viewed.get_or_insert_with(Default::default);
                r.content = Content::Empty(e);
                r.error = None;
            }
            Outcome::Failed(message) => {
                r.viewed.get_or_insert_with(Default::default);
                tracing::warn!("code review: {message}");
                r.error = Some(message);
            }
            Outcome::Loaded(review, viewed) => {
                if r.viewed.is_none() {
                    r.viewed = Some(viewed.unwrap_or_default());
                }
                let pruned = r
                    .viewed
                    .as_mut()
                    .is_some_and(|v| v.prune(&review.repo, review.mr.number, &review.files));
                r.choice = Some(review.mr.number);
                r.content = Content::Loaded(review);
                r.error = None;
                if pruned {
                    self.save_viewed();
                }
            }
        }
        let (view, files) = self.review.split();
        view.settle(files);
        if std::mem::take(&mut self.review.queued) {
            self.review_refresh();
        }
    }

    /// Writes the viewed marks to the state directory, off-thread (nothing without one).
    fn save_viewed(&self) {
        let (Some(dir), Some(viewed)) = (self.state_dir.clone(), self.review.viewed.clone()) else {
            return;
        };
        let log = self.logs.tool(Tool::Review);
        tokio::task::spawn_blocking(move || {
            if let Err(e) = viewed.save(&dir) {
                log.error(&format!("can't save the viewed marks: {e}"));
            }
        });
    }

    /// "Code Review: Switch Merge Request": the branch's next open merge request.
    pub(super) fn review_switch(&mut self) {
        match self
            .review
            .review()
            .and_then(|r| r.next_candidate().map(|n| (n, r.mr.number)))
        {
            Some((next, current)) if next != current => {
                self.review.choice = Some(next);
                self.review_refresh();
            }
            Some(_) => self.notify(Level::Info, "Only one open merge request for this branch".into()),
            None => {}
        }
    }

    /// Marks the selected file viewed, or not.
    fn review_toggle_viewed(&mut self) {
        let files = self.review.files();
        let Some(file) = self.review.view.selected_row(files).map(|i| &files[i]) else {
            return;
        };
        let (Some(key), hash) = (self.review.viewed_key(file), file.hash.clone()) else {
            return;
        };
        let viewed = self.review.viewed.get_or_insert_with(Default::default);
        if viewed.is_viewed(&key, &hash) {
            viewed.clear(&key);
        } else {
            viewed.set(key, hash);
        }
        self.save_viewed();
    }

    pub(super) fn review_command(&mut self, cmd: C) {
        let count = self.review.file_count();
        let page = self.review.view.height.max(1) as isize;
        let (view, files) = self.review.split();
        match cmd {
            C::Up => view.move_selection(files, -1),
            C::Down => view.move_selection(files, 1),
            C::PageUp => view.move_selection(files, -page),
            C::PageDown => view.move_selection(files, page),
            C::First => view.select(files, 0),
            C::Last => view.select(files, count.saturating_sub(1)),
            C::Open => self.review_open_selected(),
            C::ToggleViewed => self.review_toggle_viewed(),
            C::FocusEditor => {
                if !self.group().tabs.is_empty() {
                    self.ui.focus = Focus::Editor;
                }
            }
        }
    }

    /// Opens the selected file's review tab.
    fn review_open_selected(&mut self) {
        if let Some(path) = self.review.view.selected_path().map(String::from) {
            self.review_open_file(&path);
        }
    }

    /// Opens the inline diff of `path` (relative to the work tree, one of the review's files):
    /// the merge base on the left, the work tree on the right; the group's tab already showing it
    /// is focused instead. The file's buffer opens behind the tab (a deleted file has none), so
    /// the language servers know it and edits go through it.
    pub(super) fn review_open_file(&mut self, path: &str) {
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return;
        };
        let Some(review) = self.review.review() else { return };
        let Some(file) = review.files.iter().find(|f| f.path == path) else {
            return;
        };
        let Some(base) = review.merge_base.as_deref().and_then(|b| git2::Oid::from_str(b).ok()) else {
            return self.notify(Level::Error, "The merge base is not known: run git fetch".into());
        };
        let abs = workdir.join(&file.path);
        let old = file.old_path.as_ref().map(|o| workdir.join(o));
        let deleted = file.status == 'D';
        let pair = Pair::review(base);
        self.open_diff_tab(abs.clone(), pair, |id, path, _| DiffTab::review(id, path, base, old));
        if !deleted && self.buffer_of(&abs).is_none() && !self.opening.contains_key(&abs) {
            let request = super::files::OpenRequest {
                background: true,
                ..Default::default()
            };
            self.start_open(abs, request);
        }
    }

    /// A click in the panel: the summary switches merge request, a file row selects.
    pub(super) fn review_click(&mut self, m: MouseEvent) {
        self.ui.focus = Focus::SidePanel;
        let Some(hit) = self.hits.review.clone() else { return };
        let at = Position { x: m.column, y: m.row };
        if hit.summary.contains(at) {
            self.review_switch();
        } else {
            self.review_click_row((m.column, m.row));
        }
    }

    /// A right-click in the panel: the row under it is selected and the panel's menu opens.
    pub(super) fn review_menu(&mut self, at: (u16, u16)) {
        self.review_click_row(at);
        self.ui.focus = Focus::SidePanel;
        self.open_context_menu(
            (at.0, at.1.saturating_add(1)),
            super::context_menu::menu_for(super::context_menu::MenuTarget::Review),
        );
    }

    fn review_click_row(&mut self, at: (u16, u16)) {
        let Some(hit) = self.hits.review.clone() else { return };
        let at = Position { x: at.0, y: at.1 };
        if hit.list.contains(at)
            && let Some(path) = hit.rows.get((at.y - hit.list.y) as usize)
        {
            let path = path.clone();
            let (view, files) = self.review.split();
            view.select_path(files, &path);
            self.review_open_file(&path);
        }
    }

    /// The wheel over the panel.
    pub(super) fn review_scroll(&mut self, delta: isize) {
        let count = self.review.file_count();
        self.review.view.scroll_by(count, delta);
    }
}
