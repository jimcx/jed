//! Gitea (and Forgejo) reads: `/api/v1/repos/{owner}/{repo}/pulls…`. What the API does and does
//! not offer is recorded in `tasks/mr_review.md` ("Forge differences"); the parts that matter here:
//!
//! - A review comment has one line (`position`, or `original_position` once outdated) and no side:
//!   the side is read from the last line of its `diff_hunk` (`-` is the old file).
//! - There is no reply-to field: comments on the same path, line and side form one thread, and
//!   its first comment's id is the thread id (the id the resolve call takes).
//! - The merge base is `merge_base` of the pull request.
//! - Pending reviews (review_forge_drafts-a): `POST …/reviews` with `event: PENDING` adds to the
//!   user's existing pending review; a single comment at once (review_pending_replies-b) is a
//!   `COMMENT` review with that comment, refused while a pending review exists (the new review
//!   would publish its comments); there is no way to put a reply into one (the adapter
//!   refuses). Needs Gitea 1.14 or newer (unverified).

use std::collections::HashMap;

use serde::Deserialize;

use super::http::{Client, clean_line, clean_text, encode_path, encode_segment};
use super::{
    Anchor, Approval, ApprovalState, BranchRef, Comment, DraftComment, DraftRef, DraftReview, Forge, ForgeError,
    MergeRequest, MrComments, MrState, NewDraft, PendingComment, PendingReview, RepoRef, Result, ReviewEvent, Side,
    SubmitOutcome, Thread, Verdict, WITHDRAW_MESSAGE, latest_approvals_with_ids, review_body,
};

pub struct Gitea {
    client: Client,
}

impl Gitea {
    pub fn new(client: Client) -> Gitea {
        Gitea { client }
    }

    /// `/repos/{owner}/{repo}`, each segment encoded.
    fn repo_path(repo: &RepoRef) -> String {
        let path: Vec<String> = repo.path.split('/').map(encode_segment).collect();
        format!("/repos/{}", path.join("/"))
    }

    fn pulls(repo: &RepoRef) -> String {
        Self::repo_path(repo) + "/pulls"
    }

    fn issues(repo: &RepoRef) -> String {
        Self::repo_path(repo) + "/issues"
    }
}

#[derive(Deserialize, Default)]
struct User {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize, Default)]
struct RepoInfo {
    #[serde(default)]
    full_name: String,
}

#[derive(Deserialize, Default)]
struct Branch {
    #[serde(default, rename = "ref")]
    name: String,
    #[serde(default)]
    sha: String,
    repo: Option<RepoInfo>,
}

#[derive(Deserialize, Default)]
struct Pull {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    merged: bool,
    user: Option<User>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    base: Branch,
    #[serde(default)]
    head: Branch,
    merge_base: Option<String>,
}

#[derive(Deserialize, Default)]
struct Review {
    id: u64,
    user: Option<User>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    dismissed: bool,
    #[serde(default)]
    comments_count: u32,
    #[serde(default)]
    submitted_at: String,
    #[serde(default)]
    html_url: String,
}

#[derive(Deserialize, Default)]
struct ReviewComment {
    id: u64,
    user: Option<User>,
    resolver: Option<User>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    position: u32,
    #[serde(default)]
    original_position: u32,
    #[serde(default)]
    commit_id: String,
    #[serde(default)]
    original_commit_id: String,
    #[serde(default)]
    diff_hunk: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    html_url: String,
}

#[derive(Deserialize, Default)]
struct IssueComment {
    id: u64,
    user: Option<User>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    html_url: String,
}

fn login(u: &Option<User>) -> String {
    u.as_ref()
        .map(|u| clean_line(&u.login))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "ghost".into())
}

fn branch(b: Branch) -> (BranchRef, Option<String>) {
    let repo = b.repo.map(|r| clean_line(&r.full_name)).filter(|r| !r.is_empty());
    (
        BranchRef {
            name: clean_line(&b.name),
            sha: clean_line(&b.sha),
        },
        repo,
    )
}

fn merge_request(p: Pull) -> MergeRequest {
    let state = match (p.merged, p.state.as_str()) {
        (true, _) => MrState::Merged,
        (_, "closed") => MrState::Closed,
        _ => MrState::Open,
    };
    let (base, _) = branch(p.base);
    let (head, head_repo) = branch(p.head);
    MergeRequest {
        number: p.number,
        title: clean_line(&p.title),
        body: clean_text(&p.body),
        state,
        draft: p.draft,
        author: login(&p.user),
        url: clean_line(&p.html_url),
        base,
        head,
        head_repo,
        merge_base: p.merge_base.map(|s| clean_line(&s)).filter(|s| !s.is_empty()),
    }
}

/// `-` on the last line of the hunk means the comment is on a deleted line.
fn side_of(diff_hunk: &str) -> Side {
    match diff_hunk.lines().rev().find(|l| !l.is_empty()) {
        Some(l) if l.starts_with('-') => Side::Left,
        _ => Side::Right,
    }
}

fn make_comment(id: String, user: &Option<User>, body: &str, created_at: &str, updated_at: &str, url: &str) -> Comment {
    Comment {
        id,
        author: login(user),
        body: clean_text(body),
        created_at: clean_line(created_at),
        updated_at: clean_line(updated_at),
        url: clean_line(url),
    }
}

const MARKER_START: &str = "<!-- jed:range=";

/// jed writes a multi-line comment on its last line and ends the body with this marker (an HTML
/// comment: Gitea's page doesn't show it).
pub fn range_marker(start: u32, end: u32) -> String {
    format!("{MARKER_START}{start}-{end} -->")
}

/// A body as it is posted: the marker after the text when the comment spans lines.
pub fn with_marker(body: &str, range: Option<(u32, u32)>) -> String {
    match range {
        Some((s, e)) if s < e => format!("{}\n\n{}", body.trim_end(), range_marker(s, e)),
        _ => body.to_string(),
    }
}

/// The body without a trailing range marker, and the range it named.
pub fn split_marker(body: &str) -> (String, Option<(u32, u32)>) {
    let trimmed = body.trim_end();
    if let Some(i) = trimmed.rfind(MARKER_START)
        && let Some(inner) = trimmed[i + MARKER_START.len()..].strip_suffix(" -->")
        && let Some((s, e)) = inner.split_once('-')
        && let (Ok(s), Ok(e)) = (s.parse::<u32>(), e.parse::<u32>())
        && s <= e
    {
        return (trimmed[..i].trim_end().to_string(), Some((s, e)));
    }
    (body.to_string(), None)
}

fn event_name(e: ReviewEvent) -> &'static str {
    match e {
        ReviewEvent::Comment => "COMMENT",
        ReviewEvent::Approve => "APPROVED",
        ReviewEvent::RequestChanges => "REQUEST_CHANGES",
    }
}

