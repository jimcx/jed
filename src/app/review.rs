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
use crate::forge::Side;
use crate::git::Head;
use crate::layout::SidePanel;
use crate::logs::Tool;
use crate::review::{Content, Empty, Outcome, Request};
use crate::review_anchor::Placement;
use crate::side_sections::{Section, dragged_height, split};

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
        self.review.generation += 1;
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
            Outcome::Loaded(mut review, viewed) => {
                // Comments that couldn't be read this time: the last ones of this request stay.
                if review.comments_error.is_some()
                    && let Content::Loaded(old) = &r.content
                    && old.mr.number == review.mr.number
                {
                    review.threads = old.threads.clone();
                    review.conversation = old.conversation.clone();
                }
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
        let len = self.review.review().map_or(0, |r| r.conv_len());
        if self.review.view.conv.focused {
            return self.review_conv_command(cmd, len);
        }
        let page = self.review.view.height.max(1) as isize;
        let (view, files) = self.review.split();
        // Down from the last file enters the conversations.
        let enter = !view.conv.collapsed && len > 0 && view.selected_row(files).is_some_and(|i| i + 1 == files.len());
        match cmd {
            C::Down if enter => {
                view.conv.focused = true;
                view.conv.select(len, 0);
            }
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

    /// The keys while the CONVERSATIONS list has them (`len` rows).
    fn review_conv_command(&mut self, cmd: C, len: usize) {
        let conv = &mut self.review.view.conv;
        let page = conv.height.max(1) as isize;
        match cmd {
            C::Up if conv.selected(len).unwrap_or(0) == 0 => conv.focused = false,
            C::Up => conv.move_selection(len, -1),
            C::Down => conv.move_selection(len, 1),
            C::PageUp => conv.move_selection(len, -page),
            C::PageDown => conv.move_selection(len, page),
            C::First => conv.select(len, 0),
            C::Last => conv.select(len, len.saturating_sub(1)),
            C::Open => self.review_open_conv(),
            C::ToggleViewed => {}
            C::FocusEditor => {
                if !self.group().tabs.is_empty() {
                    self.ui.focus = Focus::Editor;
                }
            }
        }
    }

    /// Opens the selected conversation: a thread's file at its line (the mapped one when placed,
    /// else the nearest to where it was written; a file the review doesn't list just shows the
    /// thread in the status bar), a merge request comment shows its text there.
    fn review_open_conv(&mut self) {
        let Some(review) = self.review.review() else { return };
        let Some(i) = self.review.view.conv.selected(review.conv_len()) else {
            return;
        };
        let Some(t) = review.threads.get(i) else {
            let c = &review.conversation[i - review.threads.len()];
            let line = c.body.lines().next().unwrap_or("");
            let text = format!("{}: {line}", c.author);
            return self.notify(Level::Info, text);
        };
        let a = &t.thread.anchor;
        let (right, line) = match &t.place {
            Placement::Placed { side, lines } => (*side == Side::Right, lines.start),
            _ => (a.side == Side::Right, a.start_line.saturating_sub(1) as usize),
        };
        let first = t.thread.comments.first();
        let text = format!(
            "{}:{} {}: {}",
            a.path,
            a.start_line,
            first.map_or("", |c| c.author.as_str()),
            first.and_then(|c| c.body.lines().next()).unwrap_or("")
        );
        let path = a.path.clone();
        if !review.files.iter().any(|f| f.path == path) {
            return self.notify(Level::Info, text);
        }
        self.review_open_file(&path);
        if let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone())
            && let Some(d) = self.group_mut().active_diff_mut()
            && d.path == workdir.join(&path)
        {
            d.go_to_side_line(right, line);
            self.needs_redraw = true;
        }
    }

    /// Gives each review diff tab the placed threads of its file (a lookup finished, or the tab
    /// is new): plain data, so drawing needs no app state. Outdated and missing threads aren't
    /// shown in the diff (CONVERSATIONS lists them).
    pub(super) fn sync_review_threads(&mut self) {
        let generation = self.review.generation;
        let Some(workdir) = self.git.status.as_ref().map(|s| &s.repo.workdir) else {
            return;
        };
        let review = self.review.review();
        for (_, d) in self.groups.diff_tabs_mut() {
            if !d.pair.is_review() || d.threads.synced == Some(generation) {
                continue;
            }
            d.threads.synced = Some(generation);
            let blocks = review
                .map(|r| {
                    r.threads
                        .iter()
                        .filter(|t| workdir.join(&t.thread.anchor.path) == d.path)
                        .filter_map(|t| {
                            let Placement::Placed { side, lines } = &t.place else {
                                return None;
                            };
                            Some(crate::review_blocks::ThreadBlock {
                                id: t.thread.id.clone(),
                                right: *side == Side::Right,
                                line: lines.end.saturating_sub(1).max(lines.start),
                                resolved: t.thread.resolved,
                                comments: t
                                    .thread
                                    .comments
                                    .iter()
                                    .map(|c| (c.author.clone(), c.body.clone()))
                                    .collect(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            d.threads.set(blocks);
            self.needs_redraw = true;
        }
    }

    /// Next / Previous Thread: the caret to the next / previous thread's line in the active
    /// review diff tab (wrapping), its block in view.
    pub(super) fn review_step_thread(&mut self, forward: bool) {
        let Some(d) = self.group_mut().active_diff_mut().filter(|d| d.pair.is_review()) else {
            return self.notify(Level::Info, "Not in a code review's diff".into());
        };
        if !d.step_thread(forward) {
            self.notify(Level::Info, "No comments on this file".into());
        }
        self.needs_redraw = true;
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
        } else if hit.divider.contains(at) {
            self.drag = Some(super::mouse::Drag::ReviewDivider);
        } else if hit.conv_header.contains(at) {
            let conv = &mut self.review.view.conv;
            conv.focused = true;
            conv.collapsed = !conv.collapsed;
        } else if hit.conv_list.contains(at) {
            let row = hit.conv_top + ((at.y - hit.conv_list.y) / 2) as usize;
            if row < hit.conv_len {
                self.review.view.conv.focused = true;
                self.review.view.conv.select(hit.conv_len, row);
                self.review_open_conv();
            }
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
            self.review.view.conv.focused = false;
            let (view, files) = self.review.split();
            view.select_path(files, &path);
            self.review_open_file(&path);
        }
    }

    /// A drag of the CONVERSATIONS divider with the pointer on `row`: the section reaches the
    /// panel's end (stored as its height, clamped to what the files keep).
    pub(super) fn drag_review_divider(&mut self, row: u16) {
        let Some(body) = self.hits.review.as_ref().map(|h| h.body) else {
            return;
        };
        let conv = &self.review.view.conv;
        let (_, heights) = split(
            body.height,
            &[Section {
                height: conv.section_height,
                collapsed: conv.collapsed,
            }],
        );
        let want = (body.y + body.height).saturating_sub(row);
        let height = dragged_height(body.height, &heights, 0, want);
        let conv = &mut self.review.view.conv;
        conv.section_height = height;
        conv.collapsed = false;
    }

    /// The wheel over the panel, with the pointer at `at`: the conversations or the files.
    pub(super) fn review_scroll(&mut self, delta: isize, at: (u16, u16)) {
        let over_conv = self
            .hits
            .review
            .as_ref()
            .is_some_and(|h| h.conv_list.contains(Position { x: at.0, y: at.1 }));
        if over_conv {
            let len = self.review.review().map_or(0, |r| r.conv_len());
            self.review.view.conv.scroll_by(len, delta.signum());
        } else {
            let count = self.review.file_count();
            self.review.view.scroll_by(count, delta);
        }
    }
}
