//! Writing review comments (`app.composer`, drafts in `app.review.drafts`; models `review_blocks`
//! and `review_drafts`).
//!
//! "Code Review: Add Comment" opens a comment box under the selected lines of a review diff: the
//! composer is a scratch `Buffer` and `View` driven through `edit::run` (as `diff_edit` drives the
//! file's), and `Context::Composer` is active while it is: only Ctrl+Enter / Alt+Enter (save) and
//! Esc (cancel) are bound there, the editor's keys work on its text. Its text reaches the tab as a
//! `Role::Composer` block (`sync_review_threads`), so the box is laid out and drawn like a thread.
//!
//! Saving makes a [`Draft`] in the store, written to the state directory on every change
//! (off-thread, `DraftStore::save`). A draft whose lines no longer read as they did is held back:
//! not drawn in the diff, named in PENDING, and not for Submit Review (later) to post. The drafts
//! are read once (`load_drafts`) and judged against the files on disk after each lookup
//! (`check_drafts`); an open tab judges its file's drafts on its live text.

use super::{App, AppEvent, Focus, Level, review::ReviewEvent};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::{Command, ReviewCommand as C};
use crate::edit;
use crate::layout::{BlockHit, GroupId};
use crate::logs::Tool;
use crate::review_blocks::{self, Role, ThreadBlock};
use crate::review_drafts::{Draft, DraftStore};
use crate::selection::{Selection, Selections};
use crate::view::View;

/// The comment box being written.
pub(super) struct Composer {
    /// The diff tab it is in.
    pub tab: u64,
    pub buf: Buffer,
    pub view: View,
    /// The file, relative to the work tree.
    pub path: String,
    pub right: bool,
    /// The lines, 1-based inclusive.
    pub start: u32,
    pub end: u32,
    /// Their text now.
    pub lines: Vec<String>,
    pub commit: String,
    /// The draft being edited (it stays as it was until saved).
    pub editing: Option<String>,
    /// What saving does: a draft (the default), or a reply / an edit posted at once.
    pub kind: ComposerKind,
}

/// What a comment box writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ComposerKind {
    Draft,
    /// A reply in the thread `thread_id`.
    Reply {
        thread_id: String,
    },
    /// A new text for the user's comment `comment_id`; `range` is its thread's lines when it is
    /// the thread's first comment (some forges keep the range in the body).
    Edit {
        comment_id: String,
        range: Option<(u32, u32)>,
    },
}

/// `line 4` / `lines 4–6`.
fn range_label(start: u32, end: u32) -> String {
    if start == end {
        format!("line {start}")
    } else {
        format!("lines {start}–{end}")
    }
}

/// A draft as a block under its last line.
pub(super) fn draft_block(d: &Draft) -> ThreadBlock {
    ThreadBlock {
        role: Role::Draft,
        id: d.id.clone(),
        right: d.right,
        line: (d.end.max(d.start) - 1) as usize,
        resolved: false,
        mine: false,
        comments: vec![(format!("Draft · {}", range_label(d.start, d.end)), d.body.clone())],
    }
}

impl Composer {
    pub fn text(&self) -> String {
        self.buf.rope.to_string()
    }

    pub fn block(&self) -> ThreadBlock {
        let verb = match (&self.kind, self.editing.is_some()) {
            (ComposerKind::Reply { .. }, _) => "Reply (posted at once)",
            (ComposerKind::Edit { .. }, _) => "Edit your comment (saved at once)",
            (_, true) => "Edit comment",
            _ => "Comment",
        };
        ThreadBlock {
            role: Role::Composer {
                caret: self.view.selections.primary().head.0,
            },
            id: "composer".into(),
            right: self.right,
            line: (self.end.max(self.start) - 1) as usize,
            resolved: false,
            mine: false,
            comments: vec![(format!("{verb} on {}", range_label(self.start, self.end)), self.text())],
        }
    }
}

