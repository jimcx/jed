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
            Job::Submit { .. } => "Review submitted",
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
            Job::Submit { .. } => "submitting the review",
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
            let id = |i: usize| ids.get(i).cloned().unwrap_or_default();
            // Replies only (no verdict, no line comments, no summary): there is no review to post.
            let (mut posted, mut failed, mut error) = (Vec::new(), Vec::new(), None);
            if !review.comments.is_empty() || review.event != forge::ReviewEvent::Comment || !review.body.is_empty() {
                let out = api.submit_review(repo, *number, head_sha, review);
                for (i, e) in &out.failed {
                    log.error(&format!("draft {}: {e}", id(*i)));
                }
                posted = out.posted.iter().map(|&i| id(i)).collect();
                failed = out.failed.iter().map(|(i, e)| (id(*i), e.clone())).collect();
                error = out.error;
            }
            // Each reply has its own outcome. One whose thread resolve fails was posted: it is
            // confirmed (a retry would post it twice) and the failure is the submit's `error`.
            for r in replies {
                match api.reply(repo, *number, &r.thread_id, &r.body, r.lines) {
                    Err(e) => {
                        log.error(&format!("reply {}: {e}", r.id));
                        failed.push((r.id.clone(), e.to_string()));
                    }
                    Ok(()) => {
                        posted.push(r.id.clone());
                        if r.resolve
                            && let Err(e) = api.set_resolved(repo, &r.thread_id, true)
                        {
                            log.error(&format!("resolving {}: {e}", r.thread_id));
                            error.get_or_insert(format!("replied, but resolving the thread failed: {e}"));
                        }
                    }
                }
            }
            return Done::Submitted { posted, failed, error };
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
