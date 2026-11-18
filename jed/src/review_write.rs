//! What the Code Review panel sends to the forge (no app state, no drawing; `app/review_write.rs`
//! drives it): [`run`] connects (as `review::lookup` does) and performs one [`Job`], and the answer
//! comes back as a [`Done`].
//!
//! Nothing here touches the saved drafts: the app removes a draft only for an id the forge
//! confirmed in `Done::Submitted::posted`, so a failure of any kind keeps it.

use std::sync::Arc;

use crate::forge::config::ForgeConfig;
use crate::forge::{self, DraftRef, DraftReview, NewDraft, RepoRef};
use crate::logs::ToolLog;
use crate::review::Env;

/// One write.
#[derive(Debug, Clone)]
pub enum Job {
    /// The token's user.
    User,
    /// `ids[i]` is the draft `review.comments[i]` was made from.
    Submit {
        number: u64,
        head_sha: String,
        review: DraftReview,
        ids: Vec<String>,
        /// Reply drafts, posted after the review (`posted` / `failed` name them too).
        replies: Vec<ReplyDraft>,
    },
    /// Submits through the forge's pending review: `push` (local only drafts, by id) are added to
    /// it first (`review_id`: the pending review, if there is one); then the review is published
    /// with the verdict. A draft the forge can't hold goes the old way (`submit_review` for a
    /// line comment, a reply after the review). `on_forge` are the drafts already in it.
    SubmitPending {
        number: u64,
        head_sha: String,
        review_id: Option<String>,
        push: Vec<(String, NewDraft)>,
        on_forge: Vec<String>,
        event: forge::ReviewEvent,
        body: String,
    },
    /// Deletes the pending review on the forge.
    DiscardPending {
        number: u64,
        review_id: String,
        /// Delete these local only drafts too (their ids).
        local: Vec<String>,
    },
    Reply {
        number: u64,
        thread_id: String,
        body: String,
        /// The thread's first and last line.
        lines: (u32, u32),
    },
    Resolve {
        thread_id: String,
        resolved: bool,
    },
    Edit {
        comment_id: String,
        body: String,
        range: Option<(u32, u32)>,
        /// The thread's first and last line.
        lines: (u32, u32),
    },
    Delete {
        comment_id: String,
    },
    /// A plain comment on the conversation.
    Comment {
        number: u64,
        body: String,
    },
    /// Edit / delete of a conversation comment (GitHub's endpoint differs from a review comment's).
    EditConversation {
        comment_id: String,
        body: String,
    },
    DeleteConversation {
        comment_id: String,
    },
    /// Withdraws the user's verdict.
    Withdraw {
        number: u64,
        approval: forge::Approval,
    },
    /// Saves draft `id` in the forge's pending review (`pending`: its id, once known).
    AddDraft {
        number: u64,
        head_sha: String,
        pending: Option<String>,
        id: String,
        draft: NewDraft,
    },
    /// A draft's new text in the pending review.
    EditDraft {
        number: u64,
        id: String,
        r: DraftRef,
        body: String,
        range: Option<(u32, u32)>,
        /// The draft's first and last line.
        lines: (u32, u32),
    },
    DeleteDraft {
        number: u64,
        id: String,
        r: DraftRef,
    },
}

/// A reply draft as Submit Review posts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyDraft {
    /// The draft's id.
    pub id: String,
    pub thread_id: String,
    pub body: String,
    pub resolve: bool,
    /// The thread's first and last line.
    pub lines: (u32, u32),
}

impl Job {
    /// A write of a draft to the forge's pending review.
    pub fn is_draft(&self) -> bool {
        matches!(
            self,
            Job::AddDraft { .. } | Job::EditDraft { .. } | Job::DeleteDraft { .. }
        )
    }