impl App {
    /// The comment box has the keys: it belongs to the active diff tab and the editor has the focus.
    pub(super) fn composer_active(&self) -> bool {
        self.ui.focus == Focus::Editor
            && self
                .composer
                .as_ref()
                .is_some_and(|c| self.group().active_diff().is_some_and(|d| d.id == c.tab))
    }

    /// Something the review diffs draw changed (a draft, the box).
    pub(super) fn review_blocks_changed(&mut self) {
        self.review.generation += 1;
        self.sync_review_threads();
        self.needs_redraw = true;
    }

    /// The box's text or caret changed (per keystroke): only its own tab shows it, so only that tab
    /// rebuilds its blocks (the other review tabs and the drafts are untouched).
    fn composer_changed(&mut self) {
        if let Some(tab) = self.composer.as_ref().map(|c| c.tab)
            && let Some((_, d)) = self.groups.diff_tabs_mut().find(|(_, d)| d.id == tab)
        {
            d.threads.synced = None;
        }
        self.sync_review_threads();
        self.needs_redraw = true;
    }

    /// "Code Review: Add Comment": a box under the caret's lines of the active review diff.
    pub(super) fn review_comment(&mut self) {
        if self.composer.is_some() {
            if !self.composer_active() {
                self.notify(Level::Info, "Finish the open comment first".into());
            }
            return;
        }
        let Some(d) = self.group().active_diff().filter(|d| d.pair.is_review()) else {
            return self.notify(Level::Info, "Not in a code review's diff".into());
        };
        let (Some(review), Some(workdir)) = (
            self.review.review(),
            self.git.status.as_ref().map(|s| s.repo.workdir.clone()),
        ) else {
            return self.notify(Level::Info, "No merge request to comment on".into());
        };
        let Some(doc) = &d.doc else {
            return self.notify(Level::Info, "The diff is still loading".into());
        };
        if self.review.drafts.store.is_none() {
            return self.notify(Level::Info, "The saved drafts are still being read".into());
        }
        let (lo, hi) = d.caret.ordered();
        let right = d.caret.right;
        // A selection that ends at the start of a line doesn't include it.
        let last = if hi.0 > lo.0 && hi.1 == 0 { hi.0 - 1 } else { hi.0 };
        let side = if right { &doc.right } else { &doc.left };
        if last >= side.len() {
            return self.notify(Level::Info, "Select lines to comment on".into());
        }
        let Ok(path) = d.path.strip_prefix(&workdir) else {
            return self.notify(Level::Info, "The file is outside the work tree".into());
        };
        let commit = if right {
            review.mr.head.sha.clone()
        } else {
            review.merge_base.clone().unwrap_or_default()
        };
        let path = path.to_string_lossy().replace('\\', "/");
        let lines = (lo.0..=last).map(|l| side.line(l).to_string()).collect();
        let (tab, start, end) = (d.id, lo.0 as u32 + 1, last as u32 + 1);
        self.start_composer(Composer {
            tab,
            buf: Buffer::untitled(0),
            view: View::new(),
            path,
            right,
            start,
            end,
            lines,
            commit,
            editing: None,
            kind: ComposerKind::Draft,
        });
    }

    pub(super) fn start_composer(&mut self, c: Composer) {
        self.composer = Some(c);
        self.ui.focus = Focus::Editor;
        if let Some(d) = self.group_mut().active_diff_mut() {
            d.reveal = true;
            d.reveal_rows = 6;
        }
        self.review_blocks_changed();
    }

    /// The draft the panel's PENDING list has selected (when it has the keys), else the one on the
    /// active review diff's caret lines.
    fn target_draft(&self) -> Option<Draft> {
        let rows = self.review.draft_rows();
        if self.review.view.pending.focused {
            let i = self.review.view.pending.selected(rows.len())?;
            return rows.get(i).map(|r| r.draft.clone());
        }
        let d = self.group().active_diff().filter(|d| d.pair.is_review())?;
        let workdir = &self.git.status.as_ref()?.repo.workdir;
        let (lo, hi) = d.caret.ordered();
        rows.iter()
            .map(|r| r.draft)
            .find(|x| {
                x.right == d.caret.right
                    && workdir.join(&x.path) == d.path
                    && (x.start as usize) <= hi.0 + 1
                    && lo.0 < x.end.max(x.start) as usize
            })
            .cloned()
    }

