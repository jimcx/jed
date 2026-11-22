//! GitHub (github.com and Enterprise Server): REST for pull requests, files, reviews and comment
//! edits, GraphQL for what REST cannot do (review threads with their resolved state, resolving, and
//! replying by thread). What the API offers is recorded in `tasks/mr_review.md` ("Forge
//! differences"); the parts that matter here:
//!
//! - A review thread has native line ranges (`startLine`..`line`, one `diffSide`), `isOutdated`
//!   and `isResolved`; outdated threads keep their `originalLine`s and `originalCommit`. No jed
//!   range marker is needed.
//! - A thread's id is its GraphQL node id (the resolve and reply mutations take it); a comment's
//!   id is its `databaseId` (the REST edit and delete calls take that).
//! - A review is one atomic REST request: `POST …/reviews` with `comments[]` at `line`/`side`
//!   (and `start_line`/`start_side` for a range), never the old diff `position`.
//! - A pending review (review_forge_drafts-a) is a REST review created with no `event`; its
//!   comments are added, edited and deleted through GraphQL by node id, and the review's id is
//!   kept as `databaseId:nodeId` (REST wants one, GraphQL the other). A thread added to it is
//!   anchored on the review's commit.
//! - A single line comment at once (review_pending_replies-b) is `POST …/pulls/{n}/comments`; it
//!   is refused with a 422 while the user has a pending review (one per user and pull request),
//!   so the adapter says so and the app doesn't offer it then.
//! - The merge base comes from the compare endpoint (`merge_base_commit`).

use serde::Deserialize;
use serde_json::{Value, json};

use super::http::{Client, clean_line, clean_text, encode_path, encode_segment};
use super::{
    Anchor, Approval, ApprovalState, BranchRef, Comment, DraftComment, DraftRef, DraftReview, Forge, ForgeError,
    MergeRequest, MrComments, MrState, NewDraft, PendingComment, PendingReview, ReplyTo, RepoRef, Result, ReviewEvent,
    Side, SubmitOutcome, Thread, Verdict, WITHDRAW_MESSAGE, latest_approvals_with_ids, review_body,
};

/// Stop a GraphQL connection after this many pages of 100 (a malicious server can't loop us).
const MAX_GRAPHQL_PAGES: usize = 100;

pub struct GitHub {
    client: Client,
}

impl GitHub {
    pub fn new(client: Client) -> GitHub {
        GitHub { client }
    }

    /// `/repos/{owner}/{repo}`, each segment encoded.
    fn repo_path(repo: &RepoRef) -> String {
        let path: Vec<String> = repo.path.split('/').map(encode_segment).collect();
        format!("/repos/{}", path.join("/"))
    }

    fn pulls(repo: &RepoRef) -> String {
        Self::repo_path(repo) + "/pulls"
    }

    /// Every node of a connection of the pull request (`reviewThreads`, `comments`, `reviews`),
    /// following `pageInfo` cursors.
    fn nodes<T: serde::de::DeserializeOwned>(
        &self,
        repo: &RepoRef,
        number: u64,
        field: &str,
        selection: &str,
    ) -> Result<(Vec<T>, bool)> {
        let (owner, name) = repo
            .path
            .split_once('/')
            .ok_or_else(|| ForgeError::Config(format!("{}: not an owner/repo path", repo.path)))?;
        let query = CONNECTION_QUERY.replace("FIELD", field).replace("SELECTION", selection);
        let mut all = Vec::new();
        let mut after = Value::Null;
        let mut cut = false;
        for n in 0..MAX_GRAPHQL_PAGES {
            let mut data = self.client.graphql(
                &query,
                json!({ "owner": owner, "name": name, "number": number, "after": after }),
            )?;
            let conn = data
                .pointer_mut(&format!("/repository/pullRequest/{field}"))
                .ok_or_else(|| ForgeError::Parse(format!("no {field} in the answer")))?;
            let nodes: Vec<Option<T>> =
                serde_json::from_value(conn.get_mut("nodes").map(Value::take).unwrap_or_default())
                    .map_err(|e| ForgeError::Parse(e.to_string()))?;
            all.extend(nodes.into_iter().flatten());
            let page = conn.get("pageInfo");
            let more = page.and_then(|p| p.get("hasNextPage")).and_then(Value::as_bool) == Some(true);
            match page.and_then(|p| p.get("endCursor")).and_then(Value::as_str) {
                Some(cursor) if more => {
                    after = json!(cursor);
                    cut = n + 1 == MAX_GRAPHQL_PAGES;
                }
                _ => break,
            }
        }
        Ok((all, cut))
    }
}

const CONNECTION_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){\
    pullRequest(number:$number){FIELD(first:100,after:$after){pageInfo{hasNextPage endCursor} nodes{SELECTION}}}}}";

const THREAD_FIELDS: &str = "id isResolved isOutdated path line startLine originalLine originalStartLine diffSide startDiffSide \
    comments(first:100){pageInfo{hasNextPage} nodes{id databaseId body createdAt updatedAt url author{login} commit{oid} originalCommit{oid} diffHunk}}";
const REVIEW_FIELDS: &str = "databaseId body state submittedAt url author{login}";
const ISSUE_COMMENT_FIELDS: &str = "databaseId body createdAt updatedAt url author{login}";

const REPLY_MUTATION: &str = "mutation($thread:ID!,$body:String!){addPullRequestReviewThreadReply(\
    input:{pullRequestReviewThreadId:$thread,body:$body}){comment{id}}}";
const ADD_THREAD_MUTATION: &str = "mutation($review:ID!,$path:String!,$body:String!,$line:Int!,$side:DiffSide!,\
$startLine:Int,$startSide:DiffSide){addPullRequestReviewThread(input:{pullRequestReviewId:$review,path:$path,\
body:$body,line:$line,side:$side,startLine:$startLine,startSide:$startSide,subjectType:LINE}){thread{id \
comments(first:1){nodes{id}}}}}";
const ADD_REPLY_MUTATION: &str = "mutation($review:ID!,$thread:ID!,$body:String!){addPullRequestReviewThreadReply(\
input:{pullRequestReviewId:$review,pullRequestReviewThreadId:$thread,body:$body}){comment{id}}}";
const UPDATE_COMMENT_MUTATION: &str = "mutation($id:ID!,$body:String!){updatePullRequestReviewComment(\
input:{pullRequestReviewCommentId:$id,body:$body}){pullRequestReviewComment{id}}}";
const DELETE_COMMENT_MUTATION: &str = "mutation($id:ID!){deletePullRequestReviewComment(input:{id:$id}){\
pullRequestReviewComment{id}}}";
const RESOLVE_MUTATION: &str = "mutation($thread:ID!){resolveReviewThread(input:{threadId:$thread}){thread{id}}}";
const UNRESOLVE_MUTATION: &str = "mutation($thread:ID!){unresolveReviewThread(input:{threadId:$thread}){thread{id}}}";

#[derive(Deserialize, Default)]
struct User {
    login: Option<String>,
}

#[derive(Deserialize, Default)]
struct RepoInfo {
    full_name: Option<String>,
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
    title: Option<String>,
    body: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    merged: bool,
    merged_at: Option<String>,
    user: Option<User>,
    html_url: Option<String>,
    #[serde(default)]
    base: Branch,
    #[serde(default)]
    head: Branch,
}

#[derive(Deserialize, Default)]
struct Compare {
    merge_base_commit: Option<Oid>,
}

#[derive(Deserialize, Default)]
struct Oid {
    #[serde(alias = "oid")]
    sha: Option<String>,
}

#[derive(Deserialize, Default)]
struct Actor {
    login: Option<String>,
}