fn comment(c: &ReviewComment) -> Comment {
    make_comment(
        c.id.to_string(),
        &c.user,
        &split_marker(&clean_text(&c.body)).0,
        &c.created_at,
        &c.updated_at,
        &c.html_url,
    )
}

/// A review or comment id: digits only (it goes into a URL).
fn numeric_id(id: &str) -> Result<u64> {
    id.parse()
        .map_err(|_| ForgeError::Parse(format!("not a Gitea id: {}", clean_line(id))))
}

/// A pending review's comment: text without the range marker, its line (the marker's range
/// when it ends on that line) and side.
fn pending_comment(c: &ReviewComment) -> PendingComment {
    let (body, range) = split_marker(&clean_text(&c.body));
    let line = if c.position > 0 {
        c.position
    } else {
        c.original_position
    };
    let path = clean_line(&c.path);
    let anchor = (line > 0 && !path.is_empty()).then(|| Anchor {
        path,
        side: side_of(&c.diff_hunk),
        start_line: range.filter(|r| r.1 == line).map_or(line, |r| r.0),
        end_line: line,
        commit: clean_line(&c.commit_id),
    });
    PendingComment {
        id: c.id.to_string(),
        reply_to: None,
        anchor,
        body,
        resolve: false,
    }
}

/// Groups review comments into threads: same path, line, side and outdatedness, in id order.
fn threads(mut comments: Vec<ReviewComment>) -> Vec<Thread> {
    comments.sort_by_key(|c| c.id);
    let mut index: HashMap<(String, u32, Side, bool), usize> = HashMap::new();
    let mut out: Vec<Thread> = Vec::new();
    for c in &comments {
        let outdated = c.position == 0;
        let line = if outdated { c.original_position } else { c.position };
        let side = side_of(&c.diff_hunk);
        let path = clean_line(&c.path);
        let key = (path.clone(), line, side, outdated);
        let commit = if outdated && !c.original_commit_id.is_empty() {
            &c.original_commit_id
        } else {
            &c.commit_id
        };
        let range = split_marker(&clean_text(&c.body)).1.filter(|r| r.1 == line);
        let i = *index.entry(key).or_insert_with(|| {
            out.push(Thread {
                id: c.id.to_string(),
                anchor: Anchor {
                    path,
                    side,
                    start_line: line,
                    end_line: line,
                    commit: clean_line(commit),
                },
                resolved: c.resolver.is_some(),
                outdated,
                comments: Vec::new(),
                hunk: (!c.diff_hunk.is_empty()).then(|| clean_text(&c.diff_hunk)),
            });
            out.len() - 1
        });
        if let Some((start, _)) = range
            && out[i].comments.is_empty()
        {
            out[i].anchor.start_line = start;
        }
        out[i].comments.push(comment(c));
    }
    out
}

impl Forge for Gitea {
    /// Gitea cannot filter by head branch, so this lists the open pull requests and filters by
    /// branch name and head repository (a fork's pull request is left out).
    ///
    /// Blocking: call from spawn_blocking.
    fn find_open(&self, repo: &RepoRef, head_branch: &str) -> Result<Vec<MergeRequest>> {
        let pulls: Vec<Pull> = self.client.get_all(&format!("{}?state=open", Self::pulls(repo)))?;
        Ok(pulls
            .into_iter()
            .map(merge_request)
            .filter(|m| m.head.name == head_branch && super::in_repo(m, repo))
            .collect())
    }

    /// Blocking: call from spawn_blocking.
    fn merge_request(&self, repo: &RepoRef, number: u64) -> Result<MergeRequest> {
        Ok(merge_request(
            self.client.get_json(&format!("{}/{number}", Self::pulls(repo)))?,
        ))
    }

    /// Submitted reviews' line comments become threads and their summaries join the plain comments
    /// in the conversation. Pending reviews (the user's own unsent ones) are skipped: jed keeps
    /// its own drafts.
    ///
    /// Blocking: call from spawn_blocking.
    fn comments(&self, repo: &RepoRef, mr: &MergeRequest) -> Result<MrComments> {
        let number = mr.number;
        let pulls = Self::pulls(repo);
        let reviews: Vec<Review> = self.client.get_all(&format!("{pulls}/{number}/reviews"))?;
        let mut line_comments = Vec::new();
        let mut conversation: Vec<Comment> = Vec::new();
        for r in reviews.iter().filter(|r| r.state != "PENDING") {
            if r.comments_count > 0 {
                let mut cs: Vec<ReviewComment> = self
                    .client
                    .get_json(&format!("{pulls}/{number}/reviews/{}/comments", r.id))?;
                line_comments.append(&mut cs);
            }
            if !r.body.trim().is_empty() {
                conversation.push(make_comment(
                    format!("review-{}", r.id),
                    &r.user,
                    &r.body,
                    &r.submitted_at,
                    &r.submitted_at,
                    &r.html_url,
                ));
            }
        }
        let issue: Vec<IssueComment> = self
            .client
            .get_all(&format!("{}/{number}/comments", Self::issues(repo)))?;
        conversation.extend(issue.iter().map(|c| {
            make_comment(
                c.id.to_string(),
                &c.user,
                &c.body,
                &c.created_at,
                &c.updated_at,
                &c.html_url,
            )
        }));
        conversation.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        // A stale approval (a push came after it) still stands, as Gitea shows it; a dismissed
        // review clears its author's verdict.
        let mut verdicts: Vec<&Review> = reviews.iter().collect();
        verdicts.sort_by(|a, b| a.submitted_at.cmp(&b.submitted_at));
        let approvals = latest_approvals_with_ids(verdicts.into_iter().map(|r| {
            let state = match r.state.as_str() {
                _ if r.dismissed => Verdict::Clear,
                "APPROVED" => Verdict::Set(ApprovalState::Approved),
                "REQUEST_CHANGES" => Verdict::Set(ApprovalState::ChangesRequested),
                _ => Verdict::Keep,
            };
            let author = login(&r.user);
            (author, state, Some(r.id.to_string()))
        }));
        Ok(MrComments {
            threads: threads(line_comments),
            conversation,
            approvals,
            notes: Vec::new(),
        })
    }

    /// Blocking: call from spawn_blocking.
    fn current_user(&self) -> Result<String> {
        let user: User = self.client.get_json("/user")?;
        let login = clean_line(&user.login);
        if login.is_empty() {
            return Err(ForgeError::Parse("no login in the answer".into()));
        }
        Ok(login)
    }

