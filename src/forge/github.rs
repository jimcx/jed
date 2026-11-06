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
//! - The merge base comes from the compare endpoint (`merge_base_commit`).

use serde::Deserialize;
use serde_json::{Value, json};

use super::http::{Client, clean_line, clean_text, encode_segment};
use super::{
    Anchor, BranchRef, Comment, DraftReview, Forge, ForgeError, MergeRequest, MrComments, MrState, RepoRef, Result,
    ReviewEvent, Side, SubmitOutcome, Thread,
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
    ) -> Result<Vec<T>> {
        let (owner, name) = repo
            .path
            .split_once('/')
            .ok_or_else(|| ForgeError::Config(format!("{}: not an owner/repo path", repo.path)))?;
        let query = CONNECTION_QUERY.replace("FIELD", field).replace("SELECTION", selection);
        let mut all = Vec::new();
        let mut after = Value::Null;
        for _ in 0..MAX_GRAPHQL_PAGES {
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
                Some(cursor) if more => after = json!(cursor),
                _ => break,
            }
        }
        Ok(all)
    }
}

const CONNECTION_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!,$after:String){repository(owner:$owner,name:$name){\
    pullRequest(number:$number){FIELD(first:100,after:$after){pageInfo{hasNextPage endCursor} nodes{SELECTION}}}}}";

const THREAD_FIELDS: &str = "id isResolved isOutdated path line startLine originalLine originalStartLine diffSide \
    comments(first:100){nodes{id databaseId body createdAt updatedAt url author{login} commit{oid} originalCommit{oid}}}";
const REVIEW_FIELDS: &str = "databaseId body state submittedAt url author{login}";
const ISSUE_COMMENT_FIELDS: &str = "databaseId body createdAt updatedAt url author{login}";

const REPLY_MUTATION: &str = "mutation($thread:ID!,$body:String!){addPullRequestReviewThreadReply(\
    input:{pullRequestReviewThreadId:$thread,body:$body}){comment{id}}}";
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
struct Nodes<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<Option<T>>,
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
    let start = start.unwrap_or(end).clamp(1, end);
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
        comments: comments.into_iter().map(comment).collect(),
    }
}

/// A REST comment id: the digits of a `databaseId`.
fn numeric_id(id: &str) -> Result<u64> {
    id.parse()
        .map_err(|_| ForgeError::Parse(format!("not a GitHub comment id: {id}")))
}