    /// Said in the status bar after it worked.
    pub fn done_text(&self) -> &'static str {
        match self {
            Job::User => "",
            Job::Submit { .. } | Job::SubmitPending { .. } => "Review submitted",
            Job::DiscardPending { .. } => "Pending review discarded",
            Job::Reply { .. } => "Reply posted",
            Job::Resolve { resolved: true, .. } => "Thread resolved",
            Job::Resolve { .. } => "Thread reopened",
            Job::Edit { .. } => "Comment edited",
            Job::Delete { .. } | Job::DeleteConversation { .. } => "Comment deleted",
            Job::Comment { .. } => "Comment posted",
            Job::EditConversation { .. } => "Comment edited",
            Job::Withdraw { .. } => "Review withdrawn",
            Job::AddDraft { .. } => "Draft added to the review",
            Job::EditDraft { .. } => "Draft updated on the forge",
            Job::DeleteDraft { .. } => "Draft deleted on the forge",
        }
    }

    fn what(&self) -> &'static str {
        match self {
            Job::User => "reading the user",
            Job::Submit { .. } | Job::SubmitPending { .. } => "submitting the review",
            Job::DiscardPending { .. } => "discarding the pending review",
            Job::Reply { .. } => "replying",
            Job::Resolve { .. } => "resolving",
            Job::Edit { .. } => "editing the comment",
            Job::Delete { .. } | Job::DeleteConversation { .. } => "deleting the comment",
            Job::Comment { .. } => "commenting",
            Job::EditConversation { .. } => "editing the comment",
            Job::Withdraw { .. } => "withdrawing the review",
            Job::AddDraft { .. } => "adding a draft to the review",
            Job::EditDraft { .. } => "editing a draft",
            Job::DeleteDraft { .. } => "deleting a draft",
        }
    }
}

/// What a job needs besides itself.
pub struct Request {
    pub repo: RepoRef,
    pub config: Option<Arc<ForgeConfig>>,
    pub env: Env,
    pub log: ToolLog,
}

#[derive(Debug, Clone)]
pub enum Done {
    User(String),
    /// `posted` / `failed` are draft ids; `error` is the review's own failure, if any.
    Submitted {
        posted: Vec<String>,
        failed: Vec<(String, String)>,
        error: Option<String>,
    },
    /// A `Job::SubmitPending` finished: `pushed` are the drafts it added to the pending review
    /// (they have forge ids now, whatever else happened); the rest as in `Submitted`.
    SubmittedPending {
        pushed: Vec<(String, DraftRef)>,
        posted: Vec<String>,
        failed: Vec<(String, String)>,
        error: Option<String>,
    },
    /// The job worked.
    Wrote(Job),
    /// A draft job worked (`id`: the draft; `r`: where an add put it).
    Draft {
        id: String,
        r: Option<DraftRef>,
    },
    /// A draft job failed; `gone`: the forge answered 404 (the draft is not there any more).
    DraftFailed {
        job: Job,
        error: String,
        gone: bool,
    },
    Failed {
        job: Job,
        error: String,
    },
}

fn draft_failed(log: &ToolLog, job: Job, e: forge::ForgeError) -> Done {
    log.error(&format!("{}: {e}", job.what()));
    let gone = matches!(e, forge::ForgeError::Status { status: 404, .. });
    Done::DraftFailed {
        job,
        error: e.to_string(),
        gone,
    }
}

/// What a submit confirmed: draft ids posted, refused with the reason, and the review's own failure.
#[derive(Default)]
struct Sent {
    posted: Vec<String>,
    failed: Vec<(String, String)>,
    error: Option<String>,
}

/// Posts `review` the old way (one `submit_review`); `ids[i]` is the draft of comment `i`.
#[allow(clippy::too_many_arguments)]
fn post_review(
    api: &dyn forge::Forge,
    repo: &RepoRef,
    log: &ToolLog,
    number: u64,
    head_sha: &str,
    review: &DraftReview,
    ids: &[String],
    sent: &mut Sent,
) {
    let id = |i: usize| ids.get(i).cloned().unwrap_or_default();
    let out = api.submit_review(repo, number, head_sha, review);
    for (i, e) in &out.failed {
        log.error(&format!("draft {}: {e}", id(*i)));
    }
    sent.posted.extend(out.posted.iter().map(|&i| id(i)));
    sent.failed.extend(out.failed.iter().map(|(i, e)| (id(*i), e.clone())));
    sent.error = out.error;
}