#[derive(Deserialize, Default)]
struct CommitRef {
    oid: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Nodes<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<T>>,
    #[serde(default)]
    page_info: PageInfo,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    #[serde(default)]
    has_next_page: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GqlComment {
    id: Option<String>,
    database_id: Option<u64>,
    body: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    url: Option<String>,
    author: Option<Actor>,
    commit: Option<CommitRef>,
    original_commit: Option<CommitRef>,
    diff_hunk: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlThread {
    id: String,
    #[serde(default)]
    is_resolved: bool,
    #[serde(default)]
    is_outdated: bool,
    path: Option<String>,
    line: Option<u32>,
    start_line: Option<u32>,
    original_line: Option<u32>,
    original_start_line: Option<u32>,
    diff_side: Option<String>,
    start_diff_side: Option<String>,
    #[serde(default)]
    comments: Nodes<GqlComment>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GqlReview {
    database_id: Option<u64>,
    body: Option<String>,
    state: Option<String>,
    submitted_at: Option<String>,
    url: Option<String>,
    author: Option<Actor>,
}

fn ghost(login: Option<&String>) -> String {
    login
        .map(|l| clean_line(l))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "ghost".into())
}

fn branch(b: Branch) -> (BranchRef, Option<String>) {
    let repo = b
        .repo
        .and_then(|r| r.full_name)
        .map(|r| clean_line(&r))
        .filter(|r| !r.is_empty());
    (
        BranchRef {
            name: clean_line(&b.name),
            sha: clean_line(&b.sha),
        },
        repo,
    )
}

fn merge_request(p: Pull) -> MergeRequest {
    let state = if p.merged || p.merged_at.is_some() {
        MrState::Merged
    } else if p.state == "closed" {
        MrState::Closed
    } else {
        MrState::Open
    };
    let (base, _) = branch(p.base);
    let (head, head_repo) = branch(p.head);
    MergeRequest {
        number: p.number,
        title: clean_line(p.title.as_deref().unwrap_or_default()),
        body: clean_text(p.body.as_deref().unwrap_or_default()),
        state,
        draft: p.draft,
        author: ghost(p.user.as_ref().and_then(|u| u.login.as_ref())),
        url: clean_line(p.html_url.as_deref().unwrap_or_default()),
        base,
        head,
        head_repo,
        merge_base: None,
    }
}

fn event_name(e: ReviewEvent) -> &'static str {
    match e {
        ReviewEvent::Comment => "COMMENT",
        ReviewEvent::Approve => "APPROVE",
        ReviewEvent::RequestChanges => "REQUEST_CHANGES",
    }
}

fn side_name(s: Side) -> &'static str {
    match s {
        Side::Left => "LEFT",
        Side::Right => "RIGHT",
    }
}

fn text(s: &Option<String>) -> String {
    clean_line(s.as_deref().unwrap_or_default())
}

fn comment(c: &GqlComment) -> Comment {
    Comment {
        id: match (c.database_id, &c.id) {
            (Some(n), _) => n.to_string(),
            (None, Some(id)) => clean_line(id),
            (None, None) => String::new(),
        },
        author: ghost(c.author.as_ref().and_then(|a| a.login.as_ref())),
        body: clean_text(c.body.as_deref().unwrap_or_default()),
        created_at: text(&c.created_at),
        updated_at: text(&c.updated_at),
        url: text(&c.url),
    }
}

/// A thread's anchor: where it is now, or where it was (`original*`, on the original commit)
/// once outdated. A thread without any line (a comment on the file) is marked outdated.
fn thread(t: GqlThread) -> Thread {
    let comments: Vec<&GqlComment> = t.comments.nodes.iter().flatten().collect();
    let first = comments.first();
    let (end, start, commit) = if t.is_outdated {
        (
            t.original_line.or(t.line),
            t.original_start_line.or(t.start_line),
            first.and_then(|c| c.original_commit.as_ref().or(c.commit.as_ref())),
        )
    } else {
        (
            t.line.or(t.original_line),
            t.start_line,
            first.and_then(|c| c.commit.as_ref()),
        )
    };
    // No usable line at all (a file-level comment): never guess a line, leave it to CONVERSATIONS.
    let outdated = t.is_outdated || end.is_none();
    let end = end.unwrap_or(1).max(1);
    let mut start = start.unwrap_or(end).clamp(1, end);
    // A range whose ends are on different sides has no single span of lines to show or fence:
    // it is anchored on its end line (on `diffSide`) only.
    if t.start_diff_side
        .as_deref()
        .is_some_and(|s| Some(s) != t.diff_side.as_deref())
    {
        start = end;
    }
    Thread {
        id: clean_line(&t.id),
        anchor: Anchor {
            path: text(&t.path),
            // DiffSide has only LEFT and RIGHT; anything else is read as the new side.
            side: if t.diff_side.as_deref() == Some("LEFT") {
                Side::Left
            } else {
                Side::Right
            },
            start_line: start,
            end_line: end,
            commit: commit.map(|c| text(&c.oid)).unwrap_or_default(),
        },
        resolved: t.is_resolved,
        outdated,
        hunk: first
            .and_then(|c| c.diff_hunk.as_deref())
            .filter(|h| !h.is_empty())
            .map(clean_text),
        comments: comments.into_iter().map(comment).collect(),
    }
}

/// A REST comment id: the digits of a `databaseId`.
fn numeric_id(id: &str) -> Result<u64> {
    id.parse()
        .map_err(|_| ForgeError::Parse(format!("not a GitHub comment id: {id}")))
}

/// A pending review's id as `DraftRef::review_id`: the REST `id` and the GraphQL node id, which
/// the two halves of the API each want.
fn split_review_id(id: &str) -> Result<(u64, &str)> {
    id.split_once(':')
        .and_then(|(n, node)| Some((n.parse().ok()?, node)))
        .filter(|(_, node)| !node.is_empty())
        .ok_or_else(|| ForgeError::Parse(format!("not a GitHub review id: {}", clean_line(id))))
}

#[derive(Deserialize, Default)]
struct RestReview {
    id: u64,
    #[serde(default)]
    node_id: String,
    #[serde(default)]
    state: String,
    user: Option<User>,
}

#[derive(Deserialize, Default)]
struct RestComment {
    #[serde(default)]
    node_id: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    body: String,
    line: Option<u32>,
    original_line: Option<u32>,
    start_line: Option<u32>,
    original_start_line: Option<u32>,
    side: Option<String>,
    #[serde(default)]
    commit_id: String,
    in_reply_to_id: Option<u64>,
}

/// A REST comment of a pending review. Its anchor's commit is the comment's `commit_id`: the one
/// GitHub anchored the thread on.
fn pending_comment(c: &RestComment) -> PendingComment {
    let end = c.line.or(c.original_line).filter(|&l| l >= 1);
    let path = clean_line(&c.path);
    let anchor = end.filter(|_| !path.is_empty()).map(|end_line| Anchor {
        path,
        side: if c.side.as_deref() == Some("LEFT") {
            Side::Left
        } else {
            Side::Right
        },
        start_line: c
            .start_line
            .or(c.original_start_line)
            .filter(|&s| s >= 1)
            .map_or(end_line, |s| s.min(end_line)),
        end_line,
        commit: clean_line(&c.commit_id),
    });
    PendingComment {
        id: clean_line(&c.node_id),
        reply_to: c.in_reply_to_id.map(|id| ReplyTo::Comment(id.to_string())),
        anchor,
        body: clean_text(&c.body),
        resolve: false,
    }
}

impl GitHub {
    /// A new pending review at `head_sha`, or the user's existing one when GitHub says there is
    /// one (422), as `DraftRef::review_id`.
    ///
    /// Blocking: call from spawn_blocking.
    fn start_pending(&self, repo: &RepoRef, number: u64, head_sha: &str) -> Result<String> {
        let url = format!("{}/{number}/reviews", Self::pulls(repo));
        match self.client.post(&url, &json!({ "commit_id": head_sha }).to_string()) {
            Ok(r) => {
                let review: RestReview = super::http::parse_json(&r.body)?;
                Ok(format!("{}:{}", review.id, clean_line(&review.node_id)))
            }
            Err(ForgeError::Status { status: 422, message }) => {
                let user = self.current_user()?;
                self.pending_review(repo, number, &user)?
                    .map(|p| p.id)
                    .ok_or(ForgeError::Status { status: 422, message })
            }
            Err(e) => Err(e),
        }
    }
}

impl Forge for GitHub {
    /// GitHub filters by head (`owner:branch`) with the repository's own owner, so only branches of
    /// that owner's repositories match, never a fork's; the result is filtered again by branch name
    /// and head repository (a repository of the same owner but another name is left out).
    ///
    /// Blocking: call from spawn_blocking.
    fn find_open(&self, repo: &RepoRef, head_branch: &str) -> Result<Vec<MergeRequest>> {
        let owner = repo.path.split('/').next().unwrap_or_default();
        let head = format!("{}:{}", encode_segment(owner), encode_segment(head_branch));
        let pulls: Vec<Pull> = self
            .client
            .get_all(&format!("{}?state=open&head={head}", Self::pulls(repo)))?;
        Ok(pulls
            .into_iter()
            .map(merge_request)
            .filter(|m| m.head.name == head_branch && super::in_repo(m, repo))
            .collect())
    }