    /// One review holding every comment (Gitea takes them together, so they all succeed or all
    /// fail). A multi-line comment is posted on its last line with the range marker.
    ///
    /// Blocking: call from spawn_blocking.
    fn submit_review(&self, repo: &RepoRef, number: u64, head_sha: &str, review: &DraftReview) -> SubmitOutcome {
        let comments: Vec<serde_json::Value> = review
            .comments
            .iter()
            .map(|c| {
                let a = &c.anchor;
                let (new, old) = match a.side {
                    Side::Right => (a.end_line, 0),
                    Side::Left => (0, a.end_line),
                };
                serde_json::json!({
                    "path": a.path,
                    "body": with_marker(&c.body, Some((a.start_line, a.end_line))),
                    "new_position": new,
                    "old_position": old,
                })
            })
            .collect();
        let body = serde_json::json!({
            // Gitea refuses a change request without a body (line comments don't count).
            "body": review_body(review.event, &review.body, review.comments.is_empty()),
            "event": event_name(review.event),
            "commit_id": head_sha,
            "comments": comments,
        });
        let url = format!("{}/{number}/reviews", Self::pulls(repo));
        SubmitOutcome::atomic(review.comments.len(), self.client.post(&url, &body.to_string()))
    }

    /// The user's `PENDING` review (`GET …/reviews`, state `PENDING`, the user's login) and its
    /// comments (`GET …/reviews/{id}/comments`); a multi-line comment's range comes back from its
    /// marker. Gitea shows a pending review only to its author.
    ///
    /// Blocking: call from spawn_blocking.
    fn pending_review(&self, repo: &RepoRef, number: u64, user: &str) -> Result<Option<PendingReview>> {
        let pulls = Self::pulls(repo);
        let reviews: Vec<Review> = self.client.get_all(&format!("{pulls}/{number}/reviews"))?;
        let Some(review) = reviews
            .iter()
            .find(|r| r.state == "PENDING" && login(&r.user).eq_ignore_ascii_case(user))
        else {
            return Ok(None);
        };
        let mut comments: Vec<ReviewComment> = self
            .client
            .get_json(&format!("{pulls}/{number}/reviews/{}/comments", review.id))?;
        comments.sort_by_key(|c| c.id);
        Ok(Some(PendingReview {
            id: review.id.to_string(),
            comments: comments.iter().map(pending_comment).collect(),
        }))
    }

    /// `POST …/reviews` with `event: PENDING` and the one comment: Gitea adds it to the user's
    /// existing pending review (`CreateCodeComment` reuses it) or starts one, and never refuses a
    /// second. That call also rewrites the review's body and commit, so an existing review's body
    /// (`pending`) is read first and sent back. The new comment's id is the newest in the
    /// review's list. A reply has no place in a pending review on Gitea (the reply endpoint posts
    /// at once), so it is refused.
    ///
    /// Blocking: call from spawn_blocking.
    fn add_draft(
        &self,
        repo: &RepoRef,
        number: u64,
        head_sha: &str,
        pending: Option<&str>,
        draft: &NewDraft,
    ) -> Result<DraftRef> {
        let NewDraft::Line(c) = draft else {
            return Err(ForgeError::Config(
                "Gitea can't add a reply to a pending review; it is posted when the review is submitted".into(),
            ));
        };
        let pulls = Self::pulls(repo);
        let kept_body = match pending {
            Some(id) => {
                let id = numeric_id(id)?;
                self.client
                    .get_json::<Review>(&format!("{pulls}/{number}/reviews/{id}"))?
                    .body
            }
            None => String::new(),
        };
        let a = &c.anchor;
        let (new, old) = match a.side {
            Side::Right => (a.end_line, 0),
            Side::Left => (0, a.end_line),
        };
        let body = serde_json::json!({
            "body": kept_body,
            "event": "PENDING",
            "commit_id": head_sha,
            "comments": [{
                "path": a.path,
                "body": with_marker(&c.body, Some((a.start_line, a.end_line))),
                "new_position": new,
                "old_position": old,
            }],
        });
        let response = self
            .client
            .post(&format!("{pulls}/{number}/reviews"), &body.to_string())?;
        let review: Review = super::http::parse_json(&response.body)?;
        let comments: Vec<ReviewComment> = self
            .client
            .get_json(&format!("{pulls}/{number}/reviews/{}/comments", review.id))?;
        let newest = comments
            .iter()
            .map(|c| c.id)
            .max()
            .ok_or_else(|| ForgeError::Parse("the pending review has no comment".into()))?;
        Ok(DraftRef {
            review_id: review.id.to_string(),
            comment_id: newest.to_string(),
        })
    }

    /// A `COMMENT` review holding the one comment (`POST …/reviews`, as `submit_review` with one
    /// comment). Gitea's API has no single-comment route, and a review created while the user has
    /// a pending one takes that review's comments with it (`CreatePullReview` creates its
    /// comments as pending, then `SubmitReview` publishes every pending comment of the user), so
    /// with a pending review the call is refused before any write.
    ///
    /// Blocking: call from spawn_blocking.
    fn post_comment(&self, repo: &RepoRef, number: u64, head_sha: &str, c: &DraftComment) -> Result<()> {
        let user = self.current_user()?;
        if self.pending_review(repo, number, &user)?.is_some() {
            return Err(ForgeError::Config(
                "Gitea would publish your pending review with a single comment; submit or discard it first".into(),
            ));
        }
        let review = DraftReview {
            event: ReviewEvent::Comment,
            body: String::new(),
            comments: vec![c.clone()],
        };
        let out = self.submit_review(repo, number, head_sha, &review);
        match out.error {
            Some(e) => Err(ForgeError::Config(e)),
            None => Ok(()),
        }
    }

    /// A review comment is an issue comment: `PATCH …/issues/comments/{id}`, range marker kept.
    ///
    /// Blocking: call from spawn_blocking.
    fn edit_draft(
        &self,
        repo: &RepoRef,
        _number: u64,
        draft: &DraftRef,
        body: &str,
        range: Option<(u32, u32)>,
        lines: (u32, u32),
    ) -> Result<()> {
        numeric_id(&draft.comment_id)?;
        self.edit_comment(repo, &draft.comment_id, body, range, lines)
    }

    /// Blocking: call from spawn_blocking.
    fn delete_draft(&self, repo: &RepoRef, _number: u64, draft: &DraftRef) -> Result<()> {
        numeric_id(&draft.comment_id)?;
        self.delete_comment(repo, &draft.comment_id)
    }