/// Each reply has its own outcome. One whose thread resolve fails was posted: it is confirmed (a
/// retry would post it twice) and the failure is the submit's `error`.
fn post_replies(
    api: &dyn forge::Forge,
    repo: &RepoRef,
    log: &ToolLog,
    number: u64,
    replies: &[ReplyDraft],
    sent: &mut Sent,
) {
    for r in replies {
        match api.reply(repo, number, &r.thread_id, &r.body, r.lines) {
            Err(e) => {
                log.error(&format!("reply {}: {e}", r.id));
                sent.failed.push((r.id.clone(), e.to_string()));
            }
            Ok(()) => {
                sent.posted.push(r.id.clone());
                if r.resolve
                    && let Err(e) = api.set_resolved(repo, &r.thread_id, true)
                {
                    log.error(&format!("resolving {}: {e}", r.thread_id));
                    sent.error
                        .get_or_insert(format!("replied, but resolving the thread failed: {e}"));
                }
            }
        }
    }
}

/// Blocking (network): a `Job::SubmitPending`.
#[allow(clippy::too_many_arguments)]
fn submit_pending(
    api: &dyn forge::Forge,
    repo: &RepoRef,
    log: &ToolLog,
    (number, head_sha): (u64, &str),
    mut review_id: Option<String>,
    push: &[(String, NewDraft)],
    on_forge: &[String],
    (event, body): (forge::ReviewEvent, &str),
) -> Done {
    let (mut pushed, mut old, mut replies) = (Vec::new(), Vec::new(), Vec::new());
    for (id, draft) in push {
        match api.add_draft(repo, number, head_sha, review_id.as_deref(), draft) {
            Ok(r) => {
                review_id = Some(r.review_id.clone());
                pushed.push((id.clone(), r));
            }
            Err(e) => {
                log.info(&format!(
                    "draft {id} can't join the pending review ({e}): posting it the old way"
                ));
                match draft {
                    NewDraft::Line(c) => old.push((id.clone(), c.clone())),
                    NewDraft::Reply {
                        thread_id,
                        body,
                        lines,
                        resolve,
                    } => replies.push(ReplyDraft {
                        id: id.clone(),
                        thread_id: thread_id.clone(),
                        body: body.clone(),
                        resolve: *resolve,
                        lines: *lines,
                    }),
                }
            }
        }
    }
    let mut sent = Sent::default();
    let mut published = false;
    if let Some(rid) = &review_id {
        log.info(&format!("Publishing the pending review of !{number} ({event:?})"));
        if let Err(e) = api.submit_pending(repo, number, rid, event, body) {
            log.error(&format!("submitting the pending review: {e}"));
            return Done::SubmittedPending {
                pushed,
                posted: Vec::new(),
                failed: Vec::new(),
                error: Some(e.to_string()),
            };
        }
        published = true;
        sent.posted.extend(on_forge.iter().cloned());
        sent.posted.extend(pushed.iter().map(|(id, _)| id.clone()));
    }
    // Without a pending review the verdict goes with the old path's review; with one, only what
    // the forge couldn't hold is left (a plain comment review).
    let verdict = if published {
        (forge::ReviewEvent::Comment, "")
    } else {
        (event, body)
    };
    if !old.is_empty() || (!published && (verdict.0 != forge::ReviewEvent::Comment || !verdict.1.is_empty())) {
        let review = DraftReview {
            body: verdict.1.to_string(),
            event: verdict.0,
            comments: old.iter().map(|(_, c)| c.clone()).collect(),
        };
        let ids: Vec<String> = old.iter().map(|(id, _)| id.clone()).collect();
        post_review(api, repo, log, number, head_sha, &review, &ids, &mut sent);
    }
    post_replies(api, repo, log, number, &replies, &mut sent);
    Done::SubmittedPending {
        pushed,
        posted: sent.posted,
        failed: sent.failed,
        error: sent.error,
    }
}

