//! The forge layer: merge requests (pull requests) and their review threads on Gitea, GitHub and
//! GitLab, behind one `Forge` trait and shared types. No app state and no drawing: `app/` drives
//! it from `spawn_blocking`, `ui/` draws what comes back.
//!
//! - `url`: a git remote URL to a `RepoRef` (host and project path).
//! - `config`: `~/.config/jed/forges.toml` (hosts, kinds, API URLs, token variables); the only
//!   place a host or token variable can come from.
//! - `http`: the blocking HTTP client (`ureq` + rustls + the platform certificate verifier).
//! - `gitea`: the Gitea adapter.
//! - `github`: the GitHub adapter (REST, and GraphQL for threads, replies and resolving).
//! - `gitlab`: the GitLab adapter (REST v4; draft notes for reviews).
//! - `fake` (tests): a local HTTP server serving canned JSON.
//!
//! Every function that talks to a server is `Blocking: call from spawn_blocking`. All text that
//! comes from a server is run through `http::clean_line` / `http::clean_text` before it reaches
//! these types, so nothing downstream can draw a control sequence from a remote.

pub mod config;
pub mod gitea;
pub mod github;
pub mod gitlab;
pub mod http;
pub mod url;

#[cfg(test)]
pub mod fake;

use std::fmt;

/// Which API a host speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Gitea,
    GitHub,
    GitLab,
}

impl Kind {
    pub fn from_name(s: &str) -> Option<Kind> {
        match s.to_ascii_lowercase().as_str() {
            "gitea" | "forgejo" => Some(Kind::Gitea),
            "github" => Some(Kind::GitHub),
            "gitlab" => Some(Kind::GitLab),
            _ => None,
        }
    }

    /// The environment variable holding the token when the entry names none.
    pub fn default_token_env(self) -> &'static str {
        match self {
            Kind::Gitea => "GITEA_TOKEN",
            Kind::GitHub => "GITHUB_TOKEN",
            Kind::GitLab => "GITLAB_TOKEN",
        }
    }
}

/// A repository on a forge: the host (with a port only for an `http(s)://` remote) and the
/// project path (`owner/repo`; GitLab paths may have more segments).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepoRef {
    pub host: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MrState {
    Open,
    Closed,
    Merged,
}

/// A branch of a merge request: its name and the commit it points at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchRef {
    pub name: String,
    pub sha: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeRequest {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: MrState,
    pub draft: bool,
    pub author: String,
    pub url: String,
    pub base: BranchRef,
    pub head: BranchRef,
    /// `owner/repo` the head branch lives in (a fork's, for a fork MR).
    pub head_repo: Option<String>,
    /// The commit the diff is taken against; `None` when the forge doesn't say (the caller then
    /// computes it from the local clone).
    pub merge_base: Option<String>,
}

/// Which side of the diff a line number refers to. Comments on deleted lines are on the left
/// (the old file); everything else, including unchanged lines, is on the right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

/// Where a comment sits: lines `start_line..=end_line` (1-based, `start == end` for one line) of
/// `path` on `side`, as of `commit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    pub side: Side,
    pub start_line: u32,
    pub end_line: u32,
    pub commit: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    /// The forge's id as text (a number on Gitea and GitLab, a node id on GitHub).
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
}

/// A conversation on a line: its comments in order. `resolved` and `outdated` are the forge's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    /// The id the forge's resolve call takes (Gitea: the first comment's id).
    pub id: String,
    pub anchor: Anchor,
    pub resolved: bool,
    /// The anchored code has since changed or gone: `anchor` then refers to `anchor.commit`.
    pub outdated: bool,
    pub comments: Vec<Comment>,
}

/// Everyone's comments on a merge request: line threads, and the conversation under the title
/// (plain comments and review summaries).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MrComments {
    pub threads: Vec<Thread>,
    pub conversation: Vec<Comment>,
}

/// A suggested replacement (`crate::suggestion::parse`): lines `start_line..=end_line` (1-based,
/// the right side's, as of the comment's commit) become `replacement` (every line ended by `\n`;
/// empty: the lines go).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub start_line: u32,
    pub end_line: u32,
    pub replacement: String,
}

/// One comment of a draft review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftComment {
    pub anchor: Anchor,
    pub body: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewEvent {
    Comment,
    Approve,
    RequestChanges,
}

