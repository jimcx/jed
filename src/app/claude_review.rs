//! The code review answers of the Claude Code integration (`ipc::Query::Review*`; PLAN.md,
//! Claude Code integration): what `get_review`, `get_review_threads`, `review_refresh`,
//! `review_apply_suggestion` and `review_draft_reply` ask.
//!
//! Reads (`claude_review`, `claude_review_threads`) use only the loaded `ReviewState`: no forge
//! call, no token, nothing off the UI thread. Comment bodies are other people's text (already
//! cleaned of control characters); they are capped per comment and per thread list, and reach
//! Claude only as tool results.
//!
//! Writes go through `claude_review_act` from `ipc_request`:
//! - refresh: queues a lookup and answers at once.
//! - apply: the Apply Suggestion path (`review_suggest`), which is asynchronous (the commit's
//!   lines are read off-thread, the file may open first); the IPC `Responder` waits in
//!   `App::claude_apply` and every outcome of that path answers it (`suggest_notify` for a
//!   refusal, `suggest_applied` + `claude_apply_saved` for the success, once the save finished).
//! - reply: a reply draft (`Draft::reply`, one per thread), saved like any draft, never posted.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::App;
use super::Level;
use super::claude::clip_head;
use crate::buffer::BufferId;
use crate::forge::{Side, Thread};
use crate::ipc::{Query, Reply, Responder};
use crate::review::{Content, Review, ThreadRow};
use crate::review_anchor::Placement;
use crate::review_drafts::Draft;
use crate::suggestion;

/// Threads listed, comments per thread, a comment's bytes, suggestions per thread, a
/// suggestion's lines, files, drafts, and a draft's bytes.
const MAX_THREADS: usize = 100;
const MAX_COMMENTS: usize = 30;
const MAX_BODY: usize = 4000;
const MAX_SUGGESTIONS: usize = 5;
const MAX_SUGGESTION_LINES: usize = 200;
const MAX_FILES: usize = 300;
const MAX_DRAFTS: usize = 100;
/// A reply draft's body, in bytes.
const MAX_REPLY: usize = 20_000;

/// Why `mcp_read_only` hosts refuse the write tools.
pub(super) const READ_ONLY: &str = "code review is read-only for Claude (mcp_read_only in forges.toml)";

/// An apply waiting for its answer (`App::claude_apply`).
#[derive(Debug)]
pub(super) struct Apply {
    responder: Responder,
    path: PathBuf,
    /// The buffer being saved once the suggestion is in.
    saving: Option<BufferId>,
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
    }
}

fn thread_json(row: &ThreadRow, user: Option<&str>, reply: Option<&Draft>) -> Value {
    let t: &Thread = &row.thread;
    let a = &t.anchor;
    let place = match &row.place {
        Placement::Placed { side, lines } => json!({
            "status": "placed",
            "side": side_name(*side),
            "start_line": lines.start + 1,
            "end_line": lines.end.max(lines.start + 1),
        }),
        Placement::Outdated => json!({"status": "outdated"}),
        Placement::Missing => json!({"status": "missing", "note": "the anchor commit is not in the local clone"}),
    };
    let comments: Vec<Value> = t
        .comments
        .iter()
        .take(MAX_COMMENTS)
        .map(|c| {
            let (body, truncated) = clip_head(&c.body, MAX_BODY);
            json!({"author": c.author, "time": c.created_at, "body": body, "truncated": truncated})
        })
        .collect();
    let mut suggestions = Vec::new();
    'all: for (i, c) in t.comments.iter().enumerate() {
        for s in suggestion::parse(&c.body, a) {
            if suggestions.len() == MAX_SUGGESTIONS {
                break 'all;
            }
            let lines: Vec<&str> = s.replacement.lines().collect();
            suggestions.push(json!({
                "comment": i,
                "replaces_start_line": s.start_line,
                "replaces_end_line": s.end_line,
                "new_lines": &lines[..lines.len().min(MAX_SUGGESTION_LINES)],
                "truncated": lines.len() > MAX_SUGGESTION_LINES,
            }));
        }
    }
    json!({
        "id": t.id,
        "path": a.path,
        "side": side_name(a.side),
        "resolved": t.resolved,
        "outdated": t.outdated,
        "placement": place,
        "original": {"start_line": a.start_line, "end_line": a.end_line, "commit": a.commit},
        "comments": comments,
        "comments_total": t.comments.len(),
        "suggestions": suggestions,
        "own_last": user.is_some_and(|u| t.comments.last().is_some_and(|c| c.author.eq_ignore_ascii_case(u))),
        "reply_draft": reply.map(|d| json!({"id": d.id, "body": clip_head(&d.body, MAX_BODY).0, "resolve": d.resolve})),
    })
}