/// Blocking (network): call from spawn_blocking.
pub fn run(req: Request, job: Job) -> Done {
    let log = &req.log;
    let fail = |job: Job, error: String| {
        log.error(&format!("{}: {error}", job.what()));
        Done::Failed { job, error }
    };
    let config = crate::review::forge_config(&req.config);
    let env = req.env.clone();
    let api = match forge::connect(&config, &req.repo.host, move |n| env(n)) {
        Ok(f) => f,
        Err(e) => return fail(job, e.to_string()),
    };
    let repo = &req.repo;
    let result = match &job {
        Job::AddDraft {
            number,
            head_sha,
            pending,
            id,
            draft,
        } => {
            return match api.add_draft(repo, *number, head_sha, pending.as_deref(), draft) {
                Ok(r) => {
                    log.info(&format!("draft {id} added to the pending review"));
                    Done::Draft {
                        id: id.clone(),
                        r: Some(r),
                    }
                }
                Err(e) => draft_failed(log, job.clone(), e),
            };
        }
        Job::EditDraft {
            number,
            id,
            r,
            body,
            range,
            lines,
        } => {
            return match api.edit_draft(repo, *number, r, body, *range, *lines) {
                Ok(()) => Done::Draft {
                    id: id.clone(),
                    r: None,
                },
                Err(e) => draft_failed(log, job.clone(), e),
            };
        }
        Job::DeleteDraft { number, id, r } => {
            return match api.delete_draft(repo, *number, r) {
                Ok(()) => Done::Draft {
                    id: id.clone(),
                    r: None,
                },
                Err(e) => draft_failed(log, job.clone(), e),
            };
        }
        Job::SubmitPending {
            number,
            head_sha,
            review_id,
            push,
            on_forge,
            event,
            body,
        } => {
            return submit_pending(
                &*api,
                repo,
                log,
                (*number, head_sha),
                review_id.clone(),
                push,
                on_forge,
                (*event, body),
            );
        }
        Job::DiscardPending { number, review_id, .. } => api.discard_pending(repo, *number, review_id),
        Job::User => {
            return match api.current_user() {
                Ok(u) => Done::User(u),
                Err(e) => fail(job, e.to_string()),
            };
        }
        Job::Submit {
            number,
            head_sha,
            review,
            ids,
            replies,
        } => {
            log.info(&format!(
                "Submitting {} comment(s) and {} reply(ies) to !{number} ({:?})",
                review.comments.len(),
                replies.len(),
                review.event
            ));
            let mut sent = Sent::default();
            // Replies only (no verdict, no line comments, no summary): there is no review to post.
            if !review.comments.is_empty() || review.event != forge::ReviewEvent::Comment || !review.body.is_empty() {
                post_review(&*api, repo, log, *number, head_sha, review, ids, &mut sent);
            }
            post_replies(&*api, repo, log, *number, replies, &mut sent);
            return Done::Submitted {
                posted: sent.posted,
                failed: sent.failed,
                error: sent.error,
            };
        }
        Job::Reply {
            number,
            thread_id,
            body,
            lines,
        } => api.reply(repo, *number, thread_id, body, *lines),
        Job::Resolve { thread_id, resolved } => api.set_resolved(repo, thread_id, *resolved),
        Job::Edit {
            comment_id,
            body,
            range,
            lines,
        } => api.edit_comment(repo, comment_id, body, *range, *lines),
        Job::Delete { comment_id } => api.delete_comment(repo, comment_id),
        Job::Comment { number, body } => api.comment(repo, *number, body),
        Job::EditConversation { comment_id, body } => api.edit_conversation_comment(repo, comment_id, body),
        Job::DeleteConversation { comment_id } => api.delete_conversation_comment(repo, comment_id),
        Job::Withdraw { number, approval } => api.withdraw(repo, *number, approval),
    };
    match result {
        Ok(()) => {
            log.info(job.done_text());
            Done::Wrote(job)
        }
        Err(e) => fail(job, e.to_string()),
    }
}