    /// The merge base comes from the compare endpoint; if that fails the field stays `None` and
    /// the caller computes it from the local clone.
    ///
    /// Blocking: call from spawn_blocking.
    fn merge_request(&self, repo: &RepoRef, number: u64) -> Result<MergeRequest> {
        let mut mr = merge_request(self.client.get_json(&format!("{}/{number}", Self::pulls(repo)))?);
        if !mr.base.sha.is_empty() && !mr.head.sha.is_empty() {
            let url = format!(
                "{}/compare/{}...{}?per_page=1",
                Self::repo_path(repo),
                encode_segment(&mr.base.sha),
                encode_segment(&mr.head.sha)
            );
            match self.client.get_json::<Compare>(&url) {
                Ok(c) => {
                    mr.merge_base = c
                        .merge_base_commit
                        .and_then(|c| c.sha)
                        .map(|s| clean_line(&s))
                        .filter(|s| !s.is_empty())
                }
                Err(e) => tracing::warn!("merge base of #{number}: {e}"),
            }
        }
        Ok(mr)
    }

    /// Review threads (GraphQL, with resolved and outdated state), submitted reviews' summaries
    /// and the plain comments under the title. Pending reviews are skipped: jed keeps its own
    /// drafts. A thread's comments beyond the first 100 are not read (paging them would take a
    /// `node(id:)` query per thread), and neither is anything past [`MAX_GRAPHQL_PAGES`] pages of a
    /// connection: both come back in `MrComments::notes` for the lookup to log.
    ///
    /// Blocking: call from spawn_blocking.
    fn comments(&self, repo: &RepoRef, mr: &MergeRequest) -> Result<MrComments> {
        let number = mr.number;
        let (threads, threads_cut): (Vec<GqlThread>, bool) =
            self.nodes(repo, number, "reviewThreads", THREAD_FIELDS)?;
        let (reviews, reviews_cut): (Vec<GqlReview>, bool) = self.nodes(repo, number, "reviews", REVIEW_FIELDS)?;
        let (issue, issue_cut): (Vec<GqlComment>, bool) = self.nodes(repo, number, "comments", ISSUE_COMMENT_FIELDS)?;
        let mut notes = Vec::new();
        for (what, cut) in [
            ("review threads", threads_cut),
            ("reviews", reviews_cut),
            ("comments", issue_cut),
        ] {
            if cut {
                notes.push(format!("{what} past the first {MAX_GRAPHQL_PAGES} pages are not shown"));
            }
        }
        let long = threads.iter().filter(|t| t.comments.page_info.has_next_page).count();
        if long > 0 {
            notes.push(format!(
                "{long} thread(s) have more than 100 comments; only the first 100 are shown"
            ));
        }
        let mut conversation: Vec<Comment> = reviews
            .iter()
            .filter(|r| r.state.as_deref() != Some("PENDING"))
            .filter(|r| !r.body.as_deref().unwrap_or_default().trim().is_empty())
            .map(|r| Comment {
                id: format!("review-{}", r.database_id.unwrap_or_default()),
                author: ghost(r.author.as_ref().and_then(|a| a.login.as_ref())),
                body: clean_text(r.body.as_deref().unwrap_or_default()),
                created_at: text(&r.submitted_at),
                updated_at: text(&r.submitted_at),
                url: text(&r.url),
            })
            .collect();
        conversation.extend(issue.iter().map(comment));
        conversation.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        // DISMISSED is a state of its own: it clears its author's verdict.
        let mut verdicts: Vec<&GqlReview> = reviews.iter().collect();
        verdicts.sort_by(|a, b| a.submitted_at.cmp(&b.submitted_at));
        let approvals = latest_approvals_with_ids(verdicts.into_iter().map(|r| {
            let state = match r.state.as_deref() {
                Some("APPROVED") => Verdict::Set(ApprovalState::Approved),
                Some("CHANGES_REQUESTED") => Verdict::Set(ApprovalState::ChangesRequested),
                Some("DISMISSED") => Verdict::Clear,
                _ => Verdict::Keep,
            };
            (
                ghost(r.author.as_ref().and_then(|a| a.login.as_ref())),
                state,
                r.database_id.map(|i| i.to_string()),
            )
        }));
        Ok(MrComments {
            threads: threads.into_iter().map(thread).collect(),
            conversation,
            approvals,
            notes,
        })
    }

    /// Blocking: call from spawn_blocking.
    fn current_user(&self) -> Result<String> {
        let user: User = self.client.get_json("/user")?;
        let login = text(&user.login);
        if login.is_empty() {
            return Err(ForgeError::Parse("no login in the answer".into()));
        }
        Ok(login)
    }

    /// One review holding every comment: GitHub takes them together, so they all succeed or all
    /// fail (a line outside the diff fails the whole request).
    ///
    /// Blocking: call from spawn_blocking.
    fn submit_review(&self, repo: &RepoRef, number: u64, head_sha: &str, review: &DraftReview) -> SubmitOutcome {
        let comments: Vec<Value> = review
            .comments
            .iter()
            .map(|c| {
                let a = &c.anchor;
                let mut v = json!({
                    "path": a.path,
                    "body": c.body,
                    "line": a.end_line,
                    "side": side_name(a.side),
                });
                if a.start_line < a.end_line {
                    v["start_line"] = json!(a.start_line);
                    v["start_side"] = json!(side_name(a.side));
                }
                v
            })
            .collect();
        // GitHub requires a body for COMMENT and REQUEST_CHANGES: the user's summary, else a default.
        let overall = review_body(review.event, &review.body, true);
        let body = json!({
            "commit_id": head_sha,
            "body": overall,
            "event": event_name(review.event),
            "comments": comments,
        });
        let url = format!("{}/{number}/reviews", Self::pulls(repo));
        SubmitOutcome::atomic(review.comments.len(), self.client.post(&url, &body.to_string()))
    }

    /// The user's `PENDING` review (`GET …/reviews`: state `PENDING`, the user's login; GitHub
    /// lists a pending review only to its author) and its comments (`GET …/reviews/{id}/comments`,
    /// whose `in_reply_to_id` marks a reply).
    ///
    /// Blocking: call from spawn_blocking.
    fn pending_review(&self, repo: &RepoRef, number: u64, user: &str) -> Result<Option<PendingReview>> {
        let pulls = Self::pulls(repo);
        let reviews: Vec<RestReview> = self.client.get_all(&format!("{pulls}/{number}/reviews"))?;
        let Some(review) = reviews.iter().find(|r| {
            r.state == "PENDING" && text(&r.user.as_ref().and_then(|u| u.login.clone())).eq_ignore_ascii_case(user)
        }) else {
            return Ok(None);
        };
        let comments: Vec<RestComment> = self
            .client
            .get_all(&format!("{pulls}/{number}/reviews/{}/comments", review.id))?;
        Ok(Some(PendingReview {
            id: format!("{}:{}", review.id, clean_line(&review.node_id)),
            comments: comments.iter().map(pending_comment).collect(),
        }))
    }

