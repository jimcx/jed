//! The Code Review panel (`app.review`, model `crate::review`, drawn by `ui::review`): when it looks
//! the merge request up, what it does with the answer, and the file list's keys and mouse.
//!
//! A lookup (`review::lookup`) runs in `spawn_blocking` and comes back as
//! `AppEvent::Review(ReviewEvent::Looked)`. It starts at launch and whenever HEAD moves
//! (`review_head_check`, from the git status handlers), when the panel is shown, on "Code Review:
//! Refresh" and every `review.interval` (60 s) while the panel is on screen or a review diff / thread tab is active (`review_deadline`).
//! One runs at a time; a request meanwhile is queued. A failed lookup keeps the last result on
//! screen under its error; the viewed marks are written off-thread when they change.

use crossterm::event::MouseEvent;
use ratatui::layout::Position;

use super::context_menu::ReviewSpot;
use super::{App, AppEvent, Focus, Level};
use crate::command::ReviewCommand as C;
use crate::diff_view::{DiffTab, Pair};
use crate::forge::Side;
use crate::git::Head;
use crate::group::Tab;
use crate::layout::SidePanel;
use crate::logs::Tool;
use crate::review::{Content, Empty, Outcome, Pane, Request};
use crate::review_anchor::Placement;
use crate::side_sections::{dragged_height, split, split_collapsed};

/// Results of the panel's background work.
pub(super) enum ReviewEvent {
    Looked(Outcome),
    /// The saved drafts, read.
    Drafts(crate::review_drafts::DraftStore),
    /// Which drafts the files on disk hold back (by id).
    DraftsChecked(Vec<(String, Option<String>)>),
    /// A write to the forge finished (`app/review_write.rs`).
    Wrote(crate::review_write::Done),
    /// The commit's text under a suggestion (`app/review_suggest.rs`).
    Suggestion(super::review_suggest::Staged),
}

