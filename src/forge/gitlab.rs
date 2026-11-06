//! GitLab (gitlab.com and self-managed): REST API v4 only. What the API offers is recorded in
//! `tasks/mr_review.md` ("Forge differences"); the parts that matter here:
//!
//! - A merge request is addressed by its project path (`group/sub/repo`, percent-encoded as one
//!   segment) and its `iid`. `diff_refs.base_sha` is the merge base, `start_sha` the target tip.
//! - A discussion is a thread; `resolved` is derived from its resolvable notes. A diff note's
//!   `position` has `new_line` (added), `old_line` (removed) or both (unchanged), and a
//!   `line_range` for several lines. GitLab has no "outdated" flag: a note made at another head
//!   than the merge request's current one is treated as outdated (shown under CONVERSATIONS).
//! - The write calls need the MR `iid` (and a discussion id), which the `Forge` trait does not
//!   pass to `set_resolved` / `edit_comment` / `delete_comment`; the ids jed keeps carry them:
//!   `Thread::id` is `iid:discussion` and `Comment::id` is `iid:discussion:note` (the discussion
//!   part is empty for a plain MR comment). Discussion ids are checked to be hex before use.
//! - A review is draft notes (one request each, so each can fail alone) published together by
//!   `bulk_publish`; Approve is a separate `approve` request, Request Changes is the publish
//!   call's `reviewer_state`.

use serde::Deserialize;
use serde_json::{Value, json};

use super::http::{Client, clean_line, clean_text, encode_segment};
use super::{
    Anchor, BranchRef, Comment, DraftReview, Forge, ForgeError, MergeRequest, MrComments, MrState, RepoRef, Result,
    ReviewEvent, Side, SubmitOutcome, Thread,
};

pub struct GitLab {
    client: Client,
}

impl GitLab {
    pub fn new(client: Client) -> GitLab {
        GitLab { client }
    }

    /// `/projects/{path}` with the whole path as one encoded segment.
    fn project(repo: &RepoRef) -> String {
        format!("/projects/{}", encode_segment(&repo.path))
    }

    fn mr_path(repo: &RepoRef, number: u64) -> String {
        format!("{}/merge_requests/{number}", Self::project(repo))
    }

    /// The renamed files of a merge request as `(new path, old path)`: GitLab wants both paths in
    /// a position. `…/diffs` (paged); an older GitLab without it answers 404 and `…/changes` is
    /// read instead.
    ///
    /// Blocking: call from spawn_blocking.
    fn renames(&self, repo: &RepoRef, number: u64) -> Result<Vec<(String, String)>> {
        let mr = Self::mr_path(repo, number);
        let diffs: Vec<Diff> = match self.client.get_all(&format!("{mr}/diffs")) {
            Err(ForgeError::Status { status: 404, .. }) => {
                self.client.get_json::<Changes>(&format!("{mr}/changes"))?.changes
            }
            other => other?,
        };
        Ok(diffs
            .into_iter()
            .filter(|d| d.renamed_file)
            .map(|d| (text(&d.new_path), text(&d.old_path)))
            .filter(|(new, old)| !new.is_empty() && !old.is_empty())
            .collect())
    }
}

#[derive(Deserialize, Default)]
struct User {
    username: Option<String>,
}

#[derive(Deserialize, Default)]
struct DiffRefs {
    base_sha: Option<String>,
    start_sha: Option<String>,
    head_sha: Option<String>,
}

#[derive(Deserialize)]
struct Mr {
    iid: u64,
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    author: Option<User>,
    web_url: Option<String>,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    target_branch: String,
    sha: Option<String>,
    diff_refs: Option<DiffRefs>,
}

#[derive(Deserialize)]
struct Diff {
    old_path: Option<String>,
    new_path: Option<String>,
    #[serde(default)]
    renamed_file: bool,
}

#[derive(Deserialize)]
struct Changes {
    #[serde(default)]
    changes: Vec<Diff>,
}

#[derive(Deserialize, Default)]
struct Point {
    #[serde(rename = "type")]
    kind: Option<String>,
    old_line: Option<u32>,
    new_line: Option<u32>,
}

#[derive(Deserialize, Default)]
struct LineRange {
    start: Option<Point>,
    end: Option<Point>,
}

#[derive(Deserialize, Default)]
struct Position {
    head_sha: Option<String>,
    old_path: Option<String>,
    new_path: Option<String>,
    position_type: Option<String>,
    old_line: Option<u32>,
    new_line: Option<u32>,
    line_range: Option<LineRange>,
}

#[derive(Deserialize)]
struct Note {
    id: u64,
    body: Option<String>,
    author: Option<User>,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    system: bool,
    #[serde(default)]
    resolvable: bool,
    #[serde(default)]
    resolved: bool,
    position: Option<Position>,
}

#[derive(Deserialize)]
struct Discussion {
    id: String,
    #[serde(default)]
    individual_note: bool,
    #[serde(default)]
    notes: Vec<Note>,
}

fn text(s: &Option<String>) -> String {
    clean_line(s.as_deref().unwrap_or_default())
}

fn ghost(user: Option<&User>) -> String {
    user.and_then(|u| u.username.as_deref())
        .map(clean_line)
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "ghost".into())
}

