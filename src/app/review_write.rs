//! Writing to the forge from the Code Review panel (model `crate::review_write`): Submit Review,
//! Reply, Resolve / Unresolve, Edit and Delete of the user's own comments.
//!
//! Every write is a [`Job`] run in `spawn_blocking`; its [`Done`] comes back as
//! `ReviewEvent::Wrote` (`review_wrote`). One runs at a time (`review.writing`). Drafts are
//! removed from the store only for the ids a submit confirmed; a failure keeps them, names the
//! error on their PENDING row (`drafts.errors`) and in the Review tool log. A submit needs local
//! HEAD to be the merge request's head (`Review::head_banner` says why not). After a write that
//! worked the panel looks the merge request up again.

use super::prompt::Prompt;
use super::review::ReviewEvent;
use super::review_comment::{Composer, ComposerKind};
use super::{App, AppEvent, Level};
use crate::buffer::Buffer;
use crate::edit;
use crate::forge::{self, DraftComment, DraftReview, Side};
use crate::layout::{BlockHit, GroupId};
use crate::logs::Tool;
use crate::review::ThreadRow;
use crate::review_anchor::Placement;
use crate::review_blocks::{self, ThreadButton};
use crate::review_write::{self, Done, Job, Request};
use crate::view::View;

impl App {
    /// Asks the forge who the token belongs to (once per session, after the first lookup).
    pub(super) fn review_fetch_user(&mut self) {
        let r = &self.review;
        if r.user.is_some() || r.user_loading || r.review().is_none() {
            return;
        }
        if self.review_start(Job::User) {
            self.review.user_loading = true;
        }
    }