    /// "Code Review: Edit Draft": the draft in the box again.
    pub(super) fn review_edit_draft(&mut self) {
        if self.composer.is_some() {
            return self.notify(Level::Info, "Finish the open comment first".into());
        }
        if self.review.writing {
            return self.notify(Level::Info, "A change is on its way: edit the draft after it".into());
        }
        let Some(draft) = self.target_draft() else {
            return self.notify(Level::Info, "No draft here".into());
        };
        self.review_open_file(&draft.path);
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return;
        };
        let Some(d) = self
            .group()
            .active_diff()
            .filter(|d| d.pair.is_review() && d.path == workdir.join(&draft.path))
        else {
            return self.notify(Level::Info, format!("{} is not in this review", draft.path));
        };
        let tab = d.id;
        let mut buf = Buffer::untitled(0);
        let mut view = View::new();
        edit::paste(&mut buf, &mut view, &draft.body, false, &[]);
        self.review.view.pending.focused = false;
        self.start_composer(Composer {
            tab,
            buf,
            view,
            path: draft.path,
            right: draft.right,
            start: draft.start,
            end: draft.end,
            lines: draft.lines,
            commit: draft.commit,
            editing: Some(draft.id),
            kind: ComposerKind::Draft,
        });
    }

    /// "Code Review: Delete Draft".
    pub(super) fn review_delete_draft(&mut self) {
        let Some(draft) = self.target_draft() else {
            return self.notify(Level::Info, "No draft here".into());
        };
        let key = self.review.drafts_key();
        if let (Some(store), Some(key)) = (self.review.drafts.store.as_mut(), key) {
            store.remove(&key, &draft.id);
        }
        self.review.drafts.drift.remove(&draft.id);
        self.review.drafts.live.remove(&draft.id);
        self.save_drafts();
        let len = self.review.draft_rows().len();
        let p = &mut self.review.view.pending;
        p.select(len, p.selected(len).unwrap_or(0));
        if len == 0 {
            p.focused = false;
        }
        self.review_blocks_changed();
        self.notify(Level::Info, "Draft deleted".into());
    }

    /// A command while the box has the keys: whether it was the box's (moves, edits, select all).
    pub(super) fn composer_command(&mut self, cmd: Command) -> bool {
        let takes = edit::is_edit(cmd)
            || matches!(
                cmd,
                Command::Move { .. } | Command::SelectAll | Command::CancelSelection
            );
        if !takes || !self.composer_active() {
            return false;
        }
        let settings = self.settings.clone();
        if let Some(c) = &mut self.composer {
            edit::run(cmd, &mut c.buf, &mut c.view, &settings);
        }
        self.composer_changed();
        true
    }

    /// A typed character in the box.
    pub(super) fn composer_char(&mut self, ch: char) {
        let settings = self.settings.clone();
        if let Some(c) = &mut self.composer {
            edit::insert_char(&mut c.buf, &mut c.view, ch, &settings);
        }
        self.composer_changed();
    }

    /// Pasted text into the box (false: it isn't active).
    pub(super) fn composer_paste(&mut self, text: &str) -> bool {
        if !self.composer_active() {
            return false;
        }
        let (whole_line, pieces) = self
            .clipboard
            .copied(text)
            .map_or((false, &[][..]), |c| (c.whole_line, c.pieces.as_slice()));
        if let Some(c) = &mut self.composer {
            edit::paste(&mut c.buf, &mut c.view, text, whole_line, pieces);
        }
        self.composer_changed();
        true
    }

    /// Cmd+C / Cmd+X in the box (false: it isn't active).
    pub(super) fn composer_copy(&mut self, cut: bool) -> bool {
        if !self.composer_active() {
            return false;
        }
        if let Some(c) = &mut self.composer {
            let copied = edit::copy_text(&c.buf, &c.view);
            if cut {
                edit::cut(&mut c.buf, &mut c.view);
            }
            self.clipboard.write(copied);
        }
        if cut {
            self.composer_changed();
        }
        true
    }

    /// "Code Review: Comment as Draft": the text becomes a draft (a new one, or the edited
    /// one's new body) and the box closes. An empty comment is not saved.
    pub(super) fn composer_save(&mut self) {
        let Some(c) = &self.composer else { return };
        if c.kind != ComposerKind::Draft {
            return self.composer_post();
        }
        let body = c.text().trim().to_string();
        if body.is_empty() {
            return self.notify(Level::Info, "The comment is empty: write something, or Esc".into());
        }
        // A submit in flight removes its drafts by id once confirmed: an edit now would be lost.
        if c.editing.is_some() && self.review.writing {
            return self.notify(Level::Info, "A change is on its way: save the edit after it".into());
        }
        let Some(key) = self.review.drafts_key() else {
            return self.notify(
                Level::Error,
                "The merge request is not loaded: the comment was kept".into(),
            );
        };
        let Some(store) = self.review.drafts.store.as_mut() else {
            return self.notify(
                Level::Error,
                "The saved drafts are not read yet: the comment was kept".into(),
            );
        };
        let Some(c) = self.composer.take() else { return };
        let edited = c
            .editing
            .as_ref()
            .is_some_and(|id| store.set_body(&key, id, body.clone()));
        if !edited {
            store.add(
                &key,
                Draft::new(c.path, c.right, c.start, c.end, c.commit, c.lines, body),
            );
        }
        self.save_drafts();
        self.review_blocks_changed();
        let n = self.review.draft_rows().len();
        self.notify(Level::Info, format!("Draft saved ({n} pending)"));
    }

    /// Esc: the box closes, its text is dropped (an edited draft stays as it was).
    pub(super) fn composer_cancel(&mut self) {
        if self.composer.take().is_some() {
            self.review_blocks_changed();
        }
    }

    /// A click in a comment box's block `b` of group `g` at screen `(col, row)`: the caret to the
    /// text under it, or a button.
    pub(super) fn composer_click(&mut self, g: GroupId, b: &BlockHit, col: u16, row: u16) {
        let Some(c) = &self.composer else { return };
        let active = self
            .groups
            .get(g)
            .and_then(|g| g.active_diff())
            .is_some_and(|d| d.id == c.tab);
        if !active {
            return;
        }
        let rel = b.first + (row - b.rect.y) as usize;
        let text = c.text();
        let width = (b.rect.width as usize).saturating_sub(2);
        let rows = review_blocks::composer_rows(&text, width).len();
        let x = col.saturating_sub(b.rect.x) as usize;
        if (1..=rows).contains(&rel) {
            let at = review_blocks::composer_byte(&text, rel - 1, x.saturating_sub(2), width);
            if let Some(c) = &mut self.composer {
                c.view.selections = Selections::single(Selection::new(ByteIdx(at), ByteIdx(at)));
            }
            self.composer_changed();
        } else if rel == rows + 1 {
            for (cols, button) in review_blocks::composer_button_cols() {
                if cols.contains(&x) {
                    match button {
                        review_blocks::ComposerButton::Submit => self.composer_save(),
                        review_blocks::ComposerButton::Cancel => self.composer_cancel(),
                    }
                    return;
                }
            }
        }
    }

    /// Reads the saved drafts once (nothing to read without a state directory).
    pub(super) fn load_drafts(&mut self) {
        let d = &mut self.review.drafts;
        if d.store.is_some() || d.loading {
            return;
        }
        let Some(dir) = self.state_dir.clone() else {
            d.store = Some(DraftStore::default());
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        d.loading = true;
        let (tx, log) = (self.tx.clone(), self.logs.tool(Tool::Review));
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(AppEvent::Review(ReviewEvent::Drafts(DraftStore::load(&dir, &log))));
        });
    }

    pub(super) fn drafts_loaded(&mut self, loaded: DraftStore) {
        let d = &mut self.review.drafts;
        d.loading = false;
        match d.store.as_mut() {
            Some(store) => store.merge(loaded),
            None => d.store = Some(loaded),
        }
        self.check_drafts();
    }

    /// Judges the shown merge request's drafts against the files on disk, off-thread.
    pub(super) fn check_drafts(&mut self) {
        let Some(review) = self.review.review() else { return };
        let drafts: Vec<Draft> = match (&self.review.drafts.store, self.review.drafts_key()) {
            (Some(store), Some(key)) => store.get(&key).to_vec(),
            _ => Vec::new(),
        };
        if drafts.is_empty() {
            self.review.drafts.drift.clear();
            return;
        }
        let Some(repo) = self.git.status.as_ref().map(|s| s.repo.clone()) else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let (base, files) = (review.merge_base.clone(), review.files.clone());
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let held = crate::review_drafts::check(&repo, base.as_deref(), &files, &drafts);
            let _ = tx.send(AppEvent::Review(ReviewEvent::DraftsChecked(held)));
        });
    }

    pub(super) fn drafts_checked(&mut self, held: Vec<(String, Option<String>)>) {
        self.review.drafts.drift = held.into_iter().collect();
        // The open tabs say it again from their live text (`sync_review_threads`).
        self.review.drafts.live.clear();
        self.needs_redraw = true;
    }

    /// Writes the drafts to the state directory, off-thread (nothing without one).
    pub(super) fn save_drafts(&self) {
        let (Some(dir), Some(store)) = (self.state_dir.clone(), self.review.drafts.store.clone()) else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let seq = DraftStore::next_seq();
        let log = self.logs.tool(Tool::Review);
        tokio::task::spawn_blocking(move || {
            if let Err(e) = store.save(&dir, seq) {
                log.error(&format!("can't save the drafts: {e}"));
            }
        });
    }

    /// The keys while the PENDING list has them.
    pub(super) fn review_pending_command(&mut self, cmd: C) {
        let len = self.review.draft_rows().len();
        let conv_len = self.review.review().map_or(0, |r| r.conv_len());
        let view = &mut self.review.view;
        let page = view.pending.height.max(1) as isize;
        match cmd {
            C::Up if view.pending.selected(len).unwrap_or(0) == 0 => {
                view.pending.focused = false;
                if conv_len > 0 && !view.conv.collapsed {
                    view.conv.focused = true;
                    view.conv.select(conv_len, conv_len - 1);
                }
            }
            C::Up => view.pending.move_selection(len, -1),
            C::Down => view.pending.move_selection(len, 1),
            C::PageUp => view.pending.move_selection(len, -page),
            C::PageDown => view.pending.move_selection(len, page),
            C::First => view.pending.select(len, 0),
            C::Last => view.pending.select(len, len.saturating_sub(1)),
            C::Open => self.review_open_draft(),
            C::Delete => self.review_delete_draft(),
            C::ToggleViewed => {}
            C::FocusEditor => {
                if !self.group().tabs.is_empty() {
                    self.ui.focus = Focus::Editor;
                }
            }
        }
        if len == 0 {
            self.review.view.pending.focused = false;
        }
    }

    /// Opens the selected draft's file at its first line.
    pub(super) fn review_open_draft(&mut self) {
        let Some(draft) = self.target_draft() else { return };
        let (path, right, line) = (draft.path.clone(), draft.right, draft.start.saturating_sub(1) as usize);
        let in_review = self
            .review
            .review()
            .is_some_and(|r| r.files.iter().any(|f| f.path == path));
        if !in_review {
            return self.notify(
                Level::Info,
                format!("{}: {}", draft.place(), draft.body.lines().next().unwrap_or("")),
            );
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
}

#[cfg(test)]
impl App {
    /// The comment box's text, when one is open.
    pub fn test_composer_text(&self) -> Option<String> {
        self.composer.as_ref().map(|c| c.text())
    }
}