impl Forge for GitHub {
    /// GitHub filters by head (`owner:branch`); the result is filtered again by branch name, so a
    /// fork's pull request on the same branch name also shows.
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
            .filter(|m| m.head.name == head_branch)
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
    /// drafts. A thread's comments beyond the first 100 are not read.
    ///
    /// Blocking: call from spawn_blocking.
    fn comments(&self, repo: &RepoRef, number: u64) -> Result<MrComments> {
        let threads: Vec<GqlThread> = self.nodes(repo, number, "reviewThreads", THREAD_FIELDS)?;
        let reviews: Vec<GqlReview> = self.nodes(repo, number, "reviews", REVIEW_FIELDS)?;
        let issue: Vec<GqlComment> = self.nodes(repo, number, "comments", ISSUE_COMMENT_FIELDS)?;
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
        Ok(MrComments {
            threads: threads.into_iter().map(thread).collect(),
            conversation,
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
        // GitHub requires a body for COMMENT and REQUEST_CHANGES; jed has no overall-body input.
        let overall = match review.event {
            ReviewEvent::Comment if review.body.trim().is_empty() => "Review comments",
            ReviewEvent::RequestChanges if review.body.trim().is_empty() => "Changes requested",
            _ => review.body.as_str(),
        };
        let body = json!({
            "commit_id": head_sha,
            "body": overall,
            "event": event_name(review.event),
            "comments": comments,
        });
        let url = format!("{}/{number}/reviews", Self::pulls(repo));
        SubmitOutcome::atomic(review.comments.len(), self.client.post(&url, &body.to_string()))
    }

    /// By thread node id (GraphQL): the REST reply call needs a comment id, which a thread id is not.
    ///
    /// Blocking: call from spawn_blocking.
    fn reply(&self, _repo: &RepoRef, _number: u64, thread_id: &str, body: &str) -> Result<()> {
        self.client
            .graphql(REPLY_MUTATION, json!({ "thread": thread_id, "body": body }))
            .map(|_| ())
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
    fn edit_comment(&self, repo: &RepoRef, comment_id: &str, body: &str, _range: Option<(u32, u32)>) -> Result<()> {
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
        "head":{"ref":"feat","sha":"h0","repo":{"full_name":"fork/r"}}}"#;

    fn setup(routes: Vec<(&str, Reply)>) -> (FakeServer, GitHub, RepoRef) {
        let server = FakeServer::start(routes.into_iter().map(|(p, r)| (p.to_string(), r)).collect());
        let resolved = Resolved {
            host: "fake".into(),
            kind: Kind::GitHub,
            api_url: format!("{}/api/v3", server.url()),
            token_env: "T".into(),
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
        assert_eq!(m.head_repo.as_deref(), Some("fork/r"));
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
            "url":"https://g/c/101","author":{"login":"bob"},"commit":{"oid":"h0"},"originalCommit":{"oid":"h0"}},
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
                    r#"[{"databaseId":9,"body":"overall","state":"COMMENTED","submittedAt":"2026-01-05T00:00:00Z","url":"","author":{"login":"bob"}},
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
        let all = gh.comments(&repo, 7).unwrap();
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
        let _: Vec<GqlThread> = gh.nodes(&repo, 7, "reviewThreads", THREAD_FIELDS).unwrap();
        let sent = server.requests();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].body.contains("\"after\":null") && sent[1].body.contains("\"after\":\"C1\""));
        // A server that never stops saying there is more is cut off.
        let (server, gh, repo) = setup(vec![("POST /api/graphql", data("reviewThreads", "[]", Some("C")))]);
        let got: Vec<GqlThread> = gh.nodes(&repo, 7, "reviewThreads", THREAD_FIELDS).unwrap();
        assert!(got.is_empty());
        assert_eq!(server.requests().len(), MAX_GRAPHQL_PAGES);
    }

    #[test]
    fn graphql_errors_and_malformed_answers_are_errors() {
        let (_s, gh, repo) = setup(vec![(
            "POST /api/graphql",
            Reply::json(r#"{"errors":[{"message":"Could not resolve to a PullRequest"}]}"#),
        )]);
        let err = gh.comments(&repo, 7).err().unwrap();
        assert!(err.to_string().contains("Could not resolve"), "{err}");
        let (_s, gh, repo) = setup(vec![("POST /api/graphql", Reply::json("not json"))]);
        assert!(matches!(gh.comments(&repo, 7), Err(ForgeError::Parse(_))));
        let (_s, gh, repo) = setup(vec![(
            "POST /api/graphql",
            Reply::json(r#"{"data":{"repository":null}}"#),
        )]);
        assert!(matches!(gh.comments(&repo, 7), Err(ForgeError::Parse(_))));
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
        gh.reply(&repo, 7, "PRRT_a", "thanks").unwrap();
        gh.set_resolved(&repo, "PRRT_a", true).unwrap();
        gh.set_resolved(&repo, "PRRT_a", false).unwrap();
        gh.edit_comment(&repo, "101", "new", Some((1, 2))).unwrap();
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
            gh.edit_comment(&repo, "review-9", "x", None),
            Err(ForgeError::Parse(_))
        ));
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn current_user_reads_the_login() {
        let (_s, gh, _r) = setup(vec![("/api/v3/user", Reply::json(r#"{"login":"me"}"#))]);
        assert_eq!(gh.current_user().unwrap(), "me");
    }
}