fn epoch(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl App {
    /// `{number, title, unresolved, pending}` of the loaded merge request: what the prompt hook's
    /// review line prints (counts only, never comment text).
    pub(super) fn claude_review_summary(&self) -> Value {
        let Some(r) = self.review.review() else {
            return Value::Null;
        };
        json!({
            "number": r.mr.number,
            "title": crate::forge::http::clean_line(&r.mr.title),
            "unresolved": r.unresolved(),
            "pending": self.review.draft_count(),
            "read_only": r.mcp_read_only,
        })
    }

    /// `get_review`.
    pub(super) fn claude_review(&self) -> Value {
        let r = &self.review;
        let state = json!({"loading": r.loading, "error": r.error});
        let review: &Review = match &r.content {
            Content::Loaded(review) => review,
            Content::Empty(e) => {
                return json!({"review": null, "message": e.text(), "status": state});
            }
            Content::None => {
                return json!({
                    "review": null,
                    "message": "The merge request has not been looked up yet: call review_refresh.",
                    "status": state,
                });
            }
        };
        let mr = &review.mr;
        let local = self.git.status.as_ref().and_then(|s| s.head_id.as_deref());
        let files: Vec<Value> = review
            .files
            .iter()
            .take(MAX_FILES)
            .map(|f| json!({"path": f.path, "old_path": f.old_path, "status": f.status.to_string(), "additions": f.additions, "deletions": f.deletions}))
            .collect();
        let drafts: Vec<Value> = r
            .draft_rows()
            .iter()
            .take(MAX_DRAFTS)
            .map(|row| {
                let d = row.draft;
                json!({
                    "id": d.id,
                    "path": d.path,
                    "line": d.start,
                    "body": clip_head(&d.body, MAX_BODY).0,
                    "reply_to": d.reply_to,
                    "resolve": d.resolve,
                    "by_claude": d.by_claude,
                    "held_back": row.held,
                })
            })
            .collect();
        json!({"review": {
            "number": mr.number,
            "title": crate::forge::http::clean_line(&mr.title),
            "url": mr.url,
            "source_branch": mr.head.name,
            "target_branch": mr.base.name,
            "state": format!("{:?}", mr.state).to_lowercase(),
            "draft": mr.draft,
            "unresolved": review.unresolved(),
            "threads": review.threads.len(),
            "local": format!("{:?}", review.relation).to_lowercase(),
            "local_head_matches": local.is_some_and(|h| h == mr.head.sha),
            "local_note": review.head_banner(local),
            "files": files,
            "files_total": review.files.len(),
            "pending_drafts": drafts,
            "read_only": review.mcp_read_only,
            "fetched_at": r.fetched_at.map(epoch),
            "comments_error": review.comments_error,
        }, "status": state})
    }

    /// `get_review_threads`.
    pub(super) fn claude_review_threads(&self, path: Option<&str>, resolved: bool) -> Reply {
        let Some(review) = self.review.review() else {
            return Reply::error("no merge request loaded: call get_review for why");
        };
        let user = self.review.user.as_deref();
        let key = self.review.drafts_key();
        let store = self.review.drafts.store.as_ref();
        let wanted = |t: &&ThreadRow| {
            (resolved || !t.thread.resolved) && path.is_none_or(|p| t.thread.anchor.path == p.trim_start_matches("./"))
        };
        let all: Vec<&ThreadRow> = review.threads.iter().filter(wanted).collect();
        let threads: Vec<Value> = all
            .iter()
            .take(MAX_THREADS)
            .map(|row| {
                let reply = store
                    .zip(key.as_deref())
                    .and_then(|(s, k)| s.reply_for(k, &row.thread.id));
                thread_json(row, user, reply)
            })
            .collect();
        Reply::data(json!({"threads": threads, "total": all.len(), "truncated": all.len() > MAX_THREADS}))
    }

    /// The queries that change state; `responder` is answered now or, for an apply, when the
    /// suggestion is in and saved (or refused).
    pub(super) fn claude_review_act(&mut self, query: &Query, responder: Responder) {
        // Writes need a loaded merge request on a host that allows them (`mcp_read_only`).
        if !matches!(query, Query::ReviewRefresh) {
            match self.review.review() {
                None => return responder.answer(Reply::error("no merge request loaded: call get_review for why")),
                Some(r) if r.mcp_read_only => return responder.answer(Reply::error(READ_ONLY)),
                Some(_) => {}
            }
        }
        match query {
            Query::ReviewRefresh => {
                self.review_refresh();
                responder.answer(Reply::data(json!({"queued": true, "loading": self.review.loading})));
            }
            Query::ReviewReply { thread, body, resolve } => {
                responder.answer(self.claude_draft_reply(thread, body, *resolve))
            }
            Query::ReviewApply { thread } => self.claude_apply_start(thread, responder),
            _ => responder.answer(Reply::error("not a review action")),
        }
    }

    fn claude_draft_reply(&mut self, thread: &str, body: &str, resolve: bool) -> Reply {
        let body = body.trim();
        if body.is_empty() {
            return Reply::error("the reply is empty");
        }
        if body.len() > MAX_REPLY {
            return Reply::error("the reply is too long");
        }
        let Some(review) = self.review.review() else {
            return Reply::error("no merge request loaded: call get_review for why");
        };
        let Some(t) = review.threads.iter().find(|t| t.thread.id == thread) else {
            return Reply::error(format!("no thread {thread}: use an id from get_review_threads"));
        };
        let a = &t.thread.anchor;
        let mut draft = Draft::reply(
            a.path.clone(),
            a.side == Side::Right,
            (a.start_line, a.end_line),
            thread.to_string(),
            body.to_string(),
            resolve,
        );
        draft.by_claude = true;
        let Some(key) = self.review.drafts_key() else {
            return Reply::error("no merge request loaded");
        };
        if self.review.writing {
            return Reply::error("a submit is in progress in jed: try again in a moment");
        }
        let Some(store) = self.review.drafts.store.as_mut() else {
            return Reply::error("jed has not read the saved drafts yet: try again in a moment");
        };
        let id = draft.id.clone();
        let replaced = store.set_reply(&key, draft);
        let id = store.reply_for(&key, thread).map_or(id, |d| d.id.clone());
        self.save_drafts();
        self.review_blocks_changed();
        let pending = self.review.draft_count();
        self.notify(Level::Info, format!("Claude drafted a reply ({pending} pending)"));
        Reply::data(json!({
            "drafted": true,
            "draft_id": id,
            "replaced": replaced,
            "resolve": resolve,
            "pending": pending,
            "note": "not posted: the user submits the review in jed (Code Review: Submit Review)",
        }))
    }

    fn claude_apply_start(&mut self, thread: &str, responder: Responder) {
        let refuse = |responder: Responder, why: &str| responder.answer(Reply::error(why));
        // An apply whose caller has gone (timed out) is dropped, so a stuck one never blocks the next.
        if self.claude_apply.as_ref().is_some_and(|a| a.responder.is_closed()) {
            self.claude_apply = None;
        }
        if self.claude_apply.is_some() {
            return refuse(responder, "another apply is in progress");
        }
        let Some(row) = self
            .review
            .review()
            .and_then(|r| r.threads.iter().find(|t| t.thread.id == thread))
            .cloned()
        else {
            return refuse(
                responder,
                &format!("no thread {thread}: use an id from get_review_threads"),
            );
        };
        let Some(workdir) = self.git.status.as_ref().map(|s| s.repo.workdir.clone()) else {
            return refuse(responder, "no git repository");
        };
        let path = workdir.join(&row.thread.anchor.path);
        if self
            .buffer_of(&path)
            .and_then(|id| self.buffers.get(id))
            .is_some_and(|b| b.is_dirty())
        {
            return refuse(
                responder,
                "unsaved changes in jed: the user must save or revert the file first",
            );
        }
        self.claude_apply = Some(Apply {
            responder,
            path,
            saving: None,
        });
        if !self.apply_suggestion_of(row)
            && let Some(a) = self.claude_apply.take()
        {
            a.responder.answer(Reply::error("the suggestion could not be applied"));
        }
    }

    /// A refusal of the suggestion path: told to the user, and to the waiting tool call.
    pub(super) fn suggest_notify(&mut self, level: Level, text: String) {
        if let Some(a) = self.claude_apply.take() {
            a.responder.answer(Reply::error(text.clone()));
        }
        self.notify(level, text);
    }

    /// `suggest_notify` for a refusal before anything started: `false` (nothing applied).
    pub(super) fn suggest_refuse(&mut self, level: Level, text: String) -> bool {
        self.suggest_notify(level, text);
        false
    }

    /// The suggestion is in buffer `id`: a waiting tool call has the file saved.
    pub(super) fn suggest_applied(&mut self, id: BufferId) {
        let Some(path) = self.claude_apply.as_ref().map(|a| a.path.clone()) else {
            return;
        };
        self.save(id);
        if self.buffers.get(id).is_some_and(|b| b.save_in_flight()) {
            if let Some(a) = &mut self.claude_apply {
                a.saving = Some(id);
            }
        } else if let Some(a) = self.claude_apply.take() {
            a.responder.answer(Reply::error(format!(
                "applied to {} in jed but not saved: the file on disk is unchanged",
                path.display()
            )));
        }
    }

    /// A save finished: `error` is why it failed. Answers a waiting apply.
    pub(super) fn claude_apply_saved(&mut self, id: BufferId, error: Option<&str>) {
        if self.claude_apply.as_ref().is_none_or(|a| a.saving != Some(id)) {
            return;
        }
        let Some(a) = self.claude_apply.take() else { return };
        let reply = match error {
            None => Reply::data(json!({"applied": true, "saved": true, "path": a.path.display().to_string()})),
            Some(e) => Reply::error(format!("applied in jed but not saved: {e}")),
        };
        a.responder.answer(reply);
    }
}