fn merge_request(m: Mr) -> MergeRequest {
    let refs = m.diff_refs.unwrap_or_default();
    let clean = |s: &Option<String>| Some(text(s)).filter(|s| !s.is_empty());
    let head_sha = clean(&m.sha).or_else(|| clean(&refs.head_sha)).unwrap_or_default();
    MergeRequest {
        number: m.iid,
        title: clean_line(m.title.as_deref().unwrap_or_default()),
        body: clean_text(m.description.as_deref().unwrap_or_default()),
        state: match m.state.as_str() {
            "merged" => MrState::Merged,
            "closed" | "locked" => MrState::Closed,
            _ => MrState::Open,
        },
        draft: m.draft,
        author: ghost(m.author.as_ref()),
        url: text(&m.web_url),
        base: BranchRef {
            name: clean_line(&m.target_branch),
            sha: clean(&refs.start_sha).unwrap_or_default(),
        },
        head: BranchRef {
            name: clean_line(&m.source_branch),
            sha: head_sha,
        },
        head_repo: None,
        merge_base: clean(&refs.base_sha),
    }
}

/// The side and line of one end of a position: `type` says which file when the line is in both.
fn point(kind: Option<&str>, old: Option<u32>, new: Option<u32>) -> Option<(Side, u32)> {
    let found = match kind {
        Some("old") => old.map(|l| (Side::Left, l)),
        Some("new") => new.map(|l| (Side::Right, l)),
        _ => new.map(|l| (Side::Right, l)).or(old.map(|l| (Side::Left, l))),
    };
    found.filter(|&(_, l)| l >= 1)
}

/// The anchor of a text position, `None` for a file or image comment or a position without a line.
fn anchor_of(p: &Position) -> Option<Anchor> {
    if !matches!(p.position_type.as_deref(), None | Some("text")) {
        return None;
    }
    let single = point(None, p.old_line, p.new_line);
    let (start, end) = match &p.line_range {
        Some(LineRange {
            start: Some(s),
            end: Some(e),
        }) => (
            point(s.kind.as_deref(), s.old_line, s.new_line),
            point(e.kind.as_deref(), e.old_line, e.new_line),
        ),
        _ => (single, single),
    };
    let (side, end_line) = end.or(single)?;
    let start_line = match start {
        Some((s, l)) if s == side => l.min(end_line),
        _ => end_line,
    };
    let (old, new) = (text(&p.old_path), text(&p.new_path));
    let path = match side {
        Side::Left if !old.is_empty() => old,
        _ if !new.is_empty() => new,
        _ => old,
    };
    Some(Anchor {
        path,
        side,
        start_line,
        end_line,
        commit: text(&p.head_sha),
    })
}

fn comment(iid: u64, discussion: &str, n: &Note, mr_url: &str) -> Comment {
    Comment {
        id: format!("{iid}:{discussion}:{}", n.id),
        author: ghost(n.author.as_ref()),
        body: clean_text(n.body.as_deref().unwrap_or_default()),
        created_at: text(&n.created_at),
        updated_at: text(&n.updated_at),
        url: if mr_url.is_empty() {
            String::new()
        } else {
            format!("{mr_url}#note_{}", n.id)
        },
    }
}

/// `iid:discussion` or `iid:discussion:note`, checked: numbers, and a hex discussion id (empty
/// for a plain MR comment). Nothing else is ever put into a URL.
fn split_ids(id: &str) -> Result<(u64, &str, Option<u64>)> {
    let bad = || ForgeError::Parse(format!("not a GitLab id: {}", clean_line(id)));
    let mut parts = id.split(':');
    let iid = parts.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
    let disc = parts.next().ok_or_else(bad)?;
    if !disc.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let note = match parts.next() {
        Some(n) => Some(n.parse().map_err(|_| bad())?),
        None => None,
    };
    if parts.next().is_some() {
        return Err(bad());
    }
    Ok((iid, disc, note))
}

/// The URL of a note: inside its discussion, or the plain MR note.
fn note_path(repo: &RepoRef, comment_id: &str) -> Result<String> {
    let (iid, disc, note) = split_ids(comment_id)?;
    let note = note.ok_or_else(|| ForgeError::Parse(format!("not a GitLab comment id: {}", clean_line(comment_id))))?;
    let mr = GitLab::mr_path(repo, iid);
    Ok(if disc.is_empty() {
        format!("{mr}/notes/{note}")
    } else {
        format!("{mr}/discussions/{disc}/notes/{note}")
    })
}