/// The comments a user has written locally, submitted together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftReview {
    pub body: String,
    pub event: ReviewEvent,
    pub comments: Vec<DraftComment>,
}

/// What a submit did, per comment (indices into `DraftReview::comments`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmitOutcome {
    pub posted: Vec<usize>,
    pub failed: Vec<(usize, String)>,
    /// The review itself failed (set whenever nothing was posted because of one error).
    pub error: Option<String>,
}

impl SubmitOutcome {
    /// The outcome of one request carrying all `count` comments: all posted, or all failed with
    /// the request's error.
    pub fn atomic<T>(count: usize, result: Result<T>) -> SubmitOutcome {
        match result {
            Ok(_) => SubmitOutcome {
                posted: (0..count).collect(),
                failed: Vec::new(),
                error: None,
            },
            Err(e) => SubmitOutcome {
                posted: Vec::new(),
                failed: (0..count).map(|i| (i, e.to_string())).collect(),
                error: Some(e.to_string()),
            },
        }
    }
}

#[derive(Debug)]
pub enum ForgeError {
    /// Missing or invalid configuration (no entry for the host, no token, a bad URL).
    Config(String),
    /// The server answered with an error status.
    Status { status: u16, message: String },
    /// The request failed before an answer (DNS, TLS, timeout, a redirect we refuse to follow).
    Transport(String),
    /// The answer wasn't what the API documents.
    Parse(String),
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeError::Config(m) => write!(f, "{m}"),
            ForgeError::Status { status, message } if message.is_empty() => write!(f, "server answered {status}"),
            ForgeError::Status { status, message } => write!(f, "server answered {status}: {message}"),
            ForgeError::Transport(m) => write!(f, "request failed: {m}"),
            ForgeError::Parse(m) => write!(f, "unexpected answer: {m}"),
        }
    }
}

impl std::error::Error for ForgeError {}

pub type Result<T> = std::result::Result<T, ForgeError>;

/// One forge's merge request API. Every method is `Blocking: call from spawn_blocking`. Writes
/// (drafts, submit, resolve) are added by the review-writing subtask.
pub trait Forge: Send + Sync {
    /// The open merge requests whose head branch is `head_branch`.
    fn find_open(&self, repo: &RepoRef, head_branch: &str) -> Result<Vec<MergeRequest>>;
    fn merge_request(&self, repo: &RepoRef, number: u64) -> Result<MergeRequest>;
    /// Others' (and the user's own submitted) comments.
    fn comments(&self, repo: &RepoRef, number: u64) -> Result<MrComments>;
    /// The token's user (to tell which comments are the user's own).
    fn current_user(&self) -> Result<String>;
    /// Posts the review: its comments at their anchors (on `head_sha`) and the verdict. The
    /// outcome says which comments the forge took; the others stay drafts.
    fn submit_review(&self, repo: &RepoRef, number: u64, head_sha: &str, review: &DraftReview) -> SubmitOutcome;
    /// Replies in the thread (`thread_id` is `Thread::id`); posted at once.
    fn reply(&self, repo: &RepoRef, number: u64, thread_id: &str, body: &str) -> Result<()>;
    fn set_resolved(&self, repo: &RepoRef, thread_id: &str, resolved: bool) -> Result<()>;
    /// `range`: the lines of the thread's first comment, for adapters that keep a multi-line range
    /// in the body (Gitea).
    fn edit_comment(&self, repo: &RepoRef, comment_id: &str, body: &str, range: Option<(u32, u32)>) -> Result<()>;
    fn delete_comment(&self, repo: &RepoRef, comment_id: &str) -> Result<()>;
}

/// The `Forge` for a host: looks the host up in `config` (then the detected `github.com` /
/// `gitlab.com`), reads its token from the environment and builds the client.
///
/// Blocking: reads the environment and builds a TLS config; call from spawn_blocking.
pub fn connect(
    config: &config::ForgeConfig,
    host: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Box<dyn Forge>> {
    let resolved = config
        .resolve(host)
        .ok_or_else(|| ForgeError::Config(format!("no forge configured for {host}: add it to forges.toml")))?;
    let client = http::Client::new(&resolved, env)?;
    match resolved.kind {
        Kind::Gitea => Ok(Box::new(gitea::Gitea::new(client))),
        Kind::GitHub => Ok(Box::new(github::GitHub::new(client))),
        Kind::GitLab => Ok(Box::new(gitlab::GitLab::new(client))),
    }
}