    /// Starts the pending review when `pending` is `None` (`POST …/reviews` with no `event`, which
    /// leaves it PENDING; GitHub allows one per user and pull request, so a 422 means one exists
    /// already, started on the website: it is found and used). The comment is then added by
    /// GraphQL: `addPullRequestReviewThread` (path, line, side, startLine, startSide) or
    /// `addPullRequestReviewThreadReply`. GitHub resolves a thread at once, never as part of a
    /// pending review, so a reply that resolves is refused.
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
        if let NewDraft::Reply { resolve: true, .. } = draft {
            return Err(ForgeError::Config(
                "GitHub resolves a thread at once, not with a pending review; resolve it after submitting".into(),
            ));
        }
        let review_id = match pending {
            Some(id) => id.to_string(),
            None => self.start_pending(repo, number, head_sha)?,
        };
        let (_, node) = split_review_id(&review_id)?;
        let result = match draft {
            NewDraft::Line(c) => {
                let a = &c.anchor;
                let ranged = a.start_line < a.end_line;
                let side = side_name(a.side);
                self.client.graphql(
                    ADD_THREAD_MUTATION,
                    json!({
                        "review": node, "path": a.path, "body": c.body, "line": a.end_line, "side": side,
                        "startLine": ranged.then_some(a.start_line), "startSide": ranged.then_some(side),
                    }),
                )?
            }
            NewDraft::Reply { thread_id, body, .. } => self.client.graphql(
                ADD_REPLY_MUTATION,
                json!({ "review": node, "thread": thread_id, "body": body }),
            )?,
        };
        let comment = result
            .pointer("/addPullRequestReviewThread/thread/comments/nodes/0/id")
            .or_else(|| result.pointer("/addPullRequestReviewThreadReply/comment/id"))
            .and_then(Value::as_str)
            .ok_or_else(|| ForgeError::Parse("GitHub did not return the new comment".into()))?;
        Ok(DraftRef {
            review_id,
            comment_id: comment.to_string(),
        })
    }

    /// `updatePullRequestReviewComment` by node id.
    ///
    /// Blocking: call from spawn_blocking.
    fn edit_draft(
        &self,
        _repo: &RepoRef,
        _number: u64,
        draft: &DraftRef,
        body: &str,
        _range: Option<(u32, u32)>,
        _lines: (u32, u32),
    ) -> Result<()> {
        self.client
            .graphql(UPDATE_COMMENT_MUTATION, json!({ "id": draft.comment_id, "body": body }))
            .map(|_| ())
    }

    /// `deletePullRequestReviewComment` by node id.
    ///
    /// Blocking: call from spawn_blocking.
    fn delete_draft(&self, _repo: &RepoRef, _number: u64, draft: &DraftRef) -> Result<()> {
        self.client
            .graphql(DELETE_COMMENT_MUTATION, json!({ "id": draft.comment_id }))
            .map(|_| ())
    }

    /// `POST …/reviews/{id}/events` with `event` and `body` (GitHub needs a body for COMMENT and
    /// REQUEST_CHANGES: `review_body`).
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
        let (id, _) = split_review_id(review_id)?;
        let body = json!({ "event": event_name(event), "body": review_body(event, body, true) });
        self.client
            .post(
                &format!("{}/{number}/reviews/{id}/events", Self::pulls(repo)),
                &body.to_string(),
            )
            .map(|_| ())
    }

    /// `DELETE …/reviews/{id}` (a pending review only).
    ///
    /// Blocking: call from spawn_blocking.
    fn discard_pending(&self, repo: &RepoRef, number: u64, review_id: &str) -> Result<()> {
        let (id, _) = split_review_id(review_id)?;
        self.client
            .delete(&format!("{}/{number}/reviews/{id}", Self::pulls(repo)))
            .map(|_| ())
    }

    /// `POST …/pulls/{n}/comments` (`commit_id`, `path`, `line`, `side`, and `start_line` /
    /// `start_side` for a range): one comment, posted at once. While the user has a pending
    /// review GitHub answers 422 ("one pending review per pull request": the comment would have
    /// to join it), which is turned into a message that says so.
    ///
    /// Blocking: call from spawn_blocking.
    fn post_comment(&self, repo: &RepoRef, number: u64, head_sha: &str, c: &DraftComment) -> Result<()> {
        let a = &c.anchor;
        let mut body = json!({
            "commit_id": head_sha,
            "path": a.path,
            "body": c.body,
            "line": a.end_line,
            "side": side_name(a.side),
        });
        if a.start_line < a.end_line {
            body["start_line"] = json!(a.start_line);
            body["start_side"] = json!(side_name(a.side));
        }
        match self
            .client
            .post(&format!("{}/{number}/comments", Self::pulls(repo)), &body.to_string())
        {
            Err(ForgeError::Status { status: 422, message }) => {
                // The refusal for a pending review is a 422 like a line outside the diff: say
                // what it is when the message does, else add the possibility.
                let hint = "GitHub can't post a single comment while you have a pending review; add it to the review or submit that first";
                Err(ForgeError::Config(
                    if message.to_lowercase().contains("pending review") {
                        hint.into()
                    } else {
                        format!("server answered 422: {message} (a pending review of yours also causes this: {hint})")
                    },
                ))
            }
            r => r.map(|_| ()),
        }
    }

    /// By thread node id (GraphQL): the REST reply call needs a comment id, which a thread id is not.
    ///
    /// Blocking: call from spawn_blocking.
    fn reply(&self, _repo: &RepoRef, _number: u64, thread_id: &str, body: &str, _lines: (u32, u32)) -> Result<()> {
        self.client
            .graphql(REPLY_MUTATION, json!({ "thread": thread_id, "body": body }))
            .map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn comment(&self, repo: &RepoRef, number: u64, body: &str) -> Result<()> {
        let url = format!("{}/issues/{number}/comments", Self::repo_path(repo));
        self.client.post(&url, &json!({ "body": body }).to_string()).map(|_| ())
    }

    /// Plain comments live under `issues/comments`, review comments under `pulls/comments`.
    ///
    /// Blocking: call from spawn_blocking.
    fn edit_conversation_comment(&self, repo: &RepoRef, comment_id: &str, body: &str) -> Result<()> {
        let id = numeric_id(comment_id)?;
        let url = format!("{}/issues/comments/{id}", Self::repo_path(repo));
        self.client
            .patch(&url, &json!({ "body": body }).to_string())
            .map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn delete_conversation_comment(&self, repo: &RepoRef, comment_id: &str) -> Result<()> {
        let id = numeric_id(comment_id)?;
        let url = format!("{}/issues/comments/{id}", Self::repo_path(repo));
        self.client.delete(&url).map(|_| ())
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

    /// Blocking: call from spawn_blocking.
    fn set_resolved(&self, _repo: &RepoRef, thread_id: &str, resolved: bool) -> Result<()> {
        let mutation = if resolved { RESOLVE_MUTATION } else { UNRESOLVE_MUTATION };
        self.client
            .graphql(mutation, json!({ "thread": thread_id }))
            .map(|_| ())
    }

    /// GitHub keeps the range natively, so `range` is not used.
    ///
    /// Blocking: call from spawn_blocking.
    fn edit_comment(
        &self,
        repo: &RepoRef,
        comment_id: &str,
        body: &str,
        _range: Option<(u32, u32)>,
        _lines: (u32, u32),
    ) -> Result<()> {
        let id = numeric_id(comment_id)?;
        let url = format!("{}/comments/{id}", Self::pulls(repo));
        self.client
            .patch(&url, &json!({ "body": body }).to_string())
            .map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn delete_comment(&self, repo: &RepoRef, comment_id: &str) -> Result<()> {
        let id = numeric_id(comment_id)?;
        self.client
            .delete(&format!("{}/comments/{id}", Self::pulls(repo)))
            .map(|_| ())
    }

    /// Dismisses the review behind the verdict: `PUT /repos/{o}/{r}/pulls/{n}/reviews/{id}/dismissals`
    /// with `{"message": …, "event": "DISMISS"}` (the id is the review's `databaseId`). Works on
    /// approvals and change requests; the token needs write access to the repository (a plain
    /// reader gets 403/404, and a branch protection may refuse: the message is passed on).
    ///
    /// Blocking: call from spawn_blocking.
    fn withdraw(&self, repo: &RepoRef, number: u64, approval: &Approval) -> Result<()> {
        let id = approval
            .id
            .as_deref()
            .ok_or_else(|| ForgeError::Config("the review's id is unknown".into()))?;
        let id = numeric_id(id)?;
        let body = serde_json::json!({ "message": WITHDRAW_MESSAGE, "event": "DISMISS" }).to_string();
        self.client
            .put(
                &format!("{}/{number}/reviews/{id}/dismissals", Self::pulls(repo)),
                &body,
            )
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Kind;
    use super::super::config::Resolved;
    use super::super::fake::{FakeServer, Reply};
    use super::*;
    use crate::forge::DraftComment;

    const PR: &str = r#"{"number":7,"title":"Add \u001b[2Jthing","body":null,"state":"open","draft":true,
        "user":{"login":"ann"},"html_url":"https://g/o/r/pull/7",
        "base":{"ref":"main","sha":"b0","repo":{"full_name":"o/r"}},
        "head":{"ref":"feat","sha":"h0","repo":{"full_name":"O/r"}}}"#;

    #[test]
    fn conversation_comments_use_the_issue_endpoints() {
        let (server, gh, repo) = setup(vec![
            ("POST /api/v3/repos/o/r/issues/7/comments", Reply::json("{}")),
            ("PATCH /api/v3/repos/o/r/issues/comments/20", Reply::json("{}")),
            ("DELETE /api/v3/repos/o/r/issues/comments/20", Reply::status(204)),
        ]);
        gh.comment(&repo, 7, "hello").unwrap();
        gh.edit_conversation_comment(&repo, "20", "new").unwrap();
        gh.delete_conversation_comment(&repo, "20").unwrap();
        let seen = server.requests();
        assert_eq!(seen[0].body, r#"{"body":"hello"}"#);
        assert_eq!(seen[1].body, r#"{"body":"new"}"#);
        assert_eq!(seen[2].method, "DELETE");
        assert!(matches!(
            gh.edit_conversation_comment(&repo, "review-9", "x"),
            Err(ForgeError::Parse(_))
        ));
        assert!(matches!(
            gh.delete_conversation_comment(&repo, "../x"),
            Err(ForgeError::Parse(_))
        ));
    }

    #[test]
    fn review_links_b_withdraw_puts_a_dismissal_by_database_id() {
        let (server, gh, repo) = setup(vec![
            ("PUT /api/v3/repos/o/r/pulls/7/reviews/31/dismissals", Reply::json("{}")),
            (
                "PUT /api/v3/repos/o/r/pulls/7/reviews/32/dismissals",
                Reply::status(403),
            ),
        ]);
        let ok = Approval {
            author: "me".into(),
            state: ApprovalState::ChangesRequested,
            id: Some("31".into()),
        };
        gh.withdraw(&repo, 7, &ok).unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[0].method, "PUT");
        let body: serde_json::Value = serde_json::from_str(&reqs[0].body).unwrap();
        assert_eq!(body["event"], "DISMISS");
        assert_eq!(body["message"], crate::forge::WITHDRAW_MESSAGE);
        let refused = Approval {
            id: Some("32".into()),
            ..ok.clone()
        };
        assert!(gh.withdraw(&repo, 7, &refused).unwrap_err().to_string().contains("403"));
        assert!(gh.withdraw(&repo, 7, &Approval { id: None, ..ok }).is_err());
        assert_eq!(server.requests().len(), 2);
    }

    fn setup(routes: Vec<(&str, Reply)>) -> (FakeServer, GitHub, RepoRef) {
        let server = FakeServer::start(routes.into_iter().map(|(p, r)| (p.to_string(), r)).collect());
        let resolved = Resolved {
            host: "fake".into(),
            kind: Kind::GitHub,
            api_url: format!("{}/api/v3", server.url()),
            token_env: "T".into(),
            token: None,
            allow_http: true,
        };
        let client = Client::new(&resolved, |_| Some("tok".into())).unwrap();
        (
            server,
            GitHub::new(client),
            RepoRef {
                host: "fake".into(),
                path: "o/r".into(),
            },
        )
    }

    fn data(connection: &str, nodes: &str, next: Option<&str>) -> Reply {
        let page = match next {
            Some(c) => format!("{{\"hasNextPage\":true,\"endCursor\":\"{c}\"}}"),
            None => "{\"hasNextPage\":false,\"endCursor\":null}".into(),
        };
        Reply::json(&format!(
            "{{\"data\":{{\"repository\":{{\"pullRequest\":{{\"{connection}\":{{\"pageInfo\":{page},\"nodes\":{nodes}}}}}}}}}}}"
        ))
    }

    #[test]
    fn a_fork_request_on_the_same_branch_name_is_left_out() {
        let fork = PR
            .replace("\"number\":7", "\"number\":8")
            .replace("\"full_name\":\"O/r\"}}}", "\"full_name\":\"fork/r\"}}}");
        assert!(fork.contains("fork/r"));
        let (_s, gh, repo) = setup(vec![(
            "/api/v3/repos/o/r/pulls?state=open&head=o:feat&per_page=100",
            Reply::json(&format!("[{fork},{PR}]")),
        )]);
        let found = gh.find_open(&repo, "feat").unwrap();
        assert_eq!(found.iter().map(|m| m.number).collect::<Vec<_>>(), [7]);
    }

    #[test]
    fn finds_open_requests_by_head_branch() {
        let other = PR
            .replace("\"number\":7", "\"number\":8")
            .replace("\"ref\":\"feat\"", "\"ref\":\"other\"");
        let (server, gh, repo) = setup(vec![(
            "/api/v3/repos/o/r/pulls?state=open&head=o:feat&per_page=100",
            Reply::json(&format!("[{PR},{other}]")),
        )]);
        let found = gh.find_open(&repo, "feat").unwrap();
        assert_eq!(found.len(), 1);
        let m = &found[0];
        assert_eq!(
            (m.number, m.state, m.author.as_str(), m.draft),
            (7, MrState::Open, "ann", true)
        );
        assert_eq!(m.title, "Add [2Jthing");
        assert_eq!(m.body, "", "a null body is empty");
        assert_eq!(m.head_repo.as_deref(), Some("O/r"));
        let r = &server.requests()[0];
        assert_eq!(r.header("authorization"), Some("Bearer tok"));
        assert_eq!(r.header("accept"), Some("application/vnd.github+json"));
    }

    #[test]
    fn merge_request_takes_its_merge_base_from_compare() {
        let merged = PR.replace("\"state\":\"open\"", "\"state\":\"closed\",\"merged\":true");
        let (server, gh, repo) = setup(vec![
            ("/api/v3/repos/o/r/pulls/7", Reply::json(&merged)),
            (
                "/api/v3/repos/o/r/compare/b0...h0",
                Reply::json(r#"{"merge_base_commit":{"sha":"mb111"},"commits":[]}"#),
            ),
        ]);
        let m = gh.merge_request(&repo, 7).unwrap();
        assert_eq!((m.state, m.merge_base.as_deref()), (MrState::Merged, Some("mb111")));
        assert_eq!(
            server.requests()[1].target,
            "/api/v3/repos/o/r/compare/b0...h0?per_page=1"
        );
        // A failing compare leaves the merge base to the caller.
        let (_s, gh, repo) = setup(vec![("/api/v3/repos/o/r/pulls/7", Reply::json(PR))]);
        assert_eq!(gh.merge_request(&repo, 7).unwrap().merge_base, None);
        assert!(matches!(
            gh.merge_request(&repo, 9),
            Err(ForgeError::Status { status: 404, .. })
        ));
    }

    const THREADS: &str = r#"[
        {"id":"PRRT_a","isResolved":false,"isOutdated":false,"path":"src/a.rs","line":12,"startLine":10,
         "originalLine":12,"originalStartLine":10,"diffSide":"RIGHT","comments":{"nodes":[
           {"id":"PRRC_1","databaseId":101,"body":"range\u001b[2J","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
            "url":"https://g/c/101","author":{"login":"bob"},"commit":{"oid":"h0"},"originalCommit":{"oid":"h0"},"diffHunk":"@@ -1 +1 @@\n+x\u001b"},
           {"id":"PRRC_2","databaseId":102,"body":"reply","createdAt":"2026-01-02T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z",
            "url":"","author":null,"commit":{"oid":"h0"},"originalCommit":{"oid":"h0"}}]}},
        {"id":"PRRT_b","isResolved":true,"isOutdated":true,"path":"src/b.rs","line":null,"startLine":null,
         "originalLine":5,"originalStartLine":null,"diffSide":"LEFT","comments":{"nodes":[
           {"id":"PRRC_3","databaseId":103,"body":"gone","createdAt":"2026-01-03T00:00:00Z","updatedAt":"2026-01-03T00:00:00Z",
            "url":"","author":{"login":"cy"},"commit":null,"originalCommit":{"oid":"old1"}}]}}]"#;

    #[test]
    fn threads_keep_native_ranges_sides_outdated_and_resolved_state() {
        let (server, gh, repo) = setup(vec![
            ("POST /api/graphql ~reviewThreads", data("reviewThreads", THREADS, None)),
            (
                "POST /api/graphql ~reviews(",
                data(
                    "reviews",
                    r#"[{"databaseId":7,"body":"","state":"CHANGES_REQUESTED","submittedAt":"2026-01-04T00:00:00Z","url":"","author":{"login":"bob"}},
                        {"databaseId":8,"body":"","state":"DISMISSED","submittedAt":"2026-01-04T01:00:00Z","url":"","author":{"login":"bob"}},
                        {"databaseId":9,"body":"overall","state":"COMMENTED","submittedAt":"2026-01-05T00:00:00Z","url":"","author":{"login":"bob"}},
                        {"databaseId":10,"body":"secret","state":"PENDING","submittedAt":null,"url":"","author":{"login":"me"}},
                        {"databaseId":11,"body":"","state":"APPROVED","submittedAt":"2026-01-06T00:00:00Z","url":"","author":{"login":"cy"}}]"#,
                    None,
                ),
            ),
            (
                "POST /api/graphql ~comments(first:100,after:$after){pageInfo{hasNextPage endCursor} nodes{databaseId",
                data(
                    "comments",
                    r#"[{"databaseId":20,"body":"hi","createdAt":"2026-01-04T00:00:00Z","updatedAt":"2026-01-04T00:00:00Z","url":"","author":{"login":"dee"}}]"#,
                    None,
                ),
            ),
        ]);
        let all = gh.comments(&repo, &crate::forge::test_mr(7)).unwrap();
        assert_eq!(all.threads[0].hunk.as_deref(), Some("@@ -1 +1 @@\n+x"), "cleaned");
        assert_eq!(all.threads[1].hunk, None);
        // An approval with an empty body is kept; a COMMENTED review adds none; bob's dismissed change
        // request leaves nothing.
        assert_eq!(
            all.approvals,
            [crate::forge::Approval {
                author: "cy".into(),
                state: ApprovalState::Approved,
                id: Some("11".into()),
            }]
        );
        assert_eq!(all.threads.len(), 2);
        let t = &all.threads[0];
        assert_eq!(t.id, "PRRT_a");
        assert_eq!(
            (t.anchor.start_line, t.anchor.end_line, t.anchor.side),
            (10, 12, Side::Right)
        );
        assert_eq!((t.anchor.path.as_str(), t.anchor.commit.as_str()), ("src/a.rs", "h0"));
        assert!(!t.resolved && !t.outdated);
        assert_eq!(t.comments[0].id, "101");
        assert_eq!(t.comments[0].body, "range[2J");
        assert_eq!(
            (t.comments[1].author.as_str(), t.comments[1].id.as_str()),
            ("ghost", "102")
        );
        let o = &all.threads[1];
        assert!(o.resolved && o.outdated);
        assert_eq!(
            (o.anchor.start_line, o.anchor.end_line, o.anchor.side),
            (5, 5, Side::Left)
        );
        assert_eq!(o.anchor.commit, "old1");
        let conv: Vec<_> = all
            .conversation
            .iter()
            .map(|c| (c.id.as_str(), c.body.as_str()))
            .collect();
        assert_eq!(conv, [("20", "hi"), ("review-9", "overall")]);
        let first = &server.requests()[0];
        assert_eq!(first.target, "/api/graphql");
        assert!(first.body.contains("\"owner\":\"o\"") && first.body.contains("\"number\":7"));
    }

    #[test]
    fn a_thread_without_any_line_is_outdated_and_a_forged_range_is_clamped() {
        let parse = |json: &str| thread(serde_json::from_str::<GqlThread>(json).unwrap());
        let file_level = parse(r#"{"id":"T","path":"a.rs","line":null,"originalLine":null,"diffSide":"RIGHT"}"#);
        assert!(file_level.outdated);
        let forged = parse(r#"{"id":"T","path":"a.rs","line":3,"startLine":99,"diffSide":"RIGHT"}"#);
        assert!(!forged.outdated);
        assert_eq!((forged.anchor.start_line, forged.anchor.end_line), (3, 3));
        // A range from the old side to the new one is anchored on its end line.
        let mixed =
            parse(r#"{"id":"T","path":"a.rs","line":12,"startLine":10,"diffSide":"RIGHT","startDiffSide":"LEFT"}"#);
        assert_eq!(
            (mixed.anchor.start_line, mixed.anchor.end_line, mixed.anchor.side),
            (12, 12, Side::Right)
        );
        let same =
            parse(r#"{"id":"T","path":"a.rs","line":12,"startLine":10,"diffSide":"RIGHT","startDiffSide":"RIGHT"}"#);
        assert_eq!((same.anchor.start_line, same.anchor.end_line), (10, 12));
        let zero = parse(r#"{"id":"T","path":"a.rs","line":0,"startLine":0}"#);
        assert_eq!((zero.anchor.start_line, zero.anchor.end_line), (1, 1));
        let huge = serde_json::from_str::<GqlThread>(r#"{"id":"T","line":99999999999}"#);
        assert!(huge.is_err());
    }

    #[test]
    fn graphql_connections_follow_their_cursors_and_stop_at_the_cap() {
        let (server, gh, repo) = setup(vec![
            (
                "POST /api/graphql ~reviewThreads",
                data("reviewThreads", "[]", Some("C1")),
            ),
            ("POST /api/graphql ~reviewThreads", data("reviewThreads", "[]", None)),
        ]);
        let _: (Vec<GqlThread>, bool) = gh.nodes(&repo, 7, "reviewThreads", THREAD_FIELDS).unwrap();
        let sent = server.requests();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].body.contains("\"after\":null") && sent[1].body.contains("\"after\":\"C1\""));
        // A server that never stops saying there is more is cut off.
        let (server, gh, repo) = setup(vec![("POST /api/graphql", data("reviewThreads", "[]", Some("C")))]);
        let (got, cut): (Vec<GqlThread>, bool) = gh.nodes(&repo, 7, "reviewThreads", THREAD_FIELDS).unwrap();
        assert!(got.is_empty() && cut, "reported as cut");
        assert_eq!(server.requests().len(), MAX_GRAPHQL_PAGES);
    }

    #[test]
    fn the_page_cap_is_only_reported_when_more_pages_remain() {
        // MAX_GRAPHQL_PAGES pages, the last one saying there is nothing more: not cut.
        let pages = (0..MAX_GRAPHQL_PAGES)
            .map(|i| {
                let next = (i + 1 < MAX_GRAPHQL_PAGES).then(|| format!("C{i}"));
                (
                    format!(
                        "POST /api/graphql ~\"after\":{}",
                        if i == 0 {
                            "null".into()
                        } else {
                            format!("\"C{}\"", i - 1)
                        }
                    ),
                    data("reviewThreads", "[]", next.as_deref()),
                )
            })
            .collect::<Vec<_>>();
        let (server, gh, repo) = setup(pages.iter().map(|(p, r)| (p.as_str(), r.clone())).collect());
        let (_, cut): (Vec<GqlThread>, bool) = gh.nodes(&repo, 7, "reviewThreads", THREAD_FIELDS).unwrap();
        assert_eq!(server.requests().len(), MAX_GRAPHQL_PAGES);
        assert!(!cut, "the last page was the end");
    }

    #[test]
    fn a_short_thread_and_no_cut_make_no_notes() {
        let short = r#"[{"id":"T","path":"a.rs","line":3,"diffSide":"RIGHT","comments":{"pageInfo":{"hasNextPage":false},"nodes":[]}}]"#;
        let (_s, gh, repo) = setup(vec![
            ("POST /api/graphql ~reviewThreads(", data("reviewThreads", short, None)),
            ("POST /api/graphql ~reviews(", data("reviews", "[]", None)),
            (
                "POST /api/graphql ~comments(first:100,after",
                data("comments", "[]", None),
            ),
        ]);
        let all = gh.comments(&repo, &crate::forge::test_mr(7)).unwrap();
        assert!(all.notes.is_empty(), "{:?}", all.notes);
    }

    #[test]
    fn a_long_thread_comes_back_as_a_note() {
        let long = r#"[{"id":"T","path":"a.rs","line":3,"diffSide":"RIGHT","comments":{"pageInfo":{"hasNextPage":true},"nodes":[]}}]"#;
        let (_s, gh, repo) = setup(vec![
            ("POST /api/graphql ~reviewThreads(", data("reviewThreads", long, None)),
            ("POST /api/graphql ~reviews(", data("reviews", "[]", None)),
            (
                "POST /api/graphql ~comments(first:100,after",
                data("comments", "[]", None),
            ),
        ]);
        let all = gh.comments(&repo, &crate::forge::test_mr(7)).unwrap();
        assert_eq!(all.notes.len(), 1, "{:?}", all.notes);
        assert!(all.notes[0].contains("1 thread"));
    }

    #[test]
    fn graphql_errors_and_malformed_answers_are_errors() {
        let (_s, gh, repo) = setup(vec![(
            "POST /api/graphql",
            Reply::json(r#"{"errors":[{"message":"Could not resolve to a PullRequest"}]}"#),
        )]);
        let err = gh.comments(&repo, &crate::forge::test_mr(7)).err().unwrap();
        assert!(err.to_string().contains("Could not resolve"), "{err}");
        let (_s, gh, repo) = setup(vec![("POST /api/graphql", Reply::json("not json"))]);
        assert!(matches!(
            gh.comments(&repo, &crate::forge::test_mr(7)),
            Err(ForgeError::Parse(_))
        ));
        let (_s, gh, repo) = setup(vec![(
            "POST /api/graphql",
            Reply::json(r#"{"data":{"repository":null}}"#),
        )]);
        assert!(matches!(
            gh.comments(&repo, &crate::forge::test_mr(7)),
            Err(ForgeError::Parse(_))
        ));
    }

    fn draft(side: Side, start: u32, end: u32, body: &str) -> DraftComment {
        DraftComment {
            anchor: Anchor {
                path: "src/a.rs".into(),
                side,
                start_line: start,
                end_line: end,
                commit: "h0".into(),
            },
            body: body.into(),
            other: (None, None),
        }
    }

    #[test]
    fn submit_sends_one_review_with_line_and_side() {
        let (server, gh, repo) = setup(vec![("POST /api/v3/repos/o/r/pulls/7/reviews", Reply::json("{}"))]);
        let review = DraftReview {
            body: "overall".into(),
            event: ReviewEvent::RequestChanges,
            comments: vec![draft(Side::Right, 10, 12, "range"), draft(Side::Left, 4, 4, "old line")],
        };
        let out = gh.submit_review(&repo, 7, "h0", &review);
        assert_eq!((out.posted, out.error), (vec![0, 1], None));
        let sent: Value = serde_json::from_str(&server.requests()[0].body).unwrap();
        assert_eq!(
            sent,
            json!({
                "commit_id": "h0", "body": "overall", "event": "REQUEST_CHANGES",
                "comments": [
                    {"path": "src/a.rs", "body": "range", "line": 12, "side": "RIGHT", "start_line": 10, "start_side": "RIGHT"},
                    {"path": "src/a.rs", "body": "old line", "line": 4, "side": "LEFT"},
                ]
            })
        );
    }

    #[test]
    fn an_empty_overall_body_gets_a_default_except_for_approve() {
        for (event, want_event, want_body) in [
            (ReviewEvent::Comment, "COMMENT", "Review comments"),
            (ReviewEvent::RequestChanges, "REQUEST_CHANGES", "Changes requested"),
            (ReviewEvent::Approve, "APPROVE", ""),
        ] {
            let (server, gh, repo) = setup(vec![("POST /api/v3/repos/o/r/pulls/7/reviews", Reply::json("{}"))]);
            let review = DraftReview {
                body: "  ".into(),
                event,
                comments: vec![draft(Side::Right, 3, 3, "x")],
            };
            let out = gh.submit_review(&repo, 7, "h0", &review);
            assert_eq!(out.error, None);
            let sent: Value = serde_json::from_str(&server.requests()[0].body).unwrap();
            assert_eq!(
                sent,
                json!({
                    "commit_id": "h0", "body": if want_body.is_empty() { "  " } else { want_body }, "event": want_event,
                    "comments": [{"path": "src/a.rs", "body": "x", "line": 3, "side": "RIGHT"}]
                })
            );
        }
    }

    #[test]
    fn a_failed_submit_fails_every_comment_without_the_token() {
        let (_s, gh, repo) = setup(vec![(
            "POST /api/v3/repos/o/r/pulls/7/reviews",
            Reply::status(422).with_body(r#"{"message":"Unprocessable tok"}"#),
        )]);
        let review = DraftReview {
            body: String::new(),
            event: ReviewEvent::Comment,
            comments: vec![draft(Side::Right, 1, 1, "a"), draft(Side::Right, 2, 2, "b")],
        };
        let out = gh.submit_review(&repo, 7, "h0", &review);
        assert!(out.posted.is_empty() && out.failed.len() == 2);
        let err = out.error.unwrap();
        assert!(err.contains("422") && err.contains("Unprocessable <token>"), "{err}");
    }

    #[test]
    fn replies_resolves_edits_and_deletes() {
        let (server, gh, repo) = setup(vec![
            (
                "POST /api/graphql ~addPullRequestReviewThreadReply",
                Reply::json(r#"{"data":{"x":{}}}"#),
            ),
            (
                "POST /api/graphql ~resolveReviewThread",
                Reply::json(r#"{"data":{"x":{}}}"#),
            ),
            ("PATCH /api/v3/repos/o/r/pulls/comments/101", Reply::json("{}")),
            ("DELETE /api/v3/repos/o/r/pulls/comments/101", Reply::status(204)),
        ]);
        gh.reply(&repo, 7, "PRRT_a", "thanks", (1, 1)).unwrap();
        gh.set_resolved(&repo, "PRRT_a", true).unwrap();
        gh.set_resolved(&repo, "PRRT_a", false).unwrap();
        gh.edit_comment(&repo, "101", "new", Some((1, 2)), (1, 2)).unwrap();
        gh.delete_comment(&repo, "101").unwrap();
        let seen = server.requests();
        let body = |i: usize| serde_json::from_str::<Value>(&seen[i].body).unwrap();
        assert_eq!(body(0)["variables"], json!({"thread": "PRRT_a", "body": "thanks"}));
        assert!(body(1)["query"].as_str().unwrap().contains("resolveReviewThread(input"));
        assert!(body(2)["query"].as_str().unwrap().contains("unresolveReviewThread"));
        assert_eq!(seen[3].body, r#"{"body":"new"}"#);
        assert_eq!(seen[4].method, "DELETE");
        // Ids that are not numbers never reach a URL.
        assert!(matches!(gh.delete_comment(&repo, "../x"), Err(ForgeError::Parse(_))));
        assert!(matches!(
            gh.edit_comment(&repo, "review-9", "x", None, (1, 1)),
            Err(ForgeError::Parse(_))
        ));
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn an_edit_posts_a_plain_suggestion_fence_unchanged_on_a_range() {
        let (server, gh, repo) = setup(vec![("PATCH /api/v3/repos/o/r/pulls/comments/101", Reply::json("{}"))]);
        gh.edit_comment(&repo, "101", "```suggestion\nx\n```", Some((2, 4)), (2, 4))
            .unwrap();
        assert_eq!(server.requests()[0].body, r#"{"body":"```suggestion\nx\n```"}"#);
    }

    const REVIEWS: &str = r#"[{"id":3,"node_id":"PRR_3","state":"APPROVED","user":{"login":"me"}},
        {"id":8,"node_id":"PRR_8","state":"PENDING","user":{"login":"Me"}}]"#;

    #[test]
    fn the_pending_review_and_its_comments_are_read_back_clean() {
        let comments = r#"[
            {"id":101,"node_id":"PRRC_a","path":"a.rs","body":"one \u001b[2J","line":9,"start_line":5,"side":"RIGHT","commit_id":"h0"},
            {"id":102,"node_id":"PRRC_b","path":"a.rs","body":"re","line":null,"original_line":3,"side":"LEFT","commit_id":"h0","in_reply_to_id":55}]"#;
        let (server, gh, repo) = setup(vec![
            ("GET /api/v3/repos/o/r/pulls/7/reviews", Reply::json(REVIEWS)),
            (
                "GET /api/v3/repos/o/r/pulls/7/reviews/8/comments",
                Reply::json(comments),
            ),
        ]);
        let p = gh.pending_review(&repo, 7, "me").unwrap().unwrap();
        assert_eq!(p.id, "8:PRR_8");
        let (a, b) = (&p.comments[0], &p.comments[1]);
        assert_eq!((a.id.as_str(), a.body.as_str()), ("PRRC_a", "one [2J"));
        let anchor = a.anchor.as_ref().unwrap();
        assert_eq!((anchor.start_line, anchor.end_line, anchor.side), (5, 9, Side::Right));
        assert_eq!(anchor.commit, "h0");
        assert_eq!(b.reply_to, Some(crate::forge::ReplyTo::Comment("55".into())));
        assert_eq!(b.anchor.as_ref().map(|x| (x.end_line, x.side)), Some((3, Side::Left)));
        assert_eq!(server.requests().len(), 2);
        assert!(gh.pending_review(&repo, 7, "other").unwrap().is_none());
    }

    #[test]
    fn a_first_draft_starts_a_review_without_an_event_then_adds_a_thread() {
        let (server, gh, repo) = setup(vec![
            (
                "POST /api/v3/repos/o/r/pulls/7/reviews",
                Reply::json(r#"{"id":8,"node_id":"PRR_8","state":"PENDING"}"#),
            ),
            (
                "POST /api/graphql ~addPullRequestReviewThread(",
                Reply::json(
                    r#"{"data":{"addPullRequestReviewThread":{"thread":{"id":"T","comments":{"nodes":[{"id":"PRRC_a"}]}}}}}"#,
                ),
            ),
        ]);
        let d = crate::forge::NewDraft::Line(draft(Side::Left, 5, 9, "fix"));
        let r = gh.add_draft(&repo, 7, "h0", None, &d).unwrap();
        assert_eq!((r.review_id.as_str(), r.comment_id.as_str()), ("8:PRR_8", "PRRC_a"));
        let seen = server.requests();
        assert_eq!(
            seen[0].body, r#"{"commit_id":"h0"}"#,
            "no event: the review stays pending"
        );
        let v: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(
            v["variables"],
            json!({"review":"PRR_8","path":"src/a.rs","body":"fix","line":9,"side":"LEFT","startLine":5,"startSide":"LEFT"})
        );
        // One line: no start.
        let one = crate::forge::NewDraft::Line(draft(Side::Right, 4, 4, "x"));
        gh.add_draft(&repo, 7, "h0", Some("8:PRR_8"), &one).unwrap();
        let seen = server.requests();
        assert_eq!(seen.len(), 3, "an existing review is used without creating one");
        let v: Value = serde_json::from_str(&seen[2].body).unwrap();
        assert!(v["variables"]["startLine"].is_null() && v["variables"]["startSide"].is_null());
        assert_eq!(v["variables"]["side"], "RIGHT");
    }

    #[test]
    fn a_single_comment_posts_at_its_anchor_and_a_pending_review_refuses_it() {
        let (server, gh, repo) = setup(vec![(
            "POST /api/v3/repos/o/r/pulls/7/comments",
            Reply::json(r#"{"id":1}"#),
        )]);
        gh.post_comment(&repo, 7, "h0", &draft(Side::Left, 5, 9, "fix"))
            .unwrap();
        gh.post_comment(&repo, 7, "h0", &draft(Side::Right, 4, 4, "x")).unwrap();
        let seen = server.requests();
        let a: Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(
            a,
            json!({"commit_id":"h0","path":"src/a.rs","body":"fix","line":9,"side":"LEFT","start_line":5,"start_side":"LEFT"})
        );
        let b: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(b["side"], "RIGHT");
        assert!(b.get("start_line").is_none() && b.get("start_side").is_none());
        server.set(
            "POST /api/v3/repos/o/r/pulls/7/comments",
            Reply::status(422).with_body(
                r#"{"message":"Unprocessable Entity: User can only have one pending review per pull request"}"#,
            ),
        );
        let err = gh
            .post_comment(&repo, 7, "h0", &draft(Side::Right, 4, 4, "x"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pending review") && err.contains("GitHub"), "{err}");
        assert!(
            !err.contains("server answered 422"),
            "said plainly, not as a guess: {err}"
        );
        // Any other 422 keeps the server's words and adds the pending review as a possibility.
        server.set(
            "POST /api/v3/repos/o/r/pulls/7/comments",
            Reply::status(422).with_body(r#"{"message":"Validation Failed"}"#),
        );
        let err = gh
            .post_comment(&repo, 7, "h0", &draft(Side::Right, 4, 4, "x"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("server answered 422: Validation Failed"), "{err}");
        assert!(err.contains("a pending review of yours also causes this"), "{err}");
    }

    #[test]
    fn a_pending_review_started_on_the_website_is_found_and_reused() {
        let (server, gh, repo) = setup(vec![
            (
                "POST /api/v3/repos/o/r/pulls/7/reviews",
                Reply::status(422).with_body(r#"{"message":"User can only have one pending review per pull request"}"#),
            ),
            ("GET /api/v3/user", Reply::json(r#"{"login":"me"}"#)),
            ("GET /api/v3/repos/o/r/pulls/7/reviews", Reply::json(REVIEWS)),
            ("GET /api/v3/repos/o/r/pulls/7/reviews/8/comments", Reply::json("[]")),
            (
                "POST /api/graphql ~addPullRequestReviewThreadReply",
                Reply::json(r#"{"data":{"addPullRequestReviewThreadReply":{"comment":{"id":"PRRC_r"}}}}"#),
            ),
        ]);
        let reply = crate::forge::NewDraft::Reply {
            thread_id: "PRRT_x".into(),
            body: "thanks".into(),
            lines: (1, 1),
            resolve: false,
        };
        let r = gh.add_draft(&repo, 7, "h0", None, &reply).unwrap();
        assert_eq!((r.review_id.as_str(), r.comment_id.as_str()), ("8:PRR_8", "PRRC_r"));
        let last = server.requests().pop().unwrap();
        let v: Value = serde_json::from_str(&last.body).unwrap();
        assert_eq!(
            v["variables"],
            json!({"review":"PRR_8","thread":"PRRT_x","body":"thanks"})
        );
    }

    #[test]
    fn a_422_without_a_pending_review_of_the_user_keeps_the_original_error() {
        let (server, gh, repo) = setup(vec![
            (
                "POST /api/v3/repos/o/r/pulls/7/reviews",
                Reply::status(422).with_body(r#"{"message":"commit_id is not part of the pull request"}"#),
            ),
            ("GET /api/v3/user", Reply::json(r#"{"login":"nobody"}"#)),
            ("GET /api/v3/repos/o/r/pulls/7/reviews", Reply::json(REVIEWS)),
        ]);
        let d = crate::forge::NewDraft::Line(draft(Side::Right, 1, 1, "a"));
        let err = gh.add_draft(&repo, 7, "h0", None, &d).unwrap_err().to_string();
        assert!(err.contains("422") && err.contains("commit_id"), "{err}");
        assert!(
            server.requests().iter().all(|r| !r.target.contains("graphql")),
            "nothing is added to someone else's review"
        );
    }

    #[test]
    fn a_reply_that_resolves_is_refused_and_other_errors_pass_through() {
        let (server, gh, repo) = setup(vec![(
            "POST /api/v3/repos/o/r/pulls/7/reviews",
            Reply::status(403).with_body(r#"{"message":"nope"}"#),
        )]);
        let reply = crate::forge::NewDraft::Reply {
            thread_id: "PRRT_x".into(),
            body: "ok".into(),
            lines: (1, 1),
            resolve: true,
        };
        let err = gh.add_draft(&repo, 7, "h0", None, &reply).unwrap_err().to_string();
        assert!(err.contains("GitHub resolves a thread at once"), "{err}");
        assert!(server.requests().is_empty(), "refused before any request");
        let line = crate::forge::NewDraft::Line(draft(Side::Right, 1, 1, "a"));
        let err = gh.add_draft(&repo, 7, "h0", None, &line).unwrap_err().to_string();
        assert!(err.contains("403") && err.contains("nope"), "{err}");
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn drafts_are_edited_deleted_submitted_and_discarded() {
        let (server, gh, repo) = setup(vec![
            (
                "POST /api/graphql ~updatePullRequestReviewComment",
                Reply::json(r#"{"data":{"x":{}}}"#),
            ),
            (
                "POST /api/graphql ~deletePullRequestReviewComment",
                Reply::json(r#"{"data":{"x":{}}}"#),
            ),
            ("POST /api/v3/repos/o/r/pulls/7/reviews/8/events", Reply::json("{}")),
            ("DELETE /api/v3/repos/o/r/pulls/7/reviews/8", Reply::json("{}")),
        ]);
        let r = crate::forge::DraftRef {
            review_id: "8:PRR_8".into(),
            comment_id: "PRRC_a".into(),
        };
        gh.edit_draft(&repo, 7, &r, "new", None, (1, 1)).unwrap();
        gh.delete_draft(&repo, 7, &r).unwrap();
        gh.submit_pending(&repo, 7, &r.review_id, ReviewEvent::RequestChanges, "")
            .unwrap();
        gh.submit_pending(&repo, 7, &r.review_id, ReviewEvent::Approve, "")
            .unwrap();
        gh.discard_pending(&repo, 7, &r.review_id).unwrap();
        let seen = server.requests();
        let v: Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(v["variables"], json!({"id":"PRRC_a","body":"new"}));
        let v: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(v["variables"], json!({"id":"PRRC_a"}));
        assert_eq!(
            seen[2].body,
            r#"{"body":"Changes requested","event":"REQUEST_CHANGES"}"#
        );
        assert_eq!(seen[3].body, r#"{"body":"","event":"APPROVE"}"#);
        assert_eq!(seen[4].method, "DELETE");
        assert!(matches!(gh.discard_pending(&repo, 7, "8"), Err(ForgeError::Parse(_))));
        assert!(matches!(
            gh.discard_pending(&repo, 7, "x/y:z"),
            Err(ForgeError::Parse(_))
        ));
        assert_eq!(server.requests().len(), 5);
    }

    #[test]
    fn current_user_reads_the_login() {
        let (_s, gh, _r) = setup(vec![("/api/v3/user", Reply::json(r#"{"login":"me"}"#))]);
        assert_eq!(gh.current_user().unwrap(), "me");
    }

    #[test]
    fn blob_id_reads_the_file_sha_and_maps_404_to_none() {
        let (server, forge, repo) = setup(vec![(
            "/api/v3/repos/o/r/contents/src/a%20b.rs?ref=c0ffee",
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