/// SHA-1 as lower-case hex (GitLab's `line_code` is the file name's hash plus the line numbers).
fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend((data.len() as u64 * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// One end of a `line_range`; the other file's line number in the `line_code` is 0.
fn range_point(path: &str, side: Side, line: u32) -> Value {
    let (kind, old, new) = match side {
        Side::Left => ("old", line, 0),
        Side::Right => ("new", 0, line),
    };
    let mut v = json!({ "line_code": format!("{}_{old}_{new}", sha1_hex(path.as_bytes())), "type": kind });
    v[if side == Side::Left { "old_line" } else { "new_line" }] = json!(line);
    v
}

/// The (old, new) path of the anchor's file: they differ on a renamed file, and GitLab wants both.
fn paths_of<'a>(a: &'a Anchor, renames: &'a [(String, String)]) -> (&'a str, &'a str) {
    if let Some((new, old)) = renames.iter().find(|(new, _)| *new == a.path) {
        (old, new)
    } else if let Some((new, _)) = renames.iter().find(|(_, old)| *old == a.path) {
        (&a.path, new)
    } else {
        (&a.path, &a.path)
    }
}

fn position(a: &Anchor, refs: &DiffRefs, head_sha: &str, renames: &[(String, String)]) -> Value {
    let (old_path, new_path) = paths_of(a, renames);
    let mut p = json!({
        "position_type": "text",
        "base_sha": text(&refs.base_sha),
        "start_sha": text(&refs.start_sha),
        "head_sha": head_sha,
        "old_path": old_path,
        "new_path": new_path,
    });
    p[if a.side == Side::Left { "old_line" } else { "new_line" }] = json!(a.end_line);
    if a.start_line < a.end_line {
        p["line_range"] = json!({
            "start": range_point(new_path, a.side, a.start_line),
            "end": range_point(new_path, a.side, a.end_line),
        });
    }
    p
}

/// A plain `suggestion` fence in a multi-line comment becomes `suggestion:-N+0`, which is how
/// GitLab's page knows the block replaces the whole range (jed itself reads both the same).
fn gitlab_body(body: &str, a: &Anchor) -> String {
    let span = a.end_line - a.start_line;
    if span == 0 || span > crate::suggestion::MAX_SPAN {
        return body.to_string();
    }
    let mut lines: Vec<String> = body.split('\n').map(str::to_string).collect();
    for f in crate::suggestion::fences(body) {
        if f.span.is_none()
            && let Some(line) = lines.get_mut(f.open)
            && let Some(at) = line.to_ascii_lowercase().find("suggestion")
        {
            line.insert_str(at + "suggestion".len(), &format!(":-{span}+0"));
        }
    }
    lines.join("\n")
}

impl Forge for GitLab {
    /// Blocking: call from spawn_blocking.
    fn find_open(&self, repo: &RepoRef, head_branch: &str) -> Result<Vec<MergeRequest>> {
        let url = format!(
            "{}/merge_requests?state=opened&source_branch={}",
            Self::project(repo),
            encode_segment(head_branch)
        );
        let mrs: Vec<Mr> = self.client.get_all(&url)?;
        Ok(mrs
            .into_iter()
            .map(merge_request)
            .filter(|m| m.head.name == head_branch)
            .collect())
    }

    /// Blocking: call from spawn_blocking.
    fn merge_request(&self, repo: &RepoRef, number: u64) -> Result<MergeRequest> {
        Ok(merge_request(self.client.get_json(&Self::mr_path(repo, number))?))
    }

    /// Discussions: positioned ones are threads (outdated when made at another head than the
    /// current one, or not on a text line); the rest are conversation, system notes skipped.
    ///
    /// Blocking: call from spawn_blocking.
    fn comments(&self, repo: &RepoRef, number: u64) -> Result<MrComments> {
        let current = self.merge_request(repo, number)?;
        let discussions: Vec<Discussion> = self
            .client
            .get_all(&format!("{}/discussions", Self::mr_path(repo, number)))?;
        let mut out = MrComments::default();
        for d in discussions {
            let id = clean_line(&d.id);
            if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let notes: Vec<&Note> = d.notes.iter().filter(|n| !n.system).collect();
            let Some(first) = notes.first() else { continue };
            let disc = if d.individual_note { "" } else { id.as_str() };
            let make = |n: &&Note| comment(number, disc, n, &current.url);
            match &first.position {
                Some(p) => {
                    let anchor = anchor_of(p);
                    let head = text(&p.head_sha);
                    let outdated = anchor.is_none() || (!head.is_empty() && head != current.head.sha);
                    let resolvable: Vec<&&Note> = notes.iter().filter(|n| n.resolvable).collect();
                    out.threads.push(Thread {
                        id: format!("{number}:{id}"),
                        anchor: anchor.unwrap_or_else(|| Anchor {
                            path: text(&p.new_path),
                            side: Side::Right,
                            start_line: 1,
                            end_line: 1,
                            commit: head,
                        }),
                        resolved: !resolvable.is_empty() && resolvable.iter().all(|n| n.resolved),
                        outdated,
                        comments: notes.iter().map(make).collect(),
                    });
                }
                None => out.conversation.extend(notes.iter().map(make)),
            }
        }
        Ok(out)
    }

    /// Blocking: call from spawn_blocking.
    fn current_user(&self) -> Result<String> {
        let user: User = self.client.get_json("/user")?;
        let name = text(&user.username);
        if name.is_empty() {
            return Err(ForgeError::Parse("no username in the answer".into()));
        }
        Ok(name)
    }

    /// Each comment is a draft note (so one bad line fails alone), then the drafts are published
    /// together (a failed publish deletes them again, so a retry doesn't double them), then
    /// Approve posts `approve` at the reviewed commit. A merge request that has moved on since
    /// `head_sha` refuses the whole submit. Drafts the user has made on GitLab's own page are
    /// published too: they are the user's pending review.
    ///
    /// Blocking: call from spawn_blocking.
    fn submit_review(&self, repo: &RepoRef, number: u64, head_sha: &str, review: &DraftReview) -> SubmitOutcome {
        let count = review.comments.len();
        let mr_path = Self::mr_path(repo, number);
        let fail = |e: ForgeError| SubmitOutcome::atomic::<()>(count, Err(e));
        let mr: Mr = match self.client.get_json(&mr_path) {
            Ok(m) => m,
            Err(e) => return fail(e),
        };
        let refs = mr.diff_refs.unwrap_or_default();
        let current = mr.sha.clone().or_else(|| refs.head_sha.clone()).unwrap_or_default();
        if !head_sha.is_empty() && !current.is_empty() && current != head_sha {
            return fail(ForgeError::Config(
                "the merge request has new commits; refresh and submit again".into(),
            ));
        }
        let at = if head_sha.is_empty() {
            current.as_str()
        } else {
            head_sha
        };
        // Publishing is all-or-nothing for the user's drafts, so drafts made on GitLab's own page
        // would be posted with ours: refuse and say so.
        if !review.comments.is_empty() || review.event == ReviewEvent::RequestChanges {
            match self.client.get_all::<Value>(&format!("{mr_path}/draft_notes")) {
                Ok(pending) if !pending.is_empty() => {
                    return fail(ForgeError::Config(format!(
                        "{} draft note(s) are already pending on this merge request in GitLab; \
                         publish or delete them there first (submitting would publish them too)",
                        pending.len()
                    )));
                }
                Ok(_) => {}
                Err(e) => return fail(e),
            }
        }
        // Only needed for renamed files; a failure just means both paths are the anchor's.
        let renames = if review.comments.is_empty() {
            Vec::new()
        } else {
            self.renames(repo, number).unwrap_or_default()
        };
        let mut out = SubmitOutcome::default();
        let mut drafts: Vec<(usize, Option<u64>)> = Vec::new();
        for (i, c) in review.comments.iter().enumerate() {
            let body = json!({
                "note": gitlab_body(&c.body, &c.anchor),
                "position": position(&c.anchor, &refs, at, &renames),
            });
            match self.client.post(&format!("{mr_path}/draft_notes"), &body.to_string()) {
                Ok(r) => {
                    let id = serde_json::from_str::<Value>(&r.body)
                        .ok()
                        .and_then(|v| v.get("id").and_then(Value::as_u64));
                    drafts.push((i, id));
                }
                Err(e) => out.failed.push((i, e.to_string())),
            }
        }
        if !drafts.is_empty() || review.event == ReviewEvent::RequestChanges {
            let mut body = json!({});
            if !review.body.trim().is_empty() {
                body["note"] = json!(review.body);
            }
            if review.event == ReviewEvent::RequestChanges {
                body["reviewer_state"] = json!("requested_changes");
            }
            if let Err(e) = self
                .client
                .post(&format!("{mr_path}/draft_notes/bulk_publish"), &body.to_string())
            {
                let left = drafts
                    .iter()
                    .filter_map(|(_, id)| *id)
                    .filter(|id| self.client.delete(&format!("{mr_path}/draft_notes/{id}")).is_err())
                    .count();
                out.failed.extend(drafts.iter().map(|(i, _)| (*i, e.to_string())));
                out.failed.sort_by_key(|f| f.0);
                out.error = Some(if left > 0 {
                    format!("{e} ({left} draft note(s) could not be removed; delete them on GitLab before retrying)")
                } else {
                    e.to_string()
                });
                return out;
            }
        }
        out.posted = drafts.iter().map(|(i, _)| *i).collect();
        out.failed.sort_by_key(|f| f.0);
        if review.event == ReviewEvent::Approve
            && let Err(e) = self
                .client
                .post(&format!("{mr_path}/approve"), &json!({ "sha": at }).to_string())
        {
            out.error = Some(format!("the comments were posted but approving failed: {e}"));
        }
        out
    }

    /// Blocking: call from spawn_blocking.
    fn reply(&self, repo: &RepoRef, _number: u64, thread_id: &str, body: &str) -> Result<()> {
        let (iid, disc, _) = split_ids(thread_id)?;
        if disc.is_empty() {
            return Err(ForgeError::Parse("not a GitLab thread id".into()));
        }
        let url = format!("{}/discussions/{disc}/notes", Self::mr_path(repo, iid));
        self.client.post(&url, &json!({ "body": body }).to_string()).map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn set_resolved(&self, repo: &RepoRef, thread_id: &str, resolved: bool) -> Result<()> {
        let (iid, disc, _) = split_ids(thread_id)?;
        if disc.is_empty() {
            return Err(ForgeError::Parse("not a GitLab thread id".into()));
        }
        let url = format!("{}/discussions/{disc}", Self::mr_path(repo, iid));
        self.client
            .put(&url, &json!({ "resolved": resolved }).to_string())
            .map(|_| ())
    }

    /// GitLab keeps the range natively, so `range` is not used.
    ///
    /// Blocking: call from spawn_blocking.
    fn edit_comment(&self, repo: &RepoRef, comment_id: &str, body: &str, _range: Option<(u32, u32)>) -> Result<()> {
        let url = note_path(repo, comment_id)?;
        self.client.put(&url, &json!({ "body": body }).to_string()).map(|_| ())
    }

    /// Blocking: call from spawn_blocking.
    fn delete_comment(&self, repo: &RepoRef, comment_id: &str) -> Result<()> {
        self.client.delete(&note_path(repo, comment_id)?).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Kind;
    use super::super::config::Resolved;
    use super::super::fake::{FakeServer, Reply};
    use super::*;
    use crate::forge::DraftComment;

    const MR: &str = r#"{"iid":7,"title":"Add \u001b[2Jthing","description":null,"state":"opened","draft":true,
        "author":{"username":"ann"},"web_url":"https://g/o/r/-/merge_requests/7",
        "source_branch":"feat","target_branch":"main","sha":"h0",
        "diff_refs":{"base_sha":"mb1","start_sha":"t0","head_sha":"h0"}}"#;
    const P: &str = "/api/v4/projects/grp%2Fsub%2Frepo";
    const NO_DRAFTS: &str = "/merge_requests/7/draft_notes?per_page=100&page=1";

    fn setup(routes: Vec<(&str, Reply)>) -> (FakeServer, GitLab, RepoRef) {
        let server = FakeServer::start(routes.into_iter().map(|(p, r)| (p.to_string(), r)).collect());
        let resolved = Resolved {
            host: "fake".into(),
            kind: Kind::GitLab,
            api_url: format!("{}/api/v4", server.url()),
            token_env: "T".into(),
            allow_http: true,
        };
        let client = Client::new(&resolved, |_| Some("tok".into())).unwrap();
        (
            server,
            GitLab::new(client),
            RepoRef {
                host: "fake".into(),
                path: "grp/sub/repo".into(),
            },
        )
    }

    fn route(suffix: &str) -> String {
        format!("{P}{suffix}")
    }

    #[test]
    fn sha1_matches_the_known_vectors() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let long = [b'a'; 1000];
        assert_eq!(sha1_hex(&long), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
        // Vectors from `sha1sum`: padding boundaries (55, 56, 64, 119, 128 bytes) and non-ASCII.
        let b = |n: usize| "b".repeat(n);
        for (n, want) in [
            (55, "4d055f5334ac4bca50260deff4707cd8d4fc1454"),
            (56, "049adea016c8a8e3b7a9054aeeaa8643453bebd9"),
            (64, "9d682ff9a7018603023176b8c12926c6a15510ee"),
            (119, "725c2b5274ff30f1f4c30ba10735e65d3cc7c58b"),
            (128, "6d2f0f9b0a20e4f5be2cbed5cf3fd59b97c4bc4e"),
        ] {
            assert_eq!(sha1_hex(b(n).as_bytes()), want, "{n} bytes");
        }
        assert_eq!(
            sha1_hex(b"The quick brown fox jumps over the lazy dog"),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
        assert_eq!(
            sha1_hex("src/é日本.rs".as_bytes()),
            "ef837b3ff61e44f1554338c554ef6117ca9674c5"
        );
    }

    #[test]
    fn pending_drafts_made_on_gitlab_refuse_the_submit_and_renames_send_both_paths() {
        let (server, gl, repo) = setup(vec![
            (&route("/merge_requests/7"), Reply::json(MR)),
            (
                &route(NO_DRAFTS),
                Reply::json(r#"[{"id":99,"note":"mine, on the web page"}]"#),
            ),
        ]);
        let out = gl.submit_review(&repo, 7, "h0", &review(ReviewEvent::Comment, &[(4, 4)]));
        assert!(out.posted.is_empty() && out.error.as_deref().is_some_and(|e| e.contains("already pending")));
        assert!(
            server.requests().iter().all(|r| r.method == "GET"),
            "nothing was posted"
        );

        let (server, gl, repo) = setup(vec![
            (&route("/merge_requests/7"), Reply::json(MR)),
            (&route(NO_DRAFTS), Reply::json("[]")),
            (
                &route("/merge_requests/7/diffs?per_page=100&page=1"),
                Reply::json(r#"[{"old_path":"old.rs","new_path":"a.rs","renamed_file":true,"diff":""}]"#),
            ),
            (
                &format!("POST {}", route("/merge_requests/7/draft_notes")),
                Reply::json(r#"{"id":11}"#),
            ),
            (
                &format!("POST {}", route("/merge_requests/7/draft_notes/bulk_publish")),
                Reply::status(204),
            ),
        ]);
        let out = gl.submit_review(&repo, 7, "h0", &review(ReviewEvent::Comment, &[(4, 6)]));
        assert_eq!(out.posted, vec![0]);
        let reqs = server.requests();
        let draft = reqs
            .iter()
            .find(|r| r.target.ends_with("/draft_notes") && r.method == "POST")
            .unwrap();
        let v: Value = serde_json::from_str(&draft.body).unwrap();
        assert_eq!(
            (v["position"]["old_path"].as_str(), v["position"]["new_path"].as_str()),
            (Some("old.rs"), Some("a.rs"))
        );
        // `line_code` hashes the new path.
        assert_eq!(
            v["position"]["line_range"]["end"]["line_code"],
            format!("{}_0_6", sha1_hex(b"a.rs"))
        );
    }

    #[test]
    fn finds_open_requests_by_source_branch_with_the_private_token() {
        let (server, gl, repo) = setup(vec![(
            &route("/merge_requests?state=opened&source_branch=feat&per_page=100&page=1"),
            Reply::json(&format!("[{MR}]")),
        )]);
        let found = gl.find_open(&repo, "feat").unwrap();
        assert_eq!(found.len(), 1);
        let m = &found[0];
        assert_eq!(
            (m.number, m.state, m.author.as_str(), m.draft),
            (7, MrState::Open, "ann", true)
        );
        assert_eq!(m.title, "Add [2Jthing");
        assert_eq!(m.body, "");
        let r = &server.requests()[0];
        assert_eq!(r.header("private-token"), Some("tok"));
        assert_eq!(r.header("authorization"), None);
    }

    #[test]
    fn merge_request_reads_the_diff_refs() {
        let (_s, gl, repo) = setup(vec![(&route("/merge_requests/7"), Reply::json(MR))]);
        let m = gl.merge_request(&repo, 7).unwrap();
        assert_eq!(m.merge_base.as_deref(), Some("mb1"));
        assert_eq!((m.base.sha.as_str(), m.head.sha.as_str()), ("t0", "h0"));
        assert_eq!((m.base.name.as_str(), m.head.name.as_str()), ("main", "feat"));
        let merged = MR.replace("\"opened\"", "\"merged\"");
        let (_s, gl, repo) = setup(vec![(&route("/merge_requests/7"), Reply::json(&merged))]);
        assert_eq!(gl.merge_request(&repo, 7).unwrap().state, MrState::Merged);
    }

    #[test]
    fn renames_across_pages() {
        let (server, gl, repo) = setup(vec![
            (
                &route("/merge_requests/7/diffs?per_page=100&page=1"),
                Reply::json(
                    r#"[{"old_path":"a.rs","new_path":"a.rs","diff":"@@ -1 +1,2 @@\n-x\n+y\n"},
                    {"old_path":"old.rs","new_path":"b.rs","renamed_file":true,"diff":""}]"#,
                )
                .with_header("X-Next-Page", "2"),
            ),
            (
                &route("/merge_requests/7/diffs?per_page=100&page=2"),
                Reply::json(r#"[{"old_path":"c0.rs","new_path":"c.rs","renamed_file":true}]"#)
                    .with_header("X-Next-Page", ""),
            ),
        ]);
        let got = gl.renames(&repo, 7).unwrap();
        assert_eq!(
            got,
            [
                ("b.rs".to_string(), "old.rs".to_string()),
                ("c.rs".into(), "c0.rs".into())
            ]
        );
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn renames_fall_back_to_changes_on_an_older_gitlab() {
        let (_s, gl, repo) = setup(vec![(
            &route("/merge_requests/7/changes"),
            Reply::json(r#"{"changes":[{"old_path":"o","new_path":"a","renamed_file":true}]}"#),
        )]);
        assert_eq!(gl.renames(&repo, 7).unwrap(), [("a".to_string(), "o".to_string())]);
    }

    fn diff_note(id: u64, head: &str, pos: &str, resolved: bool) -> String {
        format!(
            r#"{{"id":{id},"body":"note {id}","author":{{"username":"bob"}},"created_at":"t1","updated_at":"t2",
            "system":false,"resolvable":true,"resolved":{resolved},"position":{pos}}}"#
        )
        .replace("HEAD", head)
    }

    const MULTI: &str = r#"{"head_sha":"HEAD","old_path":"a.rs","new_path":"a.rs","position_type":"text",
        "new_line":9,"old_line":null,"line_range":{"start":{"line_code":"x_0_5","type":"new","old_line":null,"new_line":5},
        "end":{"line_code":"x_0_9","type":"new","old_line":null,"new_line":9}}}"#;
    const OLD_ONLY: &str = r#"{"head_sha":"HEAD","old_path":"a.rs","new_path":"a.rs","position_type":"text","old_line":3,"new_line":null}"#;
    const IMAGE: &str = r#"{"head_sha":"HEAD","old_path":"i.png","new_path":"i.png","position_type":"image"}"#;

    #[test]
    fn discussions_become_threads_and_conversation() {
        let json = format!(
            r#"[{{"id":"aa11","individual_note":false,"notes":[{},{}]}},
            {{"id":"bb22","individual_note":false,"notes":[{}]}},
            {{"id":"cc33","individual_note":false,"notes":[{}]}},
            {{"id":"dd44","individual_note":false,"notes":[{}]}},
            {{"id":"ee55","individual_note":true,"notes":[{{"id":50,"body":"hello\u001b[2J","author":{{"username":"cy"}},"created_at":"t","updated_at":"t","system":false}}]}},
            {{"id":"ff66","individual_note":true,"notes":[{{"id":51,"body":"merged","author":{{"username":"x"}},"system":true}}]}}]"#,
            diff_note(1, "h0", MULTI, true),
            diff_note(2, "h0", MULTI, true),
            diff_note(3, "h0", OLD_ONLY, false),
            diff_note(4, "h0", IMAGE, false),
            diff_note(5, "oldhead", MULTI, false),
        );
        let (_s, gl, repo) = setup(vec![
            (&route("/merge_requests/7"), Reply::json(MR)),
            (
                &route("/merge_requests/7/discussions?per_page=100&page=1"),
                Reply::json(&json),
            ),
        ]);
        let c = gl.comments(&repo, 7).unwrap();
        assert_eq!(c.threads.len(), 4);
        let t = &c.threads[0];
        assert_eq!(t.id, "7:aa11");
        assert_eq!(
            (t.anchor.start_line, t.anchor.end_line, t.anchor.side),
            (5, 9, Side::Right)
        );
        assert_eq!(t.anchor.commit, "h0");
        assert!(t.resolved && !t.outdated);
        assert_eq!(t.comments.len(), 2);
        assert_eq!(t.comments[1].id, "7:aa11:2");
        assert_eq!(t.comments[0].url, "https://g/o/r/-/merge_requests/7#note_1");
        let old = &c.threads[1];
        assert_eq!(
            (old.anchor.side, old.anchor.start_line, old.anchor.end_line),
            (Side::Left, 3, 3)
        );
        assert!(!old.resolved && !old.outdated);
        let image = &c.threads[2];
        assert!(image.outdated);
        assert_eq!(image.anchor.path, "i.png");
        assert!(c.threads[3].outdated, "made at an older head");
        assert_eq!(c.conversation.len(), 1, "system notes are skipped");
        assert_eq!(c.conversation[0].body, "hello[2J");
        assert_eq!(c.conversation[0].id, "7::50");
    }

    #[test]
    fn discussions_page_by_x_next_page() {
        let (server, gl, repo) = setup(vec![
            (&route("/merge_requests/7"), Reply::json(MR)),
            (
                &route("/merge_requests/7/discussions?per_page=100&page=1"),
                Reply::json(r#"[{"id":"aa","individual_note":true,"notes":[{"id":1,"body":"one"}]}]"#)
                    .with_header("X-Next-Page", "2"),
            ),
            (
                &route("/merge_requests/7/discussions?per_page=100&page=2"),
                Reply::json(r#"[{"id":"bb","individual_note":true,"notes":[{"id":2,"body":"two"}]}]"#)
                    .with_header("X-Next-Page", ""),
            ),
        ]);
        assert_eq!(gl.comments(&repo, 7).unwrap().conversation.len(), 2);
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        let (_s, gl, repo) = setup(vec![(&route("/merge_requests/7"), Reply::json("{nope"))]);
        assert!(matches!(gl.merge_request(&repo, 7), Err(ForgeError::Parse(_))));
    }

    fn anchor(start: u32, end: u32, side: Side) -> Anchor {
        Anchor {
            path: "a.rs".into(),
            side,
            start_line: start,
            end_line: end,
            commit: "h0".into(),
        }
    }

    fn review(event: ReviewEvent, anchors: &[(u32, u32)]) -> DraftReview {
        DraftReview {
            body: String::new(),
            event,
            comments: anchors
                .iter()
                .map(|&(s, e)| DraftComment {
                    anchor: anchor(s, e, Side::Right),
                    body: format!("c{s}"),
                })
                .collect(),
        }
    }

    #[test]
    fn submit_posts_drafts_publishes_and_approves() {
        let (server, gl, repo) = setup(vec![
            (&route("/merge_requests/7"), Reply::json(MR)),
            (&route(NO_DRAFTS), Reply::json("[]")),
            (
                &format!("POST {}", route("/merge_requests/7/draft_notes")),
                Reply::json(r#"{"id":11}"#),
            ),
            (
                &format!("POST {}", route("/merge_requests/7/draft_notes/bulk_publish")),
                Reply::status(204),
            ),
            (
                &format!("POST {}", route("/merge_requests/7/approve")),
                Reply::json("{}"),
            ),
        ]);
        let out = gl.submit_review(&repo, 7, "h0", &review(ReviewEvent::Approve, &[(4, 4), (5, 9)]));
        assert_eq!(out.posted, vec![0, 1]);
        assert!(out.failed.is_empty() && out.error.is_none());
        let reqs = server.requests();
        let drafts: Vec<Value> = reqs
            .iter()
            .filter(|r| r.target.ends_with("/draft_notes"))
            .map(|r| serde_json::from_str(&r.body).unwrap())
            .collect();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0]["note"], "c4");
        let p = &drafts[0]["position"];
        assert_eq!(p["position_type"], "text");
        assert_eq!(
            (p["base_sha"].as_str(), p["start_sha"].as_str(), p["head_sha"].as_str()),
            (Some("mb1"), Some("t0"), Some("h0"))
        );
        assert_eq!(
            (
                p["new_line"].as_u64(),
                p["old_line"].is_null(),
                p["line_range"].is_null()
            ),
            (Some(4), true, true)
        );
        let range = &drafts[1]["position"]["line_range"];
        assert_eq!(range["start"]["new_line"], 5);
        assert_eq!(range["end"]["type"], "new");
        assert_eq!(range["end"]["line_code"], format!("{}_0_9", sha1_hex(b"a.rs")));
        let approve = reqs.iter().find(|r| r.target.ends_with("/approve")).unwrap();
        assert_eq!(approve.body, r#"{"sha":"h0"}"#);
        assert_eq!(approve.header("private-token"), Some("tok"));
        let publish = reqs.iter().find(|r| r.target.ends_with("/bulk_publish")).unwrap();
        assert_eq!(publish.body, "{}");
    }

    #[test]
    fn request_changes_sets_the_reviewer_state() {
        let (server, gl, repo) = setup(vec![
            (&route("/merge_requests/7"), Reply::json(MR)),
            (&route(NO_DRAFTS), Reply::json("[]")),
            (
                &format!("POST {}", route("/merge_requests/7/draft_notes")),
                Reply::json(r#"{"id":11}"#),
            ),
            (
                &format!("POST {}", route("/merge_requests/7/draft_notes/bulk_publish")),
                Reply::status(204),
            ),
        ]);
        let mut r = review(ReviewEvent::RequestChanges, &[(4, 4)]);
        r.body = "please fix".into();
        let out = gl.submit_review(&repo, 7, "h0", &r);
        assert_eq!(out.posted, vec![0]);
        let publish = server
            .requests()
            .into_iter()
            .find(|r| r.target.ends_with("/bulk_publish"))
            .unwrap();
        let v: Value = serde_json::from_str(&publish.body).unwrap();
        assert_eq!(
            (v["note"].as_str(), v["reviewer_state"].as_str()),
            (Some("please fix"), Some("requested_changes"))
        );
    }

    #[test]
    fn a_failing_draft_fails_alone_and_a_failing_publish_deletes_the_drafts() {
        let (server, gl, repo) = setup(vec![
            (&route("/merge_requests/7"), Reply::json(MR)),
            (&route(NO_DRAFTS), Reply::json("[]")),
            (
                &format!("POST {} ~c4", route("/merge_requests/7/draft_notes")),
                Reply::status(400).with_body(r#"{"message":"bad line"}"#),
            ),
            (
                &format!("POST {} ~c6", route("/merge_requests/7/draft_notes")),
                Reply::json(r#"{"id":12}"#),
            ),
            (
                &format!("POST {}", route("/merge_requests/7/draft_notes/bulk_publish")),
                Reply::status(500),
            ),
            (
                &format!("DELETE {}", route("/merge_requests/7/draft_notes/12")),
                Reply::status(204),
            ),
        ]);
        let out = gl.submit_review(&repo, 7, "h0", &review(ReviewEvent::Comment, &[(4, 4), (6, 6)]));
        assert!(out.posted.is_empty());
        assert_eq!(out.failed.len(), 2);
        assert!(out.failed[0].1.contains("bad line"));
        assert!(out.error.is_some());
        assert!(
            server
                .requests()
                .iter()
                .any(|r| r.method == "DELETE" && r.target.ends_with("/draft_notes/12"))
        );
    }

    #[test]
    fn a_moved_merge_request_refuses_the_submit() {
        let (server, gl, repo) = setup(vec![(&route("/merge_requests/7"), Reply::json(MR))]);
        let out = gl.submit_review(&repo, 7, "old", &review(ReviewEvent::Comment, &[(4, 4)]));
        assert!(out.posted.is_empty() && out.error.as_deref().is_some_and(|e| e.contains("new commits")));
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn plain_suggestion_fences_get_a_range_in_multi_line_comments() {
        let a = anchor(5, 9, Side::Right);
        let body = "fix\n```suggestion\nx\n```\n```rust\ny\n```";
        assert_eq!(
            gitlab_body(body, &a),
            "fix\n```suggestion:-4+0\nx\n```\n```rust\ny\n```"
        );
        assert_eq!(gitlab_body(body, &anchor(5, 5, Side::Right)), body);
        let already = "```suggestion:-1+1\nx\n```";
        assert_eq!(gitlab_body(already, &a), already);
    }

    #[test]
    fn reply_resolve_edit_delete_requests() {
        let (server, gl, repo) = setup(vec![
            (
                &format!("POST {}", route("/merge_requests/7/discussions/aa11/notes")),
                Reply::status(201).with_body("{}"),
            ),
            (
                &format!("PUT {}", route("/merge_requests/7/discussions/aa11")),
                Reply::json("{}"),
            ),
            (
                &format!("PUT {}", route("/merge_requests/7/discussions/aa11/notes/2")),
                Reply::json("{}"),
            ),
            (
                &format!("DELETE {}", route("/merge_requests/7/discussions/aa11/notes/2")),
                Reply::status(204),
            ),
            (
                &format!("PUT {}", route("/merge_requests/7/notes/50")),
                Reply::json("{}"),
            ),
            (
                &format!("DELETE {}", route("/merge_requests/7/notes/50")),
                Reply::status(204),
            ),
        ]);
        gl.reply(&repo, 7, "7:aa11", "thanks").unwrap();
        gl.set_resolved(&repo, "7:aa11", true).unwrap();
        gl.edit_comment(&repo, "7:aa11:2", "new", None).unwrap();
        gl.delete_comment(&repo, "7:aa11:2").unwrap();
        gl.edit_comment(&repo, "7::50", "plain", None).unwrap();
        gl.delete_comment(&repo, "7::50").unwrap();
        let bodies: Vec<_> = server
            .requests()
            .iter()
            .map(|r| (r.method.clone(), r.body.clone()))
            .collect();
        assert_eq!(bodies[0], ("POST".to_string(), r#"{"body":"thanks"}"#.to_string()));
        assert_eq!(bodies[1], ("PUT".to_string(), r#"{"resolved":true}"#.to_string()));
        assert_eq!(bodies[2].1, r#"{"body":"new"}"#);
    }

    #[test]
    fn ids_are_validated_before_they_reach_a_url() {
        let (server, gl, repo) = setup(vec![]);
        for bad in ["7:../x:1", "x:aa:1", "7:aa:1:2", "7:zz:1", "7:aa:q", "7"] {
            assert!(gl.edit_comment(&repo, bad, "b", None).is_err(), "{bad}");
        }
        assert!(gl.set_resolved(&repo, "7:a/b", true).is_err());
        assert!(gl.reply(&repo, 7, "7:", "x").is_err());
        assert!(server.requests().is_empty());
    }

    #[test]
    fn the_token_never_shows_in_errors() {
        let (_s, gl, repo) = setup(vec![(
            &route("/merge_requests/7"),
            Reply::status(401).with_body(r#"{"message":"bad token tok"}"#),
        )]);
        let e = gl.merge_request(&repo, 7).unwrap_err().to_string();
        assert!(!e.contains("tok") || e.contains("<token>"));
        assert!(!e.replace("<token>", "").contains("tok"), "{e}");
    }

    #[test]
    fn current_user_is_the_username() {
        let (_s, gl, _r) = setup(vec![("/api/v4/user", Reply::json(r#"{"username":"alice","id":1}"#))]);
        assert_eq!(gl.current_user().unwrap(), "alice");
    }
}