impl App {
    /// The panel is on screen, or a review diff or thread tab is the active tab of any group.
    fn review_visible(&self) -> bool {
        (self.ui.side_visible && self.ui.side_panel == SidePanel::Review)
            || self.groups.ids().into_iter().any(|g| {
                self.groups
                    .get(g)
                    .and_then(|g| g.active_diff())
                    .is_some_and(|d| d.pair.is_review() || d.thread.is_some())
            })
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
        self.load_drafts();
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
        self.review.drafts.looked_writes = self.review.drafts.forge_writes;
        let req = Request {
            repo: status.repo.clone(),
            config: self.review.config.clone(),
            env: self.review.env.clone(),
            choose: self.review.choice,
            viewed_from: self.review.viewed.is_none().then(|| self.state_dir.clone()).flatten(),
            log: self.logs.tool(Tool::Review),
            blobs: self.review.blobs.clone(),
            forge_drafts: self.settings.review_drafts_on_forge,
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(AppEvent::Review(ReviewEvent::Looked(crate::review::lookup(req))));
        });
    }

    pub(super) fn handle_review_event(&mut self, ev: ReviewEvent) {
        self.review.generation += 1;
        let outcome = match ev {
            ReviewEvent::Looked(outcome) => outcome,
            ReviewEvent::Drafts(loaded) => return self.drafts_loaded(loaded),
            ReviewEvent::DraftsChecked(held) => return self.drafts_checked(held),
            ReviewEvent::Wrote(done) => return self.review_wrote(done),
            ReviewEvent::Suggestion(staged) => return self.suggestion_read(staged),
        };
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
                r.fetched_at = Some(std::time::SystemTime::now());
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
        // The lookup placed the threads on the files on disk: the open review diffs place them on
        // the text they show again now, and load again for the commits of new threads.
        self.review_live_places();
        for (_, d) in self.groups.diff_tabs_mut() {
            if d.pair.is_review() {
                d.mark_stale();
            }
        }
        self.reconcile_drafts();
        self.check_drafts();
        self.review_fetch_user();
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
        if self.review.view.pending.focused {
            return self.review_pending_command(cmd);
        }
        if self.review.view.conv.focused {
            return self.review_conv_command(cmd, len);
        }
        let page = self.review.view.height.max(1) as isize;
        let pending = self.review.draft_rows().len();
        if self.review.view.summary {
            return self.review_summary_command(cmd);
        }
        let (view, files) = self.review.split();
        if view.files_collapsed || view.files_header {
            return self.review_files_header_command(cmd, len, pending);
        }
        // Down from the last file enters the conversations, else the pending drafts.
        let last = view.selected_row(files).is_some_and(|i| i + 1 == files.len());
        let enter = !view.conv.collapsed && len > 0 && last;
        match cmd {
            C::Down if enter => {
                view.conv.focused = true;
                view.conv.select(len, 0);
            }
            C::Down if last && len == 0 && pending > 0 => {
                view.pending.focused = true;
                view.pending.select(pending, 0);
            }
            C::Delete => {}
            C::Up if view.selected_row(files).is_none_or(|i| i == 0) => view.files_header = true,
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

    /// The keys while the FILES header has the selection (always while FILES is collapsed): Enter
    /// and Space toggle it, Down leaves for the first file (collapsed: the next section).
    fn review_files_header_command(&mut self, cmd: C, len: usize, pending: usize) {
        let view = &mut self.review.view;
        match cmd {
            C::Open | C::ToggleViewed => self.review_toggle_files(),
            C::Down if view.files_collapsed && len > 0 && !view.conv.collapsed => {
                view.conv.focused = true;
                view.conv.select(len, 0);
            }
            C::Down if view.files_collapsed && pending > 0 => {
                view.pending.focused = true;
                view.pending.select(pending, 0);
            }
            C::Down if !view.files_collapsed => {
                view.files_header = false;
                let (view, files) = self.review.split();
                view.select(files, 0);
            }
            C::Up => view.summary = true,
            C::FocusEditor if !self.group().tabs.is_empty() => self.ui.focus = Focus::Editor,
            _ => {}
        }
    }

    /// The keys while the summary row has the selection: Enter opens the conversation, Down
    /// goes on to the FILES header.
    fn review_summary_command(&mut self, cmd: C) {
        match cmd {
            C::Open => self.review_open_conversation(None),
            C::Down => {
                let view = &mut self.review.view;
                view.summary = false;
                view.files_header = true;
            }
            C::FocusEditor if !self.group().tabs.is_empty() => self.ui.focus = Focus::Editor,
            _ => {}
        }
    }

    /// Collapses or expands FILES; its header has the selection (and the keys) afterwards.
    pub(super) fn review_toggle_files(&mut self) {
        let view = &mut self.review.view;
        view.files_collapsed = !view.files_collapsed;
        view.files_header = true;
        view.conv.focused = false;
        view.pending.focused = false;
    }

    /// The keys while the CONVERSATIONS list has them (`len` rows).
    fn review_conv_command(&mut self, cmd: C, len: usize) {
        let page = self.review.view.conv.height.max(1) as isize;
        let pending = self.review.draft_rows().len();
        let conv = &mut self.review.view.conv;
        match cmd {
            C::Up if conv.selected(len).unwrap_or(0) == 0 => conv.focused = false,
            C::Up => conv.move_selection(len, -1),
            C::Down if pending > 0 && conv.selected(len).is_some_and(|i| i + 1 == len) => {
                conv.focused = false;
                self.review.view.pending.focused = true;
                self.review.view.pending.select(pending, 0);
            }
            C::Down => conv.move_selection(len, 1),
            C::Delete => {}
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

    /// Opens the selected conversation: a placed thread's file at its line; any other thread (outdated,
    /// missing, in a file the review doesn't list) in its thread tab; a merge request comment
    /// shows its text in the status bar.
    pub(super) fn review_open_conv(&mut self) {
        let Some(review) = self.review.review() else { return };
        let Some(i) = self.review.view.conv.selected(review.conv_len()) else {
            return;
        };
        let Some(t) = review.threads.get(i) else {
            return self.review_open_conversation(Some(i - review.threads.len()));
        };
        let a = &t.thread.anchor;
        let (right, line) = match &t.place {
            Placement::Placed { side, lines } => (*side == Side::Right, lines.start),
            _ => (a.side == Side::Right, a.start_line.saturating_sub(1) as usize),
        };
        let path = a.path.clone();
        // Only a thread placed in a file the review lists belongs to that file's diff.
        if !matches!(t.place, Placement::Placed { .. }) || !review.files.iter().any(|f| f.path == path) {
            let row = t.clone();
            return self.open_thread_tab(&row);
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

    /// Opens (or shows) the merge request's conversation tab, one per merge request in the
    /// active group: the description, then every comment. `reveal` is the comment (an index into
    /// `Review::conversation`) the caret goes to.
    pub(super) fn review_open_conversation(&mut self, reveal: Option<usize>) {
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return;
        };
        let Some(review) = self.review.review() else {
            return self.notify(Level::Info, "No merge request to show".into());
        };
        let (number, rows) = (review.mr.number, review.conversation.len() + 1);
        let title = format!("!{number} Conversation");
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let existing = self
            .group()
            .tabs
            .iter()
            .position(|t| t.diff().and_then(|d| d.conv.as_ref()).is_some());
        match existing {
            Some(i) => self.group_mut().active = i,
            None => {
                let id = self.next_diff_id();
                let conv = crate::diff_view::ConvTab { number, title, rows };
                let tab = DiffTab::conversation(id, workdir.join(format!("!{number} Conversation")), conv);
                self.group_mut().open_tab(Tab::Diff(Box::new(tab)));
            }
        }
        if let Some(d) = self.group_mut().active_diff_mut() {
            d.go_to_side_line(true, reveal.map_or(0, |i| i + 1));
        }
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
        self.sync_diffs();
        self.sync_review_threads();
        self.side_opened();
    }

    /// "Code Review: Open Thread": the selected thread's tab, whatever its placement.
    pub(super) fn review_open_thread(&mut self) {
        if let Some(row) = self.review_need_target() {
            self.open_thread_tab(&row);
        }
    }

    /// Opens (or shows the active group's) thread tab of `row`: its code, when known, and its
    /// block; read-only, never the file's own diff.
    fn open_thread_tab(&mut self, row: &crate::review::ThreadRow) {
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return;
        };
        let a = &row.thread.anchor;
        let (base, last, code) = match &row.code {
            Some(c) => (c.first - 1, c.last, c.text.clone()),
            None => (
                a.end_line.saturating_sub(1) as usize,
                0,
                "(the code isn't available)\n".to_string(),
            ),
        };
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let existing = self.group().tabs.iter().position(|t| {
            t.diff()
                .and_then(|d| d.thread.as_ref())
                .is_some_and(|t| t.id == row.thread.id)
        });
        match existing {
            Some(i) => self.group_mut().active = i,
            None => {
                let id = self.next_diff_id();
                let thread = crate::diff_view::ThreadTab {
                    id: row.thread.id.clone(),
                    title: format!("Thread {}:{}", a.path, a.end_line),
                    base,
                    last,
                    has_code: row.code.is_some(),
                };
                let tab = DiffTab::thread(id, workdir.join(&a.path), thread, code);
                self.group_mut().open_tab(Tab::Diff(Box::new(tab)));
            }
        }
        if let Some(d) = self.group_mut().active_diff_mut() {
            d.go_to_side_line(true, last);
        }
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
        self.sync_diffs();
        self.sync_review_threads();
    }

    /// Gives each review diff tab the placed threads of its file (a lookup finished, or the tab
    /// is new): plain data, so drawing needs no app state. Outdated and missing threads aren't
    /// shown in the diff (CONVERSATIONS lists them).
    pub(super) fn sync_review_threads(&mut self) {
        let generation = self.review.generation;
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return;
        };
        // The comment box of a tab that closed goes with it.
        if let Some(c) = &self.composer
            && !self.groups.diff_tabs_mut().any(|(_, d)| d.id == c.tab)
        {
            self.composer = None;
        }
        // Called after every diff event: nothing to build (or clone) while every tab is current.
        if !self.groups.diff_tabs_mut().any(|(_, d)| {
            (d.pair.is_review() || d.pair.is_thread()) && d.threads.synced != Some((generation, d.doc_key()))
        }) {
            return;
        }
        let drafts: &[crate::review_drafts::Draft] = match (&self.review.drafts.store, self.review.drafts_key()) {
            (Some(store), Some(key)) => store.get(&key),
            _ => &[],
        };
        let label = self.submit_label();
        let composer = self.composer.as_ref().map(|c| (c.tab, c.block(label)));
        let editing = self.composer.as_ref().and_then(|c| c.editing.clone());
        let mut live = Vec::new();
        let review = self.review.review();
        let user = self.review.user.as_deref();
        for (_, d) in self.groups.diff_tabs_mut() {
            let stamp = (generation, d.doc_key());
            if !(d.pair.is_review() || d.pair.is_thread()) || d.threads.synced == Some(stamp) {
                continue;
            }
            d.threads.synced = Some(stamp);
            if d.conv.is_some() {
                // The merge request's conversation: one block under each of the tab's rows.
                if let Some(r) = review {
                    let mut blocks = conversation_blocks(r, user);
                    // The box (a new comment, an edit) is one more row, at the end.
                    let boxed = composer.as_ref().filter(|(tab, _)| *tab == d.id);
                    let rows = blocks.len() + usize::from(boxed.is_some());
                    if let Some((_, b)) = boxed {
                        let mut b = b.clone();
                        (b.right, b.line) = (true, blocks.len());
                        blocks.push(b);
                    }
                    d.set_conversation(format!("!{} Conversation", r.mr.number), r.mr.number, rows);
                    d.threads.set(blocks);
                    self.needs_redraw = true;
                }
                continue;
            }
            let mut blocks: Vec<crate::review_blocks::ThreadBlock> = review
                .map(|r| {
                    r.threads
                        .iter()
                        .filter_map(|t| match (&d.thread, &t.place) {
                            // A thread tab: its one thread, under the code's last anchored line.
                            (Some(tab), _) => (tab.id == t.thread.id).then(|| {
                                let a = &t.thread.anchor;
                                let span = a.end_line.saturating_sub(a.start_line) as usize;
                                forge_block(t, user, true, (tab.last, span), false, r.kind)
                            }),
                            (None, Placement::Placed { side, lines })
                                if workdir.join(&t.thread.anchor.path) == d.path =>
                            {
                                let apply = *side == Side::Right
                                    && !t.thread.resolved
                                    && t.thread
                                        .comments
                                        .iter()
                                        .any(|c| !crate::suggestion::parse(&c.body, &t.thread.anchor).is_empty());
                                let line = lines.end.saturating_sub(1).max(lines.start);
                                let span = line - lines.start;
                                Some(forge_block(t, user, *side == Side::Right, (line, span), apply, r.kind))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Drafts of this file whose lines still read as they did; the others are held back
            // (PENDING names them). A diff not loaded yet judges nothing.
            if let Some(doc) = &d.doc {
                for draft in drafts.iter().filter(|x| workdir.join(&x.path) == d.path) {
                    if draft.reply_to.is_some() {
                        // Under its thread's block, wherever the thread is placed now.
                        let placed = match &d.thread {
                            Some(tab) => (draft.reply_to.as_ref() == Some(&tab.id)).then_some((Side::Right, tab.last)),
                            None => review.and_then(|r| {
                                r.threads.iter().find_map(|t| match &t.place {
                                    Placement::Placed { side, lines }
                                        if Some(&t.thread.id) == draft.reply_to.as_ref() =>
                                    {
                                        Some((*side, lines.end.saturating_sub(1).max(lines.start)))
                                    }
                                    _ => None,
                                })
                            }),
                        };
                        if let Some((side, line)) = placed
                            && editing.as_deref() != Some(draft.id.as_str())
                        {
                            let mut b = super::review_comment::draft_block(draft);
                            b.right = side == Side::Right;
                            b.line = line;
                            blocks.push(b);
                        }
                        continue;
                    }
                    if d.thread.is_some() {
                        continue;
                    }
                    let lines = if draft.right { &doc.right } else { &doc.left };
                    // A right-side draft's lines are counted in its commit: where they are in the
                    // text shown (moved by local edits; a commit changing them holds it back).
                    let placed = if draft.right { draft.place_in(&doc.live) } else { None };
                    let (first, held) = match placed {
                        Some(Some(at)) => (Some(at.start), None),
                        Some(None) => (None, Some(draft.changed())),
                        None => {
                            let first = (draft.start.max(1) - 1) as usize;
                            let held = draft.drift_from(first, |i| (i < lines.len()).then(|| lines.line(i)));
                            (Some(first), held)
                        }
                    };
                    live.push((draft.id.clone(), held.map(String::from), first));
                    if held.is_none() && editing.as_deref() != Some(draft.id.as_str()) {
                        let mut b = super::review_comment::draft_block(draft);
                        if let Some(first) = first {
                            b.line = first + (draft.end.max(draft.start) - draft.start) as usize;
                        }
                        blocks.push(b);
                    }
                }
            }
            if let Some((tab, block)) = &composer
                && *tab == d.id
            {
                let mut block = block.clone();
                if let Some(tab) = &d.thread {
                    (block.right, block.line) = (true, tab.last);
                }
                blocks.push(block);
            }
            d.threads.set(blocks);
            self.needs_redraw = true;
        }
        for (id, held, first) in live {
            self.review.drafts.live.insert(id.clone(), held);
            match first {
                Some(at) => self.review.drafts.at.insert(id, at),
                None => self.review.drafts.at.remove(&id),
            };
        }
    }

    /// Places the right-side threads of each open review diff's file on the text it shows (its
    /// last load's `Doc::live`): lines added or removed above a thread move it, a thread whose
    /// lines were edited stays on them, and only a commit changing them outdates it. A thread on a
    /// commit the tab didn't compare (one the clone lacks) keeps the lookup's place. The tabs lay
    /// their blocks out again when a place changed.
    pub(super) fn review_live_places(&mut self) {
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return;
        };
        let Content::Loaded(review) = &mut self.review.content else {
            return;
        };
        let mut changed = false;
        for (_, d) in self.groups.diff_tabs_mut() {
            let Some(doc) = d.doc.as_ref().filter(|_| d.pair.is_review()) else {
                continue;
            };
            for t in &mut review.threads {
                let a = &t.thread.anchor;
                if a.side != Side::Right || workdir.join(&a.path) != d.path {
                    continue;
                }
                let Some(place) = doc.live.place(&a.commit, a.start_line, a.end_line) else {
                    continue;
                };
                if t.place != place {
                    t.place = place;
                    changed = true;
                }
            }
        }
        if changed {
            self.review.generation += 1;
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
        self.side_opened();
    }

    /// A click in the panel: the summary switches merge request, a file row selects.
    pub(super) fn review_click(&mut self, m: MouseEvent) {
        self.ui.focus = Focus::SidePanel;
        let Some(hit) = self.hits.review.clone() else { return };
        let at = Position { x: m.column, y: m.row };
        self.review.view.summary = false;
        if hit.switcher.contains(at) {
            self.review_switch();
        } else if hit.summary.contains(at) {
            let view = &mut self.review.view;
            (view.summary, view.files_header) = (true, true);
            view.conv.focused = false;
            view.pending.focused = false;
            self.review_open_conversation(None);
        } else if let Some((_, pane)) = hit.dividers.iter().find(|d| d.0.contains(at)) {
            self.drag = Some(super::mouse::Drag::ReviewDivider(*pane));
        } else if let Some((_, command)) = hit.pending_buttons.iter().find(|b| b.0.contains(at)) {
            self.execute(*command);
        } else if hit.files_header.contains(at) {
            self.review_toggle_files();
        } else if hit.conv_header.contains(at) {
            self.review.view.files_header = false;
            let conv = &mut self.review.view.conv;
            conv.focused = true;
            conv.collapsed = !conv.collapsed;
        } else if hit.conv_list.contains(at) {
            let row = hit.conv_top + ((at.y - hit.conv_list.y) / 2) as usize;
            if row < hit.conv_len {
                self.review.view.pending.focused = false;
                self.review.view.conv.focused = true;
                self.review.view.conv.select(hit.conv_len, row);
                self.review_open_conv();
            }
        } else if hit.pending_header.contains(at) {
            self.review.view.conv.focused = false;
            let pending = &mut self.review.view.pending;
            pending.focused = true;
            pending.collapsed = !pending.collapsed;
        } else if hit.pending_list.contains(at) {
            let row = hit.pending_top + (at.y - hit.pending_list.y) as usize;
            if row < hit.pending_len {
                self.review.view.conv.focused = false;
                self.review.view.pending.focused = true;
                self.review.view.pending.select(hit.pending_len, row);
                self.review_open_draft();
            }
        } else {
            self.review.view.files_header = false;
            self.review_click_row((m.column, m.row));
        }
    }

    /// A right-click in the panel: the row under it is selected and focused (nothing opens but a
    /// file row's diff, as a click does) and the menu for what it is opens.
    pub(super) fn review_menu(&mut self, at: (u16, u16)) {
        let spot = self.review_spot(at);
        self.ui.focus = Focus::SidePanel;
        self.open_context_menu(
            (at.0, at.1.saturating_add(1)),
            super::context_menu::menu_for(super::context_menu::MenuTarget::Review(spot)),
        );
    }

    /// What is at `at` in the panel, that row selected and given the keys.
    fn review_spot(&mut self, at: (u16, u16)) -> ReviewSpot {
        let Some(hit) = self.hits.review.clone() else {
            return ReviewSpot::Panel {
                withdraw: self.can_withdraw_own(),
            };
        };
        let p = Position { x: at.0, y: at.1 };
        if hit.list.contains(p) && hit.rows.get((p.y - hit.list.y) as usize).is_some() {
            self.review_click_row(at);
            return ReviewSpot::File;
        }
        if hit.conv_list.contains(p) {
            let row = hit.conv_top + ((p.y - hit.conv_list.y) / 2) as usize;
            if row < hit.conv_len {
                let view = &mut self.review.view;
                view.pending.focused = false;
                view.conv.focused = true;
                view.conv.select(hit.conv_len, row);
                return self.conv_spot(row);
            }
        }
        if hit.pending_list.contains(p) {
            let row = hit.pending_top + (p.y - hit.pending_list.y) as usize;
            if row < hit.pending_len {
                let view = &mut self.review.view;
                view.conv.focused = false;
                view.pending.focused = true;
                view.pending.select(hit.pending_len, row);
                return ReviewSpot::Draft;
            }
        }
        if hit.pending_header.contains(p) || hit.pending_buttons.iter().any(|b| b.0.contains(p)) {
            return ReviewSpot::Pending;
        }
        ReviewSpot::Panel {
            withdraw: self.can_withdraw_own(),
        }
    }

    /// CONVERSATIONS row `row`: a thread (with whether the user wrote in it and whether its
    /// suggestion can be applied) or a merge request comment.
    fn conv_spot(&self, row: usize) -> ReviewSpot {
        let Some(t) = self.review.review().and_then(|r| r.threads.get(row)) else {
            return ReviewSpot::Comment;
        };
        let own = self
            .review
            .user
            .as_ref()
            .is_none_or(|user| t.thread.comments.iter().any(|c| c.author.eq_ignore_ascii_case(user)));
        let placed_right = matches!(&t.place, Placement::Placed { side: Side::Right, .. });
        let suggestion = placed_right
            && !t.thread.resolved
            && t.thread
                .comments
                .iter()
                .any(|c| !crate::suggestion::parse(&c.body, &t.thread.anchor).is_empty());
        ReviewSpot::Thread { own, suggestion }
    }

    fn review_click_row(&mut self, at: (u16, u16)) {
        let Some(hit) = self.hits.review.clone() else { return };
        let at = Position { x: at.0, y: at.1 };
        if hit.list.contains(at)
            && let Some(path) = hit.rows.get((at.y - hit.list.y) as usize)
        {
            let path = path.clone();
            self.review.view.conv.focused = false;
            self.review.view.pending.focused = false;
            let (view, files) = self.review.split();
            view.select_path(files, &path);
            self.review_open_file(&path);
        }
    }

    /// A drag of `pane`'s divider with the pointer on `row`: the section's top moves to `row`
    /// (stored as its height, clamped so the files and the other section keep their rows).
    pub(super) fn drag_review_divider(&mut self, pane: Pane, row: u16) {
        let Some(body) = self.hits.review.as_ref().map(|h| h.body) else {
            return;
        };
        let sections = self.review.sections();
        let Some(i) = sections.iter().position(|s| s.0 == pane) else {
            return;
        };
        let wishes = sections.iter().map(|s| s.1).collect::<Vec<_>>();
        let (_, heights) = if self.review.view.files_collapsed {
            split_collapsed(body.height, &wishes)
        } else {
            split(body.height, &wishes)
        };
        let below: u16 = heights[i + 1..].iter().sum();
        let want = (body.y + body.height).saturating_sub(below).saturating_sub(row);
        let height = dragged_height(body.height, &heights, i, want);
        let section = match pane {
            Pane::Conversations => &mut self.review.view.conv,
            Pane::Pending => &mut self.review.view.pending,
        };
        section.section_height = height;
        section.collapsed = false;
    }

    /// Whether `at` is on one of the panel's section dividers (the pointer shows a resize).
    pub(super) fn review_divider_at(&self, at: Position) -> bool {
        self.hits
            .review
            .as_ref()
            .is_some_and(|h| h.dividers.iter().any(|d| d.0.contains(at)))
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

/// The blocks of a merge request's conversation tab: the description (headed by its author and the
/// title), then each comment and review summary, block `i` under row `i`.
fn conversation_blocks(r: &crate::review::Review, user: Option<&str>) -> Vec<crate::review_blocks::ThreadBlock> {
    let text = |body: &str, empty: &str| {
        if body.trim().is_empty() {
            empty.to_string()
        } else {
            body.to_string()
        }
    };
    let mut items = vec![(
        "description".to_string(),
        format!("{} · {}", r.mr.author, r.mr.title),
        text(&r.mr.body, "(no description)"),
        None,
    )];
    items.extend(r.conversation.iter().map(|c| {
        (
            c.id.clone(),
            c.author.clone(),
            text(&c.body, "(no text)"),
            crate::review_blocks::Stamp::of(c),
        )
    }));
    items
        .into_iter()
        .enumerate()
        .map(|(i, (id, head, body, stamp))| crate::review_blocks::ThreadBlock {
            role: crate::review_blocks::Role::Comment,
            // Edit / delete: the user's own comments; a review summary (`review-N`) and the
            // description are not comments the forge lets us change.
            mine: !id.starts_with("review-")
                && user.is_some_and(|u| {
                    r.conversation
                        .iter()
                        .any(|c| c.id == id && c.author.eq_ignore_ascii_case(u))
                }),
            id,
            right: true,
            line: i,
            span: 0,
            resolved: false,
            apply: false,
            comments: vec![(head, body)],
            times: vec![stamp],
            outdated_on: None,
        })
        .collect()
}

/// A forge thread's block under `line` of a diff's text, covering `span` lines above it too
/// (`apply`: it may offer Apply Suggestion).
fn forge_block(
    t: &crate::review::ThreadRow,
    user: Option<&str>,
    right: bool,
    (line, span): (usize, usize),
    apply: bool,
    kind: crate::forge::Kind,
) -> crate::review_blocks::ThreadBlock {
    crate::review_blocks::ThreadBlock {
        role: crate::review_blocks::Role::Thread,
        id: t.thread.id.clone(),
        right,
        line,
        span,
        resolved: t.thread.resolved,
        mine: user.is_some_and(|u| t.thread.comments.iter().any(|c| c.author.eq_ignore_ascii_case(u))),
        apply,
        comments: t
            .thread
            .comments
            .iter()
            .map(|c| (c.author.clone(), c.body.clone()))
            .collect(),
        times: t.thread.comments.iter().map(crate::review_blocks::Stamp::of).collect(),
        outdated_on: t.outdated_on(kind),
    }
}
