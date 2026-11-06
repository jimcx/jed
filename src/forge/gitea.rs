//! Gitea (and Forgejo) reads: `/api/v1/repos/{owner}/{repo}/pulls…`. What the API does and does
//! not offer is recorded in `tasks/mr_review.md` ("Forge differences"); the parts that matter here:
//!
//! - A review comment has one line (`position`, or `original_position` once outdated) and no side:
//!   the side is read from the last line of its `diff_hunk` (`-` is the old file).
//! - There is no reply-to field: comments on the same path, line and side form one thread, and
//!   its first comment's id is the thread id (the id the resolve call takes).
//! - The merge base is `merge_base` of the pull request.

use std::collections::HashMap;

use serde::Deserialize;

use super::http::{Client, clean_line, clean_text, encode_segment};
use super::{
    Anchor, BranchRef, ChangedFile, Comment, DraftReview, FileStatus, Forge, ForgeError, Kind, MergeRequest,
    MrComments, MrState, RepoRef, Result, ReviewEvent, Side, SubmitOutcome, Thread,
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

#[derive(Deserialize)]
struct File {
    filename: String,
    #[serde(default)]
    previous_filename: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    additions: u32,
    #[serde(default)]
    deletions: u32,
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

fn file_status(s: &str) -> FileStatus {
    match s {
        "added" => FileStatus::Added,
        "modified" | "changed" => FileStatus::Modified,
        "deleted" | "removed" => FileStatus::Deleted,
        "renamed" => FileStatus::Renamed,
        "copied" => FileStatus::Copied,
        _ => FileStatus::Other,
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
    fn kind(&self) -> Kind {
        Kind::Gitea
    }

    /// Gitea cannot filter by head branch, so this lists the open pull requests and filters.
    ///
    /// Blocking: call from spawn_blocking.
    fn find_open(&self, repo: &RepoRef, head_branch: &str) -> Result<Vec<MergeRequest>> {
        let pulls: Vec<Pull> = self.client.get_all(&format!("{}?state=open", Self::pulls(repo)))?;
        Ok(pulls
            .into_iter()
            .map(merge_request)
            .filter(|m| m.head.name == head_branch)
            .collect())
    }

    /// Blocking: call from spawn_blocking.
    fn merge_request(&self, repo: &RepoRef, number: u64) -> Result<MergeRequest> {
        Ok(merge_request(
            self.client.get_json(&format!("{}/{number}", Self::pulls(repo)))?,
        ))
    }

    /// Blocking: call from spawn_blocking.
    fn files(&self, repo: &RepoRef, number: u64) -> Result<Vec<ChangedFile>> {
        let files: Vec<File> = self.client.get_all(&format!("{}/{number}/files", Self::pulls(repo)))?;
        Ok(files
            .into_iter()
            .map(|f| ChangedFile {
                path: clean_line(&f.filename),
                old_path: Some(clean_line(&f.previous_filename)).filter(|p| !p.is_empty()),
                status: file_status(&f.status),
                additions: f.additions,
                deletions: f.deletions,
            })
            .collect())
    }

    /// Submitted reviews' line comments become threads and their summaries join the plain comments
    /// in the conversation. Pending reviews (the user's own unsent ones) are skipped: jed keeps
    /// its own drafts.
    ///
    /// Blocking: call from spawn_blocking.
    fn comments(&self, repo: &RepoRef, number: u64) -> Result<MrComments> {
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
        Ok(MrComments {
            threads: threads(line_comments),
            conversation,
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
            "body": review.body,
            "event": event_name(review.event),
            "commit_id": head_sha,
            "comments": comments,
        });
        let url = format!("{}/{number}/reviews", Self::pulls(repo));
        match self.client.post(&url, &body.to_string()) {
            Ok(_) => SubmitOutcome {
                posted: (0..review.comments.len()).collect(),
                failed: Vec::new(),
                error: None,
            },
            Err(e) => SubmitOutcome {
                posted: Vec::new(),
                failed: (0..review.comments.len()).map(|i| (i, e.to_string())).collect(),
                error: Some(e.to_string()),
            },
        }
    }

    /// Blocking: call from spawn_blocking.
    fn reply(&self, repo: &RepoRef, number: u64, thread_id: &str, body: &str) -> Result<()> {
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
    fn edit_comment(&self, repo: &RepoRef, comment_id: &str, body: &str, range: Option<(u32, u32)>) -> Result<()> {
        let url = format!("{}/comments/{}", Self::issues(repo), encode_segment(comment_id));
        let body = serde_json::json!({ "body": with_marker(body, range) });
        self.client.patch(&url, &body.to_string()).map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn delete_comment(&self, repo: &RepoRef, comment_id: &str) -> Result<()> {
        let url = format!("{}/comments/{}", Self::issues(repo), encode_segment(comment_id));
        self.client.delete(&url).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::Resolved;
    use super::super::fake::{FakeServer, Reply};
    use super::*;

    const PR: &str = r#"{"number":7,"title":"Add \u001b[2Jthing","body":"a\r\nb","state":"open","draft":false,"merged":false,
        "user":{"login":"ann"},"html_url":"https://g/o/r/pulls/7","merge_base":"mb111",
        "base":{"ref":"main","sha":"b0","repo":{"full_name":"o/r"}},
        "head":{"ref":"feat","sha":"h0","repo":{"full_name":"fork/r"}}}"#;

    fn setup(routes: Vec<(&str, Reply)>) -> (FakeServer, Gitea, RepoRef) {
        let server = FakeServer::start(routes.into_iter().map(|(p, r)| (p.to_string(), r)).collect());
        let resolved = Resolved {
            host: "fake".into(),
            kind: Kind::Gitea,
            api_url: format!("{}/api/v1", server.url()),
            token_env: "T".into(),
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
        assert_eq!(m.head_repo.as_deref(), Some("fork/r"));
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

    #[test]
    fn lists_changed_files() {
        let files = r#"[{"filename":"a.rs","status":"changed","additions":3,"deletions":1},
            {"filename":"b.rs","previous_filename":"old.rs","status":"renamed","additions":0,"deletions":0},
            {"filename":"c.rs","status":"added","additions":9,"deletions":0},
            {"filename":"d.rs","status":"deleted","additions":0,"deletions":4}]"#;
        let (_s, gitea, repo) = setup(vec![(
            "/api/v1/repos/o/r/pulls/7/files?page=1&limit=50",
            Reply::json(files),
        )]);
        let f = gitea.files(&repo, 7).unwrap();
        assert_eq!(f.len(), 4);
        assert_eq!(
            (f[0].status, f[0].additions, f[0].deletions),
            (FileStatus::Modified, 3, 1)
        );
        assert_eq!(
            (f[1].status, f[1].old_path.as_deref()),
            (FileStatus::Renamed, Some("old.rs"))
        );
        assert_eq!((f[2].status, f[2].old_path.clone()), (FileStatus::Added, None));
        assert_eq!(f[3].status, FileStatus::Deleted);
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
        }
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
        gitea.reply(&repo, 7, "11", "hi").unwrap();
        gitea.set_resolved(&repo, "11", true).unwrap();
        gitea.set_resolved(&repo, "11", false).unwrap();
        gitea.edit_comment(&repo, "11", "new", Some((2, 4))).unwrap();
        gitea.delete_comment(&repo, "11").unwrap();
        assert_eq!(gitea.current_user().unwrap(), "me");
        let reqs = server.requests();
        let ms: Vec<&str> = reqs.iter().map(|r| r.method.as_str()).collect();
        assert_eq!(ms, ["POST", "POST", "POST", "PATCH", "DELETE", "GET"]);
        assert_eq!(reqs[0].body, "{\"body\":\"hi\"}");
        assert_eq!(reqs[3].body, "{\"body\":\"new\\n\\n<!-- jed:range=2-4 -->\"}");
        assert!(reqs[1].target.ends_with("/resolve") && reqs[2].target.ends_with("/unresolve"));
        assert!(gitea.reply(&repo, 7, "99", "x").is_err(), "an unknown route is a 404");
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
        let c = gitea.comments(&repo, 7).unwrap();
        let t = &c.threads[0];
        assert_eq!((t.anchor.start_line, t.anchor.end_line), (3, 5));
        assert_eq!(t.comments[0].body, "multi");
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
        let c = gitea.comments(&repo, 7).unwrap();
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
}