    /// Runs `job` off-thread (false: nothing started).
    fn review_start(&mut self, job: Job) -> bool {
        let Some(review) = self.review.review() else {
            return false;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return false;
        }
        let req = Request {
            repo: review.repo.clone(),
            config: self.review.config.clone(),
            env: self.review.env.clone(),
            log: self.logs.tool(Tool::Review),
        };
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(AppEvent::Review(ReviewEvent::Wrote(review_write::run(req, job))));
        });
        true
    }

    /// Starts a write (one at a time).
    fn review_write(&mut self, job: Job) -> bool {
        if self.review.writing {
            self.notify(Level::Info, "Another change is still on its way".into());
            return false;
        }
        if self.review.review().is_none() {
            self.notify(Level::Info, "No merge request".into());
            return false;
        }
        let started = self.review_start(job);
        self.review.writing = started;
        started
    }

    /// A write finished.
    pub(super) fn review_wrote(&mut self, done: Done) {
        match done {
            Done::User(u) => {
                self.review.user = Some(u);
                self.review.user_loading = false;
                self.review_blocks_changed();
            }
            Done::Submitted { posted, failed, error } => {
                self.review.writing = false;
                self.submitted(posted, failed, error);
            }
            Done::Wrote(job) => {
                self.review.writing = false;
                if matches!(job, Job::Reply { .. } | Job::Edit { .. }) {
                    self.composer = None;
                    self.review_blocks_changed();
                }
                self.notify(Level::Info, job.done_text().into());
                self.review_refresh();
            }
            Done::Failed { job: Job::User, .. } => self.review.user_loading = false,
            Done::Failed { error, .. } => {
                self.review.writing = false;
                let text = format!("The forge refused it: {error}");
                self.review.error = Some(text.clone());
                self.notify(Level::Error, text);
            }
        }
    }

    /// The forge answered a submit: confirmed drafts go, the others stay with their error.
    fn submitted(&mut self, posted: Vec<String>, failed: Vec<(String, String)>, error: Option<String>) {
        let key = self.review.drafts_key();
        if let (Some(store), Some(key)) = (self.review.drafts.store.as_mut(), key) {
            for id in &posted {
                store.remove(&key, id);
            }
        }
        for id in &posted {
            self.review.drafts.drift.remove(id);
            self.review.drafts.live.remove(id);
        }
        self.review.drafts.errors = failed.iter().cloned().collect();
        if !posted.is_empty() {
            self.save_drafts();
        }
        let len = self.review.draft_rows().len();
        let p = &mut self.review.view.pending;
        p.select(len, p.selected(len).unwrap_or(0));
        if len == 0 {
            p.focused = false;
        }
        self.review_blocks_changed();
        if failed.is_empty() && error.is_none() {
            self.notify(Level::Info, format!("Review submitted ({} comments)", posted.len()));
            self.review_refresh();
        } else {
            let first = error
                .or_else(|| failed.first().map(|f| f.1.clone()))
                .unwrap_or_default();
            let text = format!("{} draft(s) not posted, kept: {first}", failed.len());
            self.review.error = Some(text.clone());
            self.notify(Level::Error, text);
            if !posted.is_empty() {
                self.review_refresh();
            }
        }
    }

    /// "Code Review: Submit Review (...)": every draft that isn't held back, with `event`.
    pub(super) fn review_submit(&mut self, event: forge::ReviewEvent) {
        let Some(review) = self.review.review() else {
            return self.notify(Level::Info, "No merge request to submit".into());
        };
        let local = self.git.status.as_ref().and_then(|s| s.head_id.as_deref());
        if let Some(banner) = review.head_banner(local) {
            return self.notify(Level::Error, format!("Can't submit: {banner}"));
        }
        let rows = self.review.draft_rows();
        let held = rows.iter().filter(|r| r.held.is_some()).count();
        let sent: Vec<_> = rows.iter().filter(|r| r.held.is_none()).collect();
        if sent.is_empty() && event == forge::ReviewEvent::Comment {
            let why = if held > 0 {
                format!("{held} draft(s) are held back: their lines changed")
            } else {
                "No drafts to submit".into()
            };
            return self.notify(Level::Info, why);
        }
        let job = Job::Submit {
            number: review.mr.number,
            head_sha: review.mr.head.sha.clone(),
            review: DraftReview {
                body: String::new(),
                event,
                comments: sent
                    .iter()
                    .map(|r| DraftComment {
                        anchor: r.draft.anchor(),
                        body: r.draft.body.clone(),
                    })
                    .collect(),
            },
            ids: sent.iter().map(|r| r.draft.id.clone()).collect(),
        };
        let n = sent.len();
        let held_note = if held > 0 {
            format!(", {held} held back")
        } else {
            String::new()
        };
        if self.review_write(job) {
            self.review.drafts.errors.clear();
            self.notify(Level::Info, format!("Submitting {n} comment(s){held_note}…"));
        }
    }

    /// The thread the command is about: the selected CONVERSATIONS row, else the thread block at
    /// the active review diff's caret.
    fn review_target(&mut self) -> Option<ThreadRow> {
        let review = self.review.review()?;
        if self.review.view.conv.focused {
            let i = self.review.view.conv.selected(review.conv_len())?;
            if let Some(row) = review.threads.get(i) {
                return Some(row.clone());
            }
            self.notify(Level::Info, "Merge request comments are read-only here".into());
            return None;
        }
        let d = self.group().active_diff().filter(|d| d.pair.is_review())?;
        let id = &d.threads.list()[d.thread_at_caret()?].id;
        review.threads.iter().find(|r| &r.thread.id == id).cloned()
    }

    fn review_need_target(&mut self) -> Option<ThreadRow> {
        let row = self.review_target();
        if row.is_none() {
            self.notify(Level::Info, "No comment thread here".into());
        }
        row
    }

    pub(super) fn review_reply(&mut self) {
        if let Some(row) = self.review_need_target() {
            self.reply_to(row);
        }
    }

    pub(super) fn review_resolve(&mut self) {
        if let Some(row) = self.review_need_target() {
            self.resolve_thread(row);
        }
    }

    pub(super) fn review_edit_comment(&mut self) {
        if let Some(row) = self.review_need_target() {
            self.edit_in(row);
        }
    }

    pub(super) fn review_delete_comment(&mut self) {
        if let Some(row) = self.review_need_target() {
            self.delete_in(row);
        }
    }

    fn resolve_thread(&mut self, row: ThreadRow) {
        self.review_write(Job::Resolve {
            thread_id: row.thread.id,
            resolved: !row.thread.resolved,
        });
    }

    /// The user's last comment in the thread, or why there is none (said in the status bar).
    fn own_comment(&mut self, row: &ThreadRow) -> Option<forge::Comment> {
        let Some(user) = self.review.user.clone() else {
            self.notify(Level::Info, "The forge hasn't said who you are yet".into());
            return None;
        };
        let own = row
            .thread
            .comments
            .iter()
            .rev()
            .find(|c| c.author.eq_ignore_ascii_case(&user))
            .cloned();
        if own.is_none() {
            self.notify(Level::Info, "Only your own comments can be changed".into());
        }
        own
    }

    fn delete_in(&mut self, row: ThreadRow) {
        if let Some(c) = self.own_comment(&row) {
            let a = &row.thread.anchor;
            self.prompt = Some(Prompt::ConfirmDeleteComment {
                comment_id: c.id,
                place: format!("{}:{}", a.path, a.end_line),
            });
        }
    }

    /// The user confirmed the delete prompt.
    pub(super) fn review_delete_confirmed(&mut self, comment_id: String) {
        self.review_write(Job::Delete { comment_id });
    }

    /// Opens a comment box under the thread in the active review diff (opening the thread's
    /// file first when CONVERSATIONS has the keys). `None`: it can't be done (said in the
    /// status bar).
    fn compose_in(&mut self, row: &ThreadRow, kind: ComposerKind, text: &str) {
        if self.composer.is_some() {
            return self.notify(Level::Info, "Finish the open comment first".into());
        }
        let Placement::Placed { side, lines } = &row.place else {
            return self.notify(Level::Info, "This thread is outdated: it can't be answered here".into());
        };
        if self.review.view.conv.focused {
            self.review_open_conv();
        }
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return;
        };
        let path = row.thread.anchor.path.clone();
        let Some(d) = self
            .group()
            .active_diff()
            .filter(|d| d.pair.is_review() && d.path == workdir.join(&path))
        else {
            return self.notify(Level::Info, format!("Open {path}'s review diff first"));
        };
        let tab = d.id;
        let mut buf = Buffer::untitled(0);
        let mut view = View::new();
        if !text.is_empty() {
            edit::paste(&mut buf, &mut view, text, false, &[]);
        }
        let start = lines.start as u32 + 1;
        self.start_composer(Composer {
            tab,
            buf,
            view,
            path,
            right: *side == Side::Right,
            start,
            end: (lines.end as u32).max(start),
            lines: Vec::new(),
            commit: String::new(),
            editing: None,
            kind,
        });
    }

    fn reply_to(&mut self, row: ThreadRow) {
        let kind = ComposerKind::Reply {
            thread_id: row.thread.id.clone(),
        };
        self.compose_in(&row, kind, "");
    }

    fn edit_in(&mut self, row: ThreadRow) {
        let Some(c) = self.own_comment(&row) else { return };
        let a = &row.thread.anchor;
        let range = (c.id == row.thread.id && a.start_line < a.end_line).then_some((a.start_line, a.end_line));
        let kind = ComposerKind::Edit {
            comment_id: c.id.clone(),
            range,
        };
        self.compose_in(&row, kind, &c.body);
    }

    /// Ctrl+Enter in a reply / edit box: the forge gets the text at once. The box stays open
    /// until the forge says yes, so a refusal loses nothing.
    pub(super) fn composer_post(&mut self) {
        let Some(c) = &self.composer else { return };
        let body = c.text().trim().to_string();
        if body.is_empty() {
            return self.notify(Level::Info, "The comment is empty: write something, or Esc".into());
        }
        let Some(number) = self.review.review().map(|r| r.mr.number) else {
            return self.notify(Level::Error, "The merge request is not loaded".into());
        };
        let job = match &c.kind {
            ComposerKind::Reply { thread_id } => Job::Reply {
                number,
                thread_id: thread_id.clone(),
                body,
            },
            ComposerKind::Edit { comment_id, range } => Job::Edit {
                comment_id: comment_id.clone(),
                body,
                range: *range,
            },
            ComposerKind::Draft => return,
        };
        self.review_write(job);
    }

    /// A click on a forge thread's block `b` of group `g` at screen `(col, row)`: a button on its
    /// last row.
    pub(super) fn thread_click(&mut self, g: GroupId, b: &BlockHit, col: u16, row: u16) {
        let Some(d) = self.groups.get(g).and_then(|g| g.active_diff()) else {
            return;
        };
        let t = &d.threads.list()[b.thread];
        let rel = b.first + (row - b.rect.y) as usize;
        if d.threads.is_collapsed(b.thread) || rel + 2 != review_blocks::height(t, false, b.rect.width as usize) {
            return;
        }
        let x = col.saturating_sub(b.rect.x) as usize;
        let Some(button) = review_blocks::thread_buttons(t)
            .into_iter()
            .find(|(cols, _, _)| cols.contains(&x))
            .map(|b| b.1)
        else {
            return;
        };
        let Some(thread) = self
            .review
            .review()
            .and_then(|r| r.threads.iter().find(|r| r.thread.id == t.id))
            .cloned()
        else {
            return;
        };
        self.focus_group(g);
        match button {
            ThreadButton::Reply => self.reply_to(thread),
            ThreadButton::Resolve => self.resolve_thread(thread),
            ThreadButton::Edit => self.edit_in(thread),
            ThreadButton::Delete => self.delete_in(thread),
        }
    }
}
