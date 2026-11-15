//! What the Code Review panel sends to the forge (no app state, no drawing; `app/review_write.rs`
//! drives it): [`run`] connects (as `review::lookup` does) and performs one [`Job`], and the answer
//! comes back as a [`Done`].
//!
//! Nothing here touches the saved drafts: the app removes a draft only for an id the forge
//! confirmed in `Done::Submitted::posted`, so a failure of any kind keeps it.

use std::sync::Arc;

use crate::forge::config::ForgeConfig;
use crate::forge::{self, DraftReview, RepoRef};
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
    },
    Resolve {
        thread_id: String,
        resolved: bool,
    },
    Edit {
        comment_id: String,
        body: String,
        range: Option<(u32, u32)>,
    },
    Delete {
        comment_id: String,
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
}

impl Job {
    /// Said in the status bar after it worked.
    pub fn done_text(&self) -> &'static str {
        match self {
            Job::User => "",
            Job::Submit { .. } => "Review submitted",
            Job::Reply { .. } => "Reply posted",
            Job::Resolve { resolved: true, .. } => "Thread resolved",
            Job::Resolve { .. } => "Thread reopened",
            Job::Edit { .. } => "Comment edited",
            Job::Delete { .. } => "Comment deleted",
        }
    }

    fn what(&self) -> &'static str {
        match self {
            Job::User => "reading the user",
            Job::Submit { .. } => "submitting the review",
            Job::Reply { .. } => "replying",
            Job::Resolve { .. } => "resolving",
            Job::Edit { .. } => "editing the comment",
            Job::Delete { .. } => "deleting the comment",
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
    Failed {
        job: Job,
        error: String,
    },
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
            // Replies only (no verdict, no line comments): there is no review to post.
            let (mut posted, mut failed, mut error) = (Vec::new(), Vec::new(), None);
            if !review.comments.is_empty() || review.event != forge::ReviewEvent::Comment {
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
                match api.reply(repo, *number, &r.thread_id, &r.body) {
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
        } => api.reply(repo, *number, thread_id, body),
        Job::Resolve { thread_id, resolved } => api.set_resolved(repo, thread_id, *resolved),
        Job::Edit {
            comment_id,
            body,
            range,
        } => api.edit_comment(repo, comment_id, body, *range),
        Job::Delete { comment_id } => api.delete_comment(repo, comment_id),
    };
    match result {
        Ok(()) => {
            log.info(job.done_text());
            Done::Wrote(job)
        }
        Err(e) => fail(job, e.to_string()),
    }
}