    /// `POST …/reviews/{id}` with `event` and `body` (only a `PENDING` review can be submitted).
    /// A change request needs a body.
    ///
    /// Blocking: call from spawn_blocking.
    fn submit_pending(
        &self,
        repo: &RepoRef,
        number: u64,
        review_id: &str,
        event: ReviewEvent,
        body: &str,
    ) -> Result<()> {
        let id = numeric_id(review_id)?;
        let body = serde_json::json!({ "body": review_body(event, body, false), "event": event_name(event) });
        self.client
            .post(
                &format!("{}/{number}/reviews/{id}", Self::pulls(repo)),
                &body.to_string(),
            )
            .map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn discard_pending(&self, repo: &RepoRef, number: u64, review_id: &str) -> Result<()> {
        let id = numeric_id(review_id)?;
        self.client
            .delete(&format!("{}/{number}/reviews/{id}", Self::pulls(repo)))
            .map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn reply(&self, repo: &RepoRef, number: u64, thread_id: &str, body: &str, _lines: (u32, u32)) -> Result<()> {
        let url = format!(
            "{}/{number}/comments/{}/replies",
            Self::pulls(repo),
            encode_segment(thread_id)
        );
        self.client
            .post(&url, &serde_json::json!({ "body": body }).to_string())
            .map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn set_resolved(&self, repo: &RepoRef, thread_id: &str, resolved: bool) -> Result<()> {
        let verb = if resolved { "resolve" } else { "unresolve" };
        let url = format!("{}/comments/{}/{verb}", Self::pulls(repo), encode_segment(thread_id));
        self.client.post(&url, "{}").map(|_| ())
    }

    /// Review comments are issue comments on Gitea.
    ///
    /// Blocking: call from spawn_blocking.
    fn edit_comment(
        &self,
        repo: &RepoRef,
        comment_id: &str,
        body: &str,
        range: Option<(u32, u32)>,
        _lines: (u32, u32),
    ) -> Result<()> {
        let url = format!("{}/comments/{}", Self::issues(repo), encode_segment(comment_id));
        let body = serde_json::json!({ "body": with_marker(body, range) });
        self.client.patch(&url, &body.to_string()).map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn delete_comment(&self, repo: &RepoRef, comment_id: &str) -> Result<()> {
        let url = format!("{}/comments/{}", Self::issues(repo), encode_segment(comment_id));
        self.client.delete(&url).map(|_| ())
    }

    /// Dismisses the review behind the verdict: `POST /repos/{o}/{r}/pulls/{n}/reviews/{id}/dismissals`
    /// with `{"message": …}` (swagger `DismissPullReview`). Approvals and change requests alike;
    /// the server answers 403 unless the caller is a repository admin (its message is passed on).
    ///
    /// Blocking: call from spawn_blocking.
    fn withdraw(&self, repo: &RepoRef, number: u64, approval: &Approval) -> Result<()> {
        let id = approval
            .id
            .as_deref()
            .ok_or_else(|| ForgeError::Config("the review's id is unknown".into()))?;
        let url = format!(
            "{}/{number}/reviews/{}/dismissals",
            Self::pulls(repo),
            encode_segment(id)
        );
        let body = serde_json::json!({ "message": WITHDRAW_MESSAGE }).to_string();
        self.client.post(&url, &body).map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn comment(&self, repo: &RepoRef, number: u64, body: &str) -> Result<()> {
        let url = format!("{}/{number}/comments", Self::issues(repo));
        self.client
            .post(&url, &serde_json::json!({ "body": body }).to_string())
            .map(|_| ())
    }

    /// Plain comments are issue comments too: the review comments' calls, without a range marker.
    ///
    /// Blocking: call from spawn_blocking.
    fn edit_conversation_comment(&self, repo: &RepoRef, comment_id: &str, body: &str) -> Result<()> {
        self.edit_comment(repo, comment_id, body, None, (0, 0))
    }

    /// Blocking: call from spawn_blocking.
    fn delete_conversation_comment(&self, repo: &RepoRef, comment_id: &str) -> Result<()> {
        self.delete_comment(repo, comment_id)
    }

    /// Blocking: call from spawn_blocking.
    fn blob_id(&self, repo: &RepoRef, commit: &str, path: &str) -> Result<Option<String>> {
        #[derive(Deserialize, Default)]
        struct File {
            #[serde(default)]
            sha: String,
        }
        let url = format!(
            "{}/contents/{}?ref={}",
            Self::repo_path(repo),
            encode_path(path),
            encode_segment(commit)
        );
        let file: Option<File> = self.client.get_json_opt(&url)?;
        Ok(file.map(|f| clean_line(&f.sha)).filter(|s| !s.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Kind;
    use super::super::config::Resolved;
    use super::super::fake::{FakeServer, Reply};
    use super::*;

    const PR: &str = r#"{"number":7,"title":"Add \u001b[2Jthing","body":"a\r\nb","state":"open","draft":false,"merged":false,
        "user":{"login":"ann"},"html_url":"https://g/o/r/pulls/7","merge_base":"mb111",
        "base":{"ref":"main","sha":"b0","repo":{"full_name":"o/r"}},
        "head":{"ref":"feat","sha":"h0","repo":{"full_name":"O/r"}}}"#;

    #[test]
    fn conversation_comments_are_posted_edited_and_deleted_as_issue_comments() {
        let (server, gitea, repo) = setup(vec![
            ("POST /api/v1/repos/o/r/issues/7/comments", Reply::json("{}")),
            ("PATCH /api/v1/repos/o/r/issues/comments/40", Reply::json("{}")),
            ("DELETE /api/v1/repos/o/r/issues/comments/40", Reply::status(204)),
        ]);
        gitea.comment(&repo, 7, "hello").unwrap();
        gitea.edit_conversation_comment(&repo, "40", "new").unwrap();
        gitea.delete_conversation_comment(&repo, "40").unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[0].body, r#"{"body":"hello"}"#);
        assert_eq!(reqs[1].body, r#"{"body":"new"}"#);
        assert_eq!(reqs[2].method, "DELETE");
    }

    #[test]
    fn review_links_b_withdraw_posts_a_dismissal_with_a_message() {
        let (server, gitea, repo) = setup(vec![
            (
                "POST /api/v1/repos/o/r/pulls/7/reviews/31/dismissals",
                Reply::json("{}"),
            ),
            (
                "POST /api/v1/repos/o/r/pulls/7/reviews/32/dismissals",
                Reply::status(403),
            ),
        ]);
        let ok = Approval {
            author: "me".into(),
            state: ApprovalState::Approved,
            id: Some("31".into()),
        };
        gitea.withdraw(&repo, 7, &ok).unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(
            reqs[0].body,
            format!(r#"{{"message":"{}"}}"#, crate::forge::WITHDRAW_MESSAGE)
        );
        let refused = Approval {
            id: Some("32".into()),
            ..ok.clone()
        };
        let err = gitea.withdraw(&repo, 7, &refused).unwrap_err().to_string();
        assert!(err.contains("403"), "{err}");
        let no_id = Approval { id: None, ..ok };
        assert!(gitea.withdraw(&repo, 7, &no_id).is_err());
        assert_eq!(server.requests().len(), 2, "no id: nothing sent");
    }

    fn setup(routes: Vec<(&str, Reply)>) -> (FakeServer, Gitea, RepoRef) {
        let server = FakeServer::start(routes.into_iter().map(|(p, r)| (p.to_string(), r)).collect());
        let resolved = Resolved {
            host: "fake".into(),
            kind: Kind::Gitea,
            api_url: format!("{}/api/v1", server.url()),
            token_env: "T".into(),
            token: None,
            allow_http: true,
        };
        let client = Client::new(&resolved, |_| Some("tok".into())).unwrap();
        (
            server,
            Gitea::new(client),
            RepoRef {
                host: "fake".into(),
                path: "o/r".into(),
            },
        )
    }

    #[test]
    fn a_fork_request_on_the_same_branch_name_is_left_out() {
        let fork = PR
            .replace("\"number\":7", "\"number\":8")
            .replace("\"full_name\":\"O/r\"}}}", "\"full_name\":\"fork/r\"}}}");
        assert!(fork.contains("fork/r"));
        let (_s, gitea, repo) = setup(vec![(
            "/api/v1/repos/o/r/pulls?state=open&page=1&limit=50",
            Reply::json(&format!("[{fork},{PR}]")),
        )]);
        let found = gitea.find_open(&repo, "feat").unwrap();
        assert_eq!(found.iter().map(|m| m.number).collect::<Vec<_>>(), [7]);
    }

    #[test]
    fn finds_open_requests_by_head_branch() {
        let other = PR
            .replace("\"number\":7", "\"number\":8")
            .replace("\"ref\":\"feat\"", "\"ref\":\"other\"");
        let list = format!("[{PR},{other}]");
        let (server, gitea, repo) = setup(vec![(
            "/api/v1/repos/o/r/pulls?state=open&page=1&limit=50",
            Reply::json(&list),
        )]);
        let found = gitea.find_open(&repo, "feat").unwrap();
        assert_eq!(found.len(), 1);
        let m = &found[0];
        assert_eq!((m.number, m.state, m.author.as_str()), (7, MrState::Open, "ann"));
        assert_eq!(m.title, "Add [2Jthing", "control characters are removed");
        assert_eq!(m.body, "a\nb");
        assert_eq!((m.base.name.as_str(), m.head.sha.as_str()), ("main", "h0"));
        assert_eq!(m.head_repo.as_deref(), Some("O/r"));
        assert_eq!(m.merge_base.as_deref(), Some("mb111"));
        assert_eq!(server.requests()[0].header("authorization"), Some("token tok"));
        assert!(gitea.find_open(&repo, "nope").unwrap().is_empty());
    }

    #[test]
    fn merge_request_details_and_merged_state() {
        let merged = PR
            .replace("\"state\":\"open\"", "\"state\":\"closed\"")
            .replace("\"merged\":false", "\"merged\":true");
        let (_s, gitea, repo) = setup(vec![("/api/v1/repos/o/r/pulls/7", Reply::json(&merged))]);
        assert_eq!(gitea.merge_request(&repo, 7).unwrap().state, MrState::Merged);
        assert!(matches!(
            gitea.merge_request(&repo, 9),
            Err(super::super::ForgeError::Status { status: 404, .. })
        ));
    }

    fn draft(path: &str, side: Side, start: u32, end: u32, body: &str) -> super::super::DraftComment {
        super::super::DraftComment {
            anchor: Anchor {
                path: path.into(),
                side,
                start_line: start,
                end_line: end,
                commit: "h0".into(),
            },
            body: body.into(),
            other: (None, None),
        }
    }

    const PENDING_REVIEWS: &str = r#"[
        {"id":4,"user":{"login":"ann"},"state":"PENDING","body":"hers"},
        {"id":5,"user":{"login":"Me"},"state":"PENDING","body":"keep this"},
        {"id":6,"user":{"login":"me"},"state":"APPROVED","body":""}]"#;

    #[test]
    fn the_pending_review_is_the_users_own_and_its_comments_read_back_clean() {
        let comments = r#"[
            {"id":12,"path":"b.rs","body":"two\n\n<!-- jed:range=7-9 -->","position":9,"commit_id":"h0","diff_hunk":"@@\n+x"},
            {"id":11,"path":"a.rs","body":"one \u001b[2J","position":0,"original_position":3,"commit_id":"h0","diff_hunk":"@@\n-old"}]"#;
        let (server, gitea, repo) = setup(vec![
            ("GET /api/v1/repos/o/r/pulls/7/reviews", Reply::json(PENDING_REVIEWS)),
            (
                "GET /api/v1/repos/o/r/pulls/7/reviews/5/comments",
                Reply::json(comments),
            ),
        ]);
        let p = gitea.pending_review(&repo, 7, "me").unwrap().unwrap();
        assert_eq!(p.id, "5");
        let ids: Vec<&str> = p.comments.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["11", "12"], "in id order");
        assert_eq!(p.comments[0].body, "one [2J", "control characters are removed");
        let a = p.comments[0].anchor.as_ref().unwrap();
        assert_eq!((a.side, a.start_line, a.end_line), (Side::Left, 3, 3));
        let b = p.comments[1].anchor.as_ref().unwrap();
        assert_eq!((b.start_line, b.end_line, b.side), (7, 9, Side::Right));
        assert_eq!(p.comments[1].body, "two", "the range marker is stripped");
        assert!(p.comments.iter().all(|c| c.reply_to.is_none()));
        assert_eq!(server.requests().len(), 2);
        assert!(gitea.pending_review(&repo, 7, "nobody").unwrap().is_none());
    }

    #[test]
    fn a_single_comment_is_a_comment_review_and_refused_while_a_review_is_pending() {
        let (server, gitea, repo) = setup(vec![
            ("GET /api/v1/user", Reply::json(r#"{"login":"me"}"#)),
            (
                "GET /api/v1/repos/o/r/pulls/7/reviews",
                Reply::json(r#"[{"id":6,"user":{"login":"me"},"state":"APPROVED","body":""}]"#),
            ),
            ("POST /api/v1/repos/o/r/pulls/7/reviews", Reply::json(r#"{"id":9}"#)),
            ("GET /api/v1/repos/o/r/pulls/7/reviews/5/comments", Reply::json("[]")),
        ]);
        let c = draft("a.rs", Side::Left, 4, 6, "fix");
        gitea.post_comment(&repo, 7, "h0", &c).unwrap();
        let post = server.requests().into_iter().find(|r| r.method == "POST").unwrap();
        let sent: serde_json::Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(
            (sent["event"].as_str(), sent["commit_id"].as_str()),
            (Some("COMMENT"), Some("h0"))
        );
        assert_eq!(sent["body"], "");
        assert_eq!(
            sent["comments"],
            serde_json::json!([{"path":"a.rs","body":"fix\n\n<!-- jed:range=4-6 -->","new_position":0,"old_position":6}])
        );
        // A pending review would be published with it: refused, nothing written.
        server.set("GET /api/v1/repos/o/r/pulls/7/reviews", Reply::json(PENDING_REVIEWS));
        let err = gitea.post_comment(&repo, 7, "h0", &c).unwrap_err().to_string();
        assert!(err.contains("pending review"), "{err}");
        assert_eq!(server.requests().iter().filter(|r| r.method == "POST").count(), 1);
    }

    #[test]
    fn a_draft_is_posted_as_a_pending_review_reusing_the_existing_one() {
        let (server, gitea, repo) = setup(vec![
            (
                "GET /api/v1/repos/o/r/pulls/7/reviews/5",
                Reply::json(r#"{"id":5,"body":"keep this"}"#),
            ),
            (
                "POST /api/v1/repos/o/r/pulls/7/reviews",
                Reply::json(r#"{"id":5,"state":"PENDING"}"#),
            ),
            (
                "GET /api/v1/repos/o/r/pulls/7/reviews/5/comments",
                Reply::json(r#"[{"id":11},{"id":14},{"id":13}]"#),
            ),
        ]);
        let d = super::super::NewDraft::Line(draft("a.rs", Side::Left, 4, 6, "fix"));
        let r = gitea.add_draft(&repo, 7, "h0", Some("5"), &d).unwrap();
        assert_eq!(
            (r.review_id.as_str(), r.comment_id.as_str()),
            ("5", "14"),
            "the newest comment"
        );
        let seen = server.requests();
        assert_eq!((seen[0].method.as_str(), seen[1].method.as_str()), ("GET", "POST"));
        let sent: serde_json::Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(sent["event"], "PENDING");
        assert_eq!(sent["body"], "keep this", "the review's body is sent back unchanged");
        assert_eq!(sent["commit_id"], "h0");
        assert_eq!(
            sent["comments"][0],
            serde_json::json!({"path":"a.rs","body":"fix\n\n<!-- jed:range=4-6 -->","new_position":0,"old_position":6})
        );
    }

    #[test]
    fn a_first_draft_starts_the_review_with_an_empty_body_and_a_reply_is_refused() {
        let (server, gitea, repo) = setup(vec![
            ("POST /api/v1/repos/o/r/pulls/7/reviews", Reply::json(r#"{"id":9}"#)),
            (
                "GET /api/v1/repos/o/r/pulls/7/reviews/9/comments",
                Reply::json(r#"[{"id":21}]"#),
            ),
        ]);
        let d = super::super::NewDraft::Line(draft("a.rs", Side::Right, 2, 2, "one"));
        let r = gitea.add_draft(&repo, 7, "h0", None, &d).unwrap();
        assert_eq!((r.review_id.as_str(), r.comment_id.as_str()), ("9", "21"));
        let sent: serde_json::Value = serde_json::from_str(&server.requests()[0].body).unwrap();
        assert_eq!(sent["body"], "");
        assert_eq!(sent["comments"][0]["new_position"], 2);
        assert_eq!(sent["comments"][0]["body"], "one", "no marker on one line");
        let reply = super::super::NewDraft::Reply {
            thread_id: "11".into(),
            body: "ok".into(),
            lines: (2, 2),
            resolve: false,
        };
        let err = gitea
            .add_draft(&repo, 7, "h0", Some("9"), &reply)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Gitea can't add a reply to a pending review"), "{err}");
        assert_eq!(server.requests().len(), 2, "a refused reply sends nothing");
    }

    #[test]
    fn drafts_are_edited_deleted_submitted_and_discarded_by_id() {
        let (server, gitea, repo) = setup(vec![
            ("PATCH /api/v1/repos/o/r/issues/comments/14", Reply::json("{}")),
            ("DELETE /api/v1/repos/o/r/issues/comments/14", Reply::status(204)),
            ("POST /api/v1/repos/o/r/pulls/7/reviews/5", Reply::json("{}")),
            ("DELETE /api/v1/repos/o/r/pulls/7/reviews/5", Reply::status(204)),
        ]);
        let r = super::super::DraftRef {
            review_id: "5".into(),
            comment_id: "14".into(),
        };
        gitea.edit_draft(&repo, 7, &r, "new", Some((4, 6)), (4, 6)).unwrap();
        gitea.delete_draft(&repo, 7, &r).unwrap();
        gitea
            .submit_pending(&repo, 7, "5", ReviewEvent::RequestChanges, "")
            .unwrap();
        gitea
            .submit_pending(&repo, 7, "5", ReviewEvent::Approve, "lgtm")
            .unwrap();
        gitea.discard_pending(&repo, 7, "5").unwrap();
        let seen = server.requests();
        assert_eq!(seen[0].body, r#"{"body":"new\n\n<!-- jed:range=4-6 -->"}"#);
        assert_eq!(seen[1].method, "DELETE");
        assert_eq!(
            seen[2].body,
            r#"{"body":"Changes requested","event":"REQUEST_CHANGES"}"#
        );
        assert_eq!(seen[3].body, r#"{"body":"lgtm","event":"APPROVED"}"#);
        assert_eq!(seen[4].method, "DELETE");
        let bad = super::super::DraftRef {
            comment_id: "../x".into(),
            ..r
        };
        assert!(gitea.delete_draft(&repo, 7, &bad).is_err());
        assert!(gitea.discard_pending(&repo, 7, "5/..").is_err());
        assert_eq!(server.requests().len(), 5, "bad ids send nothing");
    }

    #[test]
    fn range_marker_round_trips() {
        let posted = with_marker("fix this\n", Some((41, 42)));
        assert_eq!(posted, "fix this\n\n<!-- jed:range=41-42 -->");
        assert_eq!(split_marker(&posted), ("fix this".to_string(), Some((41, 42))));
        assert_eq!(with_marker("one", Some((5, 5))), "one");
        assert_eq!(split_marker("plain"), ("plain".to_string(), None));
        assert_eq!(
            split_marker("x <!-- jed:range=9-3 -->").1,
            None,
            "end before start is text"
        );
    }

    #[test]
    fn a_forged_marker_never_panics_and_only_a_well_formed_trailing_one_counts() {
        for bad in [
            "x <!-- jed:range=1-99999999999999999999 -->",
            "x <!-- jed:range=-5-3 -->",
            "x <!-- jed:range=a-b -->",
            "x <!-- jed:range=1-2 --> trailing",
            "x <!-- jed:range=1-2-3 -->",
            "<!-- jed:range=",
            "<!-- jed:range=- -->",
        ] {
            assert_eq!(split_marker(bad), (bad.to_string(), None), "{bad}");
        }
        let (text, range) = split_marker("x <!-- jed:range=0-4294967295 -->");
        assert_eq!((text.as_str(), range), ("x", Some((0, u32::MAX))));
        // An edit re-appends the marker once, never twice.
        let once = with_marker(&split_marker("a\n\n<!-- jed:range=2-3 -->").0, Some((2, 3)));
        assert_eq!(once.matches("jed:range").count(), 1);
    }

    #[test]
    fn submit_sends_one_review_with_anchors_and_the_marker() {
        let (server, gitea, repo) = setup(vec![(
            "POST /api/v1/repos/o/r/pulls/7/reviews",
            Reply::json("{\"id\":5}"),
        )]);
        let review = DraftReview {
            body: "overall".into(),
            event: ReviewEvent::RequestChanges,
            comments: vec![
                draft("a.rs", Side::Right, 3, 5, "multi"),
                draft("b.rs", Side::Left, 8, 8, "gone"),
            ],
        };
        let out = gitea.submit_review(&repo, 7, "h0", &review);
        assert_eq!((out.posted.clone(), out.failed.len()), (vec![0, 1], 0));
        let reqs = server.requests();
        assert_eq!(
            (reqs[0].method.as_str(), reqs[0].header("authorization")),
            ("POST", Some("token tok"))
        );
        let v: serde_json::Value = serde_json::from_str(&reqs[0].body).unwrap();
        assert_eq!(v["event"], "REQUEST_CHANGES");
        assert_eq!(v["body"], "overall");
        assert_eq!(v["commit_id"], "h0");
        assert_eq!(v["comments"][0]["path"], "a.rs");
        assert_eq!(v["comments"][0]["new_position"], 5);
        assert_eq!(v["comments"][0]["old_position"], 0);
        assert_eq!(v["comments"][0]["body"], "multi\n\n<!-- jed:range=3-5 -->");
        assert_eq!(v["comments"][1]["new_position"], 0);
        assert_eq!(v["comments"][1]["old_position"], 8);
        assert_eq!(v["comments"][1]["body"], "gone");
    }

    #[test]
    fn a_failed_submit_fails_every_comment_without_the_token() {
        let (_s, gitea, repo) = setup(vec![(
            "POST /api/v1/repos/o/r/pulls/7/reviews",
            Reply::status(500).with_body("{\"message\":\"boom tok\"}"),
        )]);
        let review = DraftReview {
            body: String::new(),
            event: ReviewEvent::Comment,
            comments: vec![draft("a.rs", Side::Right, 1, 1, "x")],
        };
        let out = gitea.submit_review(&repo, 7, "h0", &review);
        assert!(out.posted.is_empty());
        assert_eq!(out.failed.len(), 1);
        assert!(
            out.failed[0].1.contains("500") && !out.failed[0].1.contains("boom tok"),
            "{out:?}"
        );
    }

    #[test]
    fn replies_resolves_edits_and_deletes() {
        let (server, gitea, repo) = setup(vec![
            ("POST /api/v1/repos/o/r/pulls/7/comments/11/replies", Reply::json("{}")),
            ("POST /api/v1/repos/o/r/pulls/comments/11/resolve", Reply::status(204)),
            ("POST /api/v1/repos/o/r/pulls/comments/11/unresolve", Reply::status(204)),
            ("PATCH /api/v1/repos/o/r/issues/comments/11", Reply::json("{}")),
            ("DELETE /api/v1/repos/o/r/issues/comments/11", Reply::status(204)),
            ("/api/v1/user", Reply::json("{\"login\":\"me\"}")),
        ]);
        gitea.reply(&repo, 7, "11", "hi", (1, 1)).unwrap();
        gitea.set_resolved(&repo, "11", true).unwrap();
        gitea.set_resolved(&repo, "11", false).unwrap();
        gitea.edit_comment(&repo, "11", "new", Some((2, 4)), (2, 4)).unwrap();
        gitea.delete_comment(&repo, "11").unwrap();
        assert_eq!(gitea.current_user().unwrap(), "me");
        let reqs = server.requests();
        let ms: Vec<&str> = reqs.iter().map(|r| r.method.as_str()).collect();
        assert_eq!(ms, ["POST", "POST", "POST", "PATCH", "DELETE", "GET"]);
        assert_eq!(reqs[0].body, "{\"body\":\"hi\"}");
        assert_eq!(reqs[3].body, "{\"body\":\"new\\n\\n<!-- jed:range=2-4 -->\"}");
        assert!(reqs[1].target.ends_with("/resolve") && reqs[2].target.ends_with("/unresolve"));
        // A plain suggestion fence on a multi-line thread is posted as written (a plain fence
        // means the whole range there), and only an edit of the first comment adds the marker.
        let fence = "```suggestion\nx\n```";
        gitea.reply(&repo, 7, "11", fence, (2, 4)).unwrap();
        gitea.edit_comment(&repo, "11", fence, None, (2, 4)).unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[reqs.len() - 2].body, r#"{"body":"```suggestion\nx\n```"}"#);
        assert_eq!(reqs[reqs.len() - 1].body, r#"{"body":"```suggestion\nx\n```"}"#);
        assert!(
            gitea.reply(&repo, 7, "99", "x", (1, 1)).is_err(),
            "an unknown route is a 404"
        );
    }

    #[test]
    fn a_multi_line_thread_reads_back_its_range_and_hides_the_marker() {
        let reviews = r#"[{"id":1,"user":{"login":"me"},"body":"","state":"COMMENT","comments_count":1}]"#;
        let comments = r#"[{"id":21,"user":{"login":"me"},"body":"multi\n\n<!-- jed:range=3-5 -->","path":"a.rs","position":5,"original_position":5,"commit_id":"c1","original_commit_id":"c1","diff_hunk":"@@ -1 +1 @@\n+y"}]"#;
        let (_s, gitea, repo) = setup(vec![
            (
                "/api/v1/repos/o/r/pulls/7/reviews?page=1&limit=50",
                Reply::json(reviews),
            ),
            ("/api/v1/repos/o/r/pulls/7/reviews/1/comments", Reply::json(comments)),
            ("/api/v1/repos/o/r/issues/7/comments?page=1&limit=50", Reply::json("[]")),
        ]);
        let c = gitea.comments(&repo, &crate::forge::test_mr(7)).unwrap();
        let t = &c.threads[0];
        assert_eq!((t.anchor.start_line, t.anchor.end_line), (3, 5));
        assert_eq!(t.comments[0].body, "multi");
    }

    #[test]
    fn a_dismissed_review_clears_the_reviewers_verdict() {
        let reviews = r#"[
            {"id":1,"user":{"login":"bob"},"body":"","state":"REQUEST_CHANGES","submitted_at":"2026-01-01T10:00:00Z"},
            {"id":2,"user":{"login":"bob"},"body":"","state":"APPROVED","dismissed":true,"submitted_at":"2026-01-01T11:00:00Z"},
            {"id":3,"user":{"login":"sam"},"body":"","state":"APPROVED","stale":true,"submitted_at":"2026-01-01T12:00:00Z"}]"#;
        let (_s, gitea, repo) = setup(vec![
            (
                "/api/v1/repos/o/r/pulls/7/reviews?page=1&limit=50",
                Reply::json(reviews),
            ),
            ("/api/v1/repos/o/r/issues/7/comments?page=1&limit=50", Reply::json("[]")),
        ]);
        let a = gitea.comments(&repo, &crate::forge::test_mr(7)).unwrap().approvals;
        let got: Vec<(&str, ApprovalState)> = a.iter().map(|a| (a.author.as_str(), a.state)).collect();
        // bob's dismissed approval leaves nothing; sam's stale approval still stands.
        assert_eq!(got, [("sam", ApprovalState::Approved)]);
        assert!(a[0].id.is_some(), "the review's id is kept, for withdrawing it");
    }

    #[test]
    fn a_change_request_always_carries_a_body() {
        let (server, gitea, repo) = setup(vec![("POST /api/v1/repos/o/r/pulls/7/reviews", Reply::json("{}"))]);
        let empty = DraftReview {
            event: ReviewEvent::RequestChanges,
            body: String::new(),
            comments: Vec::new(),
        };
        assert!(gitea.submit_review(&repo, 7, "h", &empty).error.is_none());
        let sent: serde_json::Value = serde_json::from_str(&server.requests()[0].body).unwrap();
        assert_eq!(sent["body"], "Changes requested");
        // A comment review needs a body only when no line comments come with it; approvals never.
        for (event, expect) in [(ReviewEvent::Comment, "Review comments"), (ReviewEvent::Approve, "")] {
            let r = DraftReview { event, ..empty.clone() };
            assert!(gitea.submit_review(&repo, 7, "h", &r).error.is_none());
            let sent: serde_json::Value = serde_json::from_str(&server.requests().last().unwrap().body).unwrap();
            assert_eq!(sent["body"], expect, "{event:?}");
        }
        let mine = DraftReview {
            body: "mine".into(),
            ..empty
        };
        gitea.submit_review(&repo, 7, "h", &mine);
        let sent: serde_json::Value = serde_json::from_str(&server.requests().last().unwrap().body).unwrap();
        assert_eq!(sent["body"], "mine");
    }

    #[test]
    fn approvals_keep_the_latest_state_per_author() {
        let reviews = r#"[
            {"id":1,"user":{"login":"sam"},"body":"","state":"APPROVED","submitted_at":"2026-01-01T10:00:00Z"},
            {"id":2,"user":{"login":"bob"},"body":"","state":"REQUEST_CHANGES","submitted_at":"2026-01-01T11:00:00Z"},
            {"id":3,"user":{"login":"sam"},"body":"nit","state":"COMMENT","submitted_at":"2026-01-01T12:00:00Z"},
            {"id":4,"user":{"login":"alex"},"body":"","state":"APPROVED","dismissed":true,"submitted_at":"2026-01-01T13:00:00Z"},
            {"id":5,"user":{"login":"bob"},"body":"","state":"APPROVED","submitted_at":"2026-01-01T14:00:00Z"},
            {"id":6,"user":{"login":"zed"},"body":"","state":"REQUEST_CHANGES","submitted_at":"2026-01-01T15:00:00Z"}]"#;
        let (_s, gitea, repo) = setup(vec![
            (
                "/api/v1/repos/o/r/pulls/7/reviews?page=1&limit=50",
                Reply::json(reviews),
            ),
            ("/api/v1/repos/o/r/issues/7/comments?page=1&limit=50", Reply::json("[]")),
        ]);
        let a = gitea.comments(&repo, &crate::forge::test_mr(7)).unwrap().approvals;
        let got: Vec<(&str, ApprovalState)> = a.iter().map(|a| (a.author.as_str(), a.state)).collect();
        assert_eq!(
            got,
            [
                ("sam", ApprovalState::Approved),
                ("bob", ApprovalState::Approved),
                ("zed", ApprovalState::ChangesRequested)
            ]
        );
    }

    #[test]
    fn comments_become_threads_and_a_conversation() {
        let reviews = r#"[
            {"id":1,"user":{"login":"bob"},"body":"Looks \u001b[0mgood","state":"COMMENT","comments_count":3,"submitted_at":"2026-01-02T10:00:00Z","html_url":"u1"},
            {"id":2,"user":{"login":"me"},"body":"","state":"PENDING","comments_count":1,"submitted_at":"","html_url":""}]"#;
        let comments = r#"[
            {"id":11,"user":{"login":"bob"},"body":"why?","path":"a.rs","position":5,"original_position":5,"commit_id":"c1","original_commit_id":"c1","diff_hunk":"@@ -1,2 +1,3 @@\n x\n+y","resolver":null},
            {"id":12,"user":{"login":"ann"},"body":"because","path":"a.rs","position":5,"original_position":5,"commit_id":"c1","original_commit_id":"c1","diff_hunk":"@@ -1,2 +1,3 @@\n x\n+y","resolver":{"login":"bob"}},
            {"id":13,"user":{"login":"bob"},"body":"gone","path":"a.rs","position":0,"original_position":3,"commit_id":"c2","original_commit_id":"c0","diff_hunk":"@@ -1,2 +1,3 @@\n x\n-z","resolver":null}]"#;
        let issue = r#"[{"id":99,"user":{"login":"ann"},"body":"thanks","created_at":"2026-01-01T10:00:00Z","updated_at":"","html_url":"u99"}]"#;
        let (server, gitea, repo) = setup(vec![
            (
                "/api/v1/repos/o/r/pulls/7/reviews?page=1&limit=50",
                Reply::json(reviews),
            ),
            ("/api/v1/repos/o/r/pulls/7/reviews/1/comments", Reply::json(comments)),
            (
                "/api/v1/repos/o/r/issues/7/comments?page=1&limit=50",
                Reply::json(issue),
            ),
        ]);
        let c = gitea.comments(&repo, &crate::forge::test_mr(7)).unwrap();
        assert_eq!(c.threads.len(), 2);
        let t = &c.threads[0];
        assert_eq!(t.id, "11");
        assert_eq!(
            (t.anchor.start_line, t.anchor.end_line, t.anchor.side),
            (5, 5, Side::Right)
        );
        assert_eq!(
            t.comments.iter().map(|c| c.body.as_str()).collect::<Vec<_>>(),
            ["why?", "because"]
        );
        assert!(!t.resolved && !t.outdated, "the first comment decides resolved");
        assert_eq!(t.hunk.as_deref(), Some("@@ -1,2 +1,3 @@\n x\n+y"));
        let o = &c.threads[1];
        assert!(o.outdated);
        assert_eq!(
            (o.anchor.start_line, o.anchor.side, o.anchor.commit.as_str()),
            (3, Side::Left, "c0")
        );
        assert_eq!(
            c.conversation.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["99", "review-1"]
        );
        assert_eq!(c.conversation[1].body, "Looks [0mgood");
        assert!(
            server.requests().iter().all(|r| !r.target.contains("reviews/2")),
            "pending reviews are skipped"
        );
    }

    #[test]
    fn blob_id_reads_the_file_sha_and_maps_404_to_none() {
        let (server, forge, repo) = setup(vec![(
            "/api/v1/repos/o/r/contents/src/a%20b.rs?ref=c0ffee",
            Reply::json(r#"{"name":"a b.rs","sha":"abc123"}"#),
        )]);
        assert_eq!(
            forge.blob_id(&repo, "c0ffee", "src/a b.rs").unwrap().as_deref(),
            Some("abc123")
        );
        assert_eq!(forge.blob_id(&repo, "c0ffee", "src/none.rs").unwrap(), None);
        assert_eq!(server.requests().len(), 2);
    }
}
