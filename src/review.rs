//! The Code Review panel's model (no drawing, no app state beyond [`ReviewState`]; `app/review.rs`
//! drives it, `ui/review.rs` draws it): the merge request found for the checked-out branch, its
//! changed files, the viewed marks and the file list's selection and scroll.
//!
//! [`lookup`] is the whole blocking pipeline: the local branch and remote (`git::range`), the
//! host's forge (`forge::connect`; its token comes from the environment through [`Env`]), the open
//! merge requests of the branch, the merge base and the files changed since it. It ends in an
//! [`Outcome`]: a loaded review, one of the [`Empty`] states, or a failure (the panel then keeps
//! the last result and shows the error above it).
//!
//! The viewed marks are local ([`Viewed`], `review-viewed.json` in the state directory): a file is
//! viewed while its current content hash is the one stored when it was marked.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::forge::config::ForgeConfig;
use crate::forge::{self, Comment, MergeRequest, RepoRef, Thread};
use crate::git::Repo;
use crate::git::range::{self, FileChange, RangeError};
use crate::logs::ToolLog;
use crate::review_anchor::{self, Placement};
use crate::review_drafts::{DraftRow, DraftStore};
use crate::scrollbar::ListScroll;

/// Reads an environment variable (the tests give their own).
pub type Env = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// How often a visible panel looks again.
pub const POLL: Duration = Duration::from_secs(60);

/// A loaded review.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub repo: RepoRef,
    pub mr: MergeRequest,
    /// The numbers of every open merge request of the branch, newest first (the switcher).
    pub candidates: Vec<u64>,
    /// The commit the files are diffed against; `None` when it couldn't be found.
    pub merge_base: Option<String>,
    /// The files changed since the merge base, in the diff's order.
    pub files: Vec<FileChange>,
    /// Why `files` is empty although the merge request has changes (`run git fetch`).
    pub files_error: Option<String>,
    /// The line threads, by path and line, each with where it sits now.
    pub threads: Vec<ThreadRow>,
    /// The merge request's own conversation (plain comments, review summaries), read-only.
    pub conversation: Vec<Comment>,
    /// Why the comments couldn't be read (the rest of the review is still shown).
    pub comments_error: Option<String>,
    /// How local HEAD related to the merge request's head when this was looked up.
    pub relation: range::HeadRelation,
    /// The host's entry in `forges.toml` sets `mcp_read_only`: Claude's tools may not apply
    /// suggestions or draft replies.
    pub mcp_read_only: bool,
}

/// A thread and where [`review_anchor`] put it.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub thread: Thread,
    pub place: Placement,
}

impl Review {
    /// Threads nobody resolved.
    pub fn unresolved(&self) -> usize {
        self.threads.iter().filter(|t| !t.thread.resolved).count()
    }

    /// Rows of the CONVERSATIONS list: the threads, then the conversation's comments.
    pub fn conv_len(&self) -> usize {
        self.threads.len() + self.conversation.len()
    }

    /// This merge request's place in the switcher (1-based) and the number of candidates.
    pub fn position(&self) -> (usize, usize) {
        let i = self.candidates.iter().position(|n| *n == self.mr.number).unwrap_or(0);
        (i + 1, self.candidates.len())
    }

    /// The candidate after this one, wrapping.
    pub fn next_candidate(&self) -> Option<u64> {
        let i = self.candidates.iter().position(|n| *n == self.mr.number)?;
        self.candidates.get((i + 1) % self.candidates.len()).copied()
    }
}

/// Why the panel has no merge request to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Empty {
    NoRepo,
    /// No remote, or one whose URL isn't a repository path.
    NoRemote,
    UnknownHost(String),
    /// The environment variable that should hold the host's token is unset.
    NoToken(String),
    Detached,
    /// No open merge request for this branch.
    NoMr(String),
}

impl Empty {
    pub fn text(&self) -> String {
        match self {
            Empty::NoRepo => "No git repository.".into(),
            Empty::NoRemote => "The repository has no remote on a known forge.".into(),
            Empty::UnknownHost(h) => format!("{h} is not a configured forge: add it to forges.toml."),
            Empty::NoToken(var) => format!("No token: set {var} in the environment, or token in forges.toml."),
            Empty::Detached => "No branch is checked out.".into(),
            Empty::NoMr(b) => format!("No open merge request for {b}."),
        }
    }
}

/// What a lookup found.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Empty(Empty),
    /// The review, and the viewed marks as read from disk when the request asked for them.
    Loaded(Box<Review>, Option<Viewed>),
    Failed(String),
}

/// What [`lookup`] needs.
pub struct Request {
    pub repo: Repo,
    pub config: Option<Arc<ForgeConfig>>,
    pub env: Env,
    /// The merge request the user switched to, if still open.
    pub choose: Option<u64>,
    /// Read the viewed marks from here (the app has none in memory yet).
    pub viewed_from: Option<PathBuf>,
    pub log: ToolLog,
}

/// The given `forges.toml`, else the user's file.
///
/// Blocking (file): call from spawn_blocking.
pub fn forge_config(given: &Option<Arc<ForgeConfig>>) -> Arc<ForgeConfig> {
    match given {
        Some(c) => c.clone(),
        None => Arc::new(forge::config::load_default()),
    }
}

/// Blocking (git, files, network): the whole pipeline; call from spawn_blocking.
pub fn lookup(req: Request) -> Outcome {
    let log = &req.log;
    let info = match range::local_info(&req.repo) {
        Ok(i) => i,
        Err(e) => return fail(log, format!("git: {e}")),
    };
    if info.detached {
        return Outcome::Empty(Empty::Detached);
    }
    let Some(repo_ref) = info.remote_url.as_deref().and_then(forge::url::parse_remote) else {
        return Outcome::Empty(Empty::NoRemote);
    };
    let config = forge_config(&req.config);
    for e in &config.errors {
        log.error(e);
    }
    let Some(resolved) = config.resolve(&repo_ref.host) else {
        return Outcome::Empty(Empty::UnknownHost(repo_ref.host));
    };
    if resolved.token.is_none() && (req.env)(&resolved.token_env).is_none_or(|t| t.trim().is_empty()) {
        return Outcome::Empty(Empty::NoToken(resolved.token_env));
    }
    let env = req.env.clone();
    let api = match forge::connect(&config, &repo_ref.host, move |n| env(n)) {
        Ok(f) => f,
        Err(e) => return fail(log, e.to_string()),
    };
    log.info(&format!(
        "Looking for open merge requests of {} in {}",
        info.head_branch, repo_ref.path
    ));
    let mut found = match api.find_open(&repo_ref, &info.head_branch) {
        Ok(f) => f,
        Err(e) => return fail(log, e.to_string()),
    };
    if found.is_empty() {
        return Outcome::Empty(Empty::NoMr(info.head_branch));
    }
    found.sort_by_key(|m| std::cmp::Reverse(m.number));
    let candidates: Vec<u64> = found.iter().map(|m| m.number).collect();
    let number = req.choose.filter(|n| candidates.contains(n)).unwrap_or(candidates[0]);
    let mr = match api.merge_request(&repo_ref, number) {
        Ok(m) => m,
        Err(e) => return fail(log, e.to_string()),
    };
    log.info(&format!("Found !{number}: {}", mr.title));
    let base = range::merge_base(
        &req.repo,
        mr.merge_base.as_deref(),
        &mr.base.sha,
        &mr.base.name,
        info.remote.as_deref(),
    );
    let (merge_base, files, files_error) = match base {
        Ok(b) => match range::changes(&req.repo, &b) {
            Ok(files) => (Some(b), files, None),
            Err(e) => (Some(b), Vec::new(), Some(e.to_string())),
        },
        Err(e @ (RangeError::Missing | RangeError::Git(_))) => (None, Vec::new(), Some(e.to_string())),
    };
    if let Some(e) = &files_error {
        log.error(e);
    }
    let (threads, conversation, comments_error) = match api.comments(&repo_ref, number) {
        Ok(c) => {
            let places = review_anchor::place_all(&req.repo, &c.threads, merge_base.as_deref(), &files);
            let mut rows: Vec<ThreadRow> = c
                .threads
                .into_iter()
                .zip(places)
                .map(|(thread, place)| ThreadRow { thread, place })
                .collect();
            rows.sort_by(|a, b| {
                let (a, b) = (&a.thread.anchor, &b.thread.anchor);
                (&a.path, a.start_line).cmp(&(&b.path, b.start_line))
            });
            (rows, c.conversation, None)
        }
        Err(e) => {
            let e = format!("comments: {e}");
            log.error(&e);
            (Vec::new(), Vec::new(), Some(e))
        }
    };
    let viewed = req.viewed_from.as_deref().map(Viewed::load);
    let relation = range::head_relation(&req.repo, &mr.head.sha);
    let mcp_read_only = config.mcp_read_only(&repo_ref.host);
    Outcome::Loaded(
        Box::new(Review {
            repo: repo_ref,
            mr,
            candidates,
            merge_base,
            files,
            files_error,
            threads,
            conversation,
            comments_error,
            relation,
            mcp_read_only,
        }),
        viewed,
    )
}

fn fail(log: &ToolLog, message: String) -> Outcome {
    log.error(&message);
    Outcome::Failed(message)
}

/// The viewed marks: `host/project#number:path` to the content hash the file had when marked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewed {
    map: BTreeMap<String, String>,
}

const VIEWED_FILE: &str = "review-viewed.json";

impl Viewed {
    pub fn key(repo: &RepoRef, number: u64, path: &str) -> String {
        format!("{}/{}#{number}:{path}", repo.host, repo.path)
    }

    pub fn is_viewed(&self, key: &str, hash: &str) -> bool {
        self.map.get(key).is_some_and(|h| h == hash)
    }

    pub fn set(&mut self, key: String, hash: String) {
        self.map.insert(key, hash);
    }

    pub fn clear(&mut self, key: &str) {
        self.map.remove(key);
    }

    /// Forgets this merge request's marks whose file left the list or changed since. Whether
    /// anything went.
    pub fn prune(&mut self, repo: &RepoRef, number: u64, files: &[FileChange]) -> bool {
        let prefix = Viewed::key(repo, number, "");
        let before = self.map.len();
        self.map.retain(|k, h| match k.strip_prefix(&prefix) {
            None => true,
            Some(path) => files.iter().any(|f| f.path == path && f.hash == *h),
        });
        self.map.len() != before
    }

    /// Blocking: the marks saved in `dir` (none when absent or unreadable).
    pub fn load(dir: &Path) -> Viewed {
        let map = std::fs::read_to_string(dir.join(VIEWED_FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Viewed { map }
    }

    /// Blocking: writes the marks to `dir` (through a temporary file).
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{VIEWED_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.map)?)?;
        std::fs::rename(tmp, dir.join(VIEWED_FILE))
    }
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Content {
    /// Nothing looked up yet.
    #[default]
    None,
    Empty(Empty),
    Loaded(Box<Review>),
}

/// HEAD as of the last check: work tree, branch name, commit.
pub type HeadKey = (PathBuf, String, Option<String>);

/// The panel's state (`app.review`).
pub struct ReviewState {
    pub content: Content,
    /// The last lookup failed with this; the previous content stays under it.
    pub error: Option<String>,
    /// A lookup is running.
    pub loading: bool,
    /// Another lookup was asked for while one ran.
    pub queued: bool,
    pub view: ReviewView,
    /// The viewed marks, once read (with the first lookup).
    pub viewed: Option<Viewed>,
    /// The merge request switched to.
    pub choice: Option<u64>,
    pub head_key: Option<HeadKey>,
    /// When a visible panel looks again.
    pub next_poll: Option<Instant>,
    pub interval: Duration,
    /// `forges.toml` as loaded; `None` makes each lookup read the user's file (tests give one).
    pub config: Option<Arc<ForgeConfig>>,
    pub env: Env,
    /// Look up at launch and when HEAD moves (off in tests, which opt in).
    pub auto: bool,
    /// Counts what review diff tabs show besides their text (a lookup's result, a draft or the
    /// comment box changing): they copy their blocks when it changed.
    pub generation: u64,
    pub drafts: Drafts,
    /// The token's user, once the forge said (to find the user's own comments).
    pub user: Option<String>,
    pub user_loading: bool,
    /// A write (submit, reply, resolve, edit, delete) is on its way: one at a time.
    pub writing: bool,
    /// When the loaded review was looked up.
    pub fetched_at: Option<std::time::SystemTime>,
}

impl Review {
    /// Why local HEAD can't be submitted from, when it isn't the merge request's head: the
    /// banner's text.
    pub fn head_banner(&self, local_head: Option<&str>) -> Option<&'static str> {
        use range::HeadRelation as R;
        if local_head.is_some_and(|h| h == self.mr.head.sha) {
            return None;
        }
        Some(match self.relation {
            R::Ahead => "local is ahead of the merge request: push",
            R::Behind => "local is behind the merge request: pull",
            R::Diverged => "local and the merge request have diverged",
            // Looked up when HEAD was the same, but it moved since.
            R::Same | R::Unknown => "local is not at the merge request's head",
        })
    }
}

impl Drafts {
    /// `review`'s drafts in file and line order, borrowed (a field of the state, so a caller can
    /// hold the rows while changing another field: `ReviewState::draft_rows` is the shorthand).
    pub fn rows<'a>(&'a self, review: &Review) -> Vec<DraftRow<'a>> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        let key = DraftStore::key(&review.repo, review.mr.number);
        let mut rows: Vec<DraftRow> = store
            .get(&key)
            .iter()
            .map(|d| DraftRow {
                held: self
                    .live
                    .get(&d.id)
                    .or_else(|| self.drift.get(&d.id))
                    .and_then(|h| h.as_deref()),
                draft: d,
            })
            .collect();
        rows.sort_by(|a, b| {
            (&a.draft.path, a.draft.right, a.draft.start, a.draft.created).cmp(&(
                &b.draft.path,
                b.draft.right,
                b.draft.start,
                b.draft.created,
            ))
        });
        rows
    }
}

/// The drafts as the app holds them (`app/review_comment.rs` changes them).
#[derive(Debug, Default)]
pub struct Drafts {
    /// Every merge request's drafts, once read (`None` until then).
    pub store: Option<DraftStore>,
    pub loading: bool,
    /// Why a draft is held back, by id, as of the last check against the files on disk.
    pub drift: HashMap<String, Option<String>>,
    /// The same as the open review diff tabs see their file's text now (it wins over `drift`).
    pub live: HashMap<String, Option<String>>,
    /// Why the forge refused a draft at the last submit, by id (until the next submit).
    pub errors: HashMap<String, String>,
}

impl Default for ReviewState {
    fn default() -> Self {
        ReviewState {
            content: Content::None,
            error: None,
            loading: false,
            queued: false,
            view: ReviewView::default(),
            viewed: None,
            choice: None,
            head_key: None,
            next_poll: None,
            interval: POLL,
            config: cfg!(test).then(|| Arc::new(ForgeConfig::default())),
            auto: !cfg!(test),
            generation: 0,
            drafts: Drafts::default(),
            user: None,
            user_loading: false,
            writing: false,
            fetched_at: None,
            env: Arc::new(|name| std::env::var(name).ok()),
        }
    }
}

/// A section under FILES in the Code Review panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Conversations,
    Pending,
}

impl ReviewState {
    /// The sections under FILES, top to bottom, with their wishes (for `side_sections::split`):
    /// CONVERSATIONS while there are any (or their error), PENDING while there are drafts.
    pub fn sections(&self) -> Vec<(Pane, crate::side_sections::Section)> {
        let Some(review) = self.review() else {
            return Vec::new();
        };
        let section = |c: &Conversations| crate::side_sections::Section {
            height: c.section_height,
            collapsed: c.collapsed,
        };
        let mut out = Vec::new();
        if review.conv_len() > 0 || review.comments_error.is_some() {
            out.push((Pane::Conversations, section(&self.view.conv)));
        }
        if !self.drafts.rows(review).is_empty() {
            out.push((Pane::Pending, section(&self.view.pending)));
        }
        out
    }

    pub fn review(&self) -> Option<&Review> {
        match &self.content {
            Content::Loaded(r) => Some(r),
            _ => None,
        }
    }

    pub fn files(&self) -> &[FileChange] {
        self.review().map_or(&[], |r| &r.files)
    }

    pub fn file_count(&self) -> usize {
        self.files().len()
    }

    /// The list's selection state and the files together (disjoint borrows: no copy of the list).
    pub fn split(&mut self) -> (&mut ReviewView, &[FileChange]) {
        let files = match &self.content {
            Content::Loaded(r) => r.files.as_slice(),
            _ => &[],
        };
        (&mut self.view, files)
    }

    /// The key of the shown merge request's drafts.
    pub fn drafts_key(&self) -> Option<String> {
        let r = self.review()?;
        Some(DraftStore::key(&r.repo, r.mr.number))
    }

    /// The shown merge request's drafts by file and line, each with why it is held back (if it
    /// is): what Submit Review will consume (it posts the ones not held back).
    pub fn draft_rows(&self) -> Vec<DraftRow<'_>> {
        match &self.content {
            Content::Loaded(r) => self.drafts.rows(r),
            _ => Vec::new(),
        }
    }

    /// How many drafts the shown merge request has (`draft_rows().len()` without building and
    /// sorting the rows: the prompt hook asks per prompt).
    pub fn draft_count(&self) -> usize {
        let (Some(store), Some(key)) = (&self.drafts.store, self.drafts_key()) else {
            return 0;
        };
        store.get(&key).len()
    }

    /// The viewed-state key of `file` in the shown merge request.
    pub fn viewed_key(&self, file: &FileChange) -> Option<String> {
        let r = self.review()?;
        Some(Viewed::key(&r.repo, r.mr.number, &file.path))
    }

    /// Files marked viewed.
    pub fn viewed_count(&self) -> usize {
        let (Some(v), Some(r)) = (&self.viewed, self.review()) else {
            return 0;
        };
        // One key buffer for the whole list (this runs every frame).
        let mut key = Viewed::key(&r.repo, r.mr.number, "");
        let prefix = key.len();
        r.files
            .iter()
            .filter(|f| {
                key.truncate(prefix);
                key.push_str(&f.path);
                v.is_viewed(&key, &f.hash)
            })
            .count()
    }
}

/// The file list's selection (by path) and scroll.
#[derive(Debug, Default)]
pub struct ReviewView {
    top: usize,
    /// The CONVERSATIONS section.
    pub conv: Conversations,
    /// The PENDING section: likewise (its rows are one line tall).
    pub pending: Conversations,
    selected: Option<(String, usize)>,
    /// Rows drawn in the last frame (0 until the first draw).
    pub height: usize,
    reveal_pending: bool,
}

/// The CONVERSATIONS section's state: its dragged height (divider and header included), whether
/// it is collapsed, whether it has the keys, and its list's selection (an index into
/// `Review::threads` then `Review::conversation`) and scroll. A row is two lines tall.
#[derive(Debug)]
pub struct Conversations {
    pub section_height: u16,
    pub collapsed: bool,
    pub focused: bool,
    selected: usize,
    top: usize,
    /// Rows (not lines) drawn in the last frame.
    pub height: usize,
}

impl Default for Conversations {
    fn default() -> Self {
        Conversations {
            section_height: 8,
            collapsed: false,
            focused: false,
            selected: 0,
            top: 0,
            height: 0,
        }
    }
}

impl Conversations {
    fn list(&self) -> ListScroll {
        ListScroll {
            top: self.top,
            height: self.height,
        }
    }

    /// The selected row, clamped to a list of `len`.
    pub fn selected(&self, len: usize) -> Option<usize> {
        (len > 0).then(|| self.selected.min(len - 1))
    }

    pub fn select(&mut self, len: usize, row: usize) {
        if len == 0 {
            return;
        }
        self.selected = row.min(len - 1);
        if self.height > 0 {
            let mut l = self.list();
            l.reveal(len, self.selected);
            self.top = l.top;
        }
    }

    pub fn move_selection(&mut self, len: usize, delta: isize) {
        let at = self.selected(len).unwrap_or(0);
        self.select(len, at.saturating_add_signed(delta));
    }

    pub fn top(&self, len: usize) -> usize {
        self.list().scroll(len)
    }

    pub fn scroll_by(&mut self, len: usize, delta: isize) {
        let mut l = self.list();
        l.scroll_by(len, delta);
        self.top = l.top;
    }
}

/// The list's scroll bar (`ReviewView::scrollable`).
pub struct ReviewScroll<'a> {
    view: &'a mut ReviewView,
    rows: usize,
}

impl crate::scrollbar::Scrollable for ReviewScroll<'_> {
    fn scroll(&self) -> crate::scrollbar::Scroll {
        self.view.list().scrollbar(self.rows)
    }

    fn set_top(&mut self, top: usize) {
        self.view.top = top;
        self.view.top = self.view.list().scroll(self.rows);
    }
}

impl ReviewView {
    pub fn scrollable(&mut self, rows: usize) -> ReviewScroll<'_> {
        ReviewScroll { view: self, rows }
    }

    fn list(&self) -> ListScroll {
        ListScroll {
            top: self.top,
            height: self.height,
        }
    }

    /// The selected file's index: its path's row, else (it left) the row now at its last index.
    pub fn selected_row(&self, files: &[FileChange]) -> Option<usize> {
        let (path, last) = self.selected.as_ref()?;
        if files.is_empty() {
            return None;
        }
        Some(
            files
                .iter()
                .position(|f| f.path == *path)
                .unwrap_or((*last).min(files.len() - 1)),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_path(&self) -> Option<&str> {
        self.selected.as_ref().map(|(p, _)| p.as_str())
    }

    pub fn select(&mut self, files: &[FileChange], row: usize) {
        if files.is_empty() {
            self.selected = None;
            return;
        }
        let row = row.min(files.len() - 1);
        self.selected = Some((files[row].path.clone(), row));
        self.reveal_selected(files);
    }

    pub fn select_path(&mut self, files: &[FileChange], path: &str) -> bool {
        match files.iter().position(|f| f.path == path) {
            Some(i) => {
                self.select(files, i);
                true
            }
            None => false,
        }
    }

    /// Keeps the selection on its file as the list changes (each frame).
    pub fn settle(&mut self, files: &[FileChange]) {
        let Some((path, last)) = &self.selected else { return };
        match self.selected_row(files) {
            // Unchanged (every frame): no new path string.
            Some(i) if i == *last && files[i].path == *path => {}
            Some(i) => self.selected = Some((files[i].path.clone(), i)),
            None => self.selected = None,
        }
    }

    pub fn move_selection(&mut self, files: &[FileChange], delta: isize) {
        match self.selected_row(files) {
            Some(i) => self.select(files, i.saturating_add_signed(delta)),
            None => self.select(files, 0),
        }
    }

    pub fn scroll(&self, len: usize) -> usize {
        self.list().scroll(len)
    }

    pub fn scroll_by(&mut self, len: usize, delta: isize) {
        let mut l = self.list();
        l.scroll_by(len, delta);
        self.top = l.top;
    }

    pub fn clamp(&mut self, len: usize) {
        self.top = self.scroll(len);
    }

    /// The drawn height (each frame); a reveal asked for before the first draw runs now.
    pub fn set_height(&mut self, files: &[FileChange], height: usize) {
        self.height = height;
        if self.reveal_pending && height > 0 {
            self.reveal_pending = false;
            self.reveal_selected(files);
        }
    }

    pub fn reveal_selected(&mut self, files: &[FileChange]) {
        if self.height == 0 {
            self.reveal_pending = true;
            return;
        }
        let Some(row) = self.selected_row(files) else { return };
        let mut l = self.list();
        l.reveal(files.len(), row);
        self.top = l.top;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, hash: &str) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: None,
            status: 'M',
            additions: 1,
            deletions: 0,
            hash: hash.into(),
        }
    }

    fn repo() -> RepoRef {
        RepoRef {
            host: "git.example.com".into(),
            path: "alice/project".into(),
        }
    }

    #[test]
    fn a_viewed_mark_holds_only_for_the_same_content() {
        let mut v = Viewed::default();
        let key = Viewed::key(&repo(), 42, "a.rs");
        v.set(key.clone(), "h1".into());
        assert!(v.is_viewed(&key, "h1"));
        assert!(!v.is_viewed(&key, "h2"));
        // Pruning drops changed and vanished files of this request only.
        v.set(Viewed::key(&repo(), 42, "gone.rs"), "x".into());
        v.set(Viewed::key(&repo(), 43, "a.rs"), "other".into());
        assert!(v.prune(&repo(), 42, &[file("a.rs", "h2")]));
        assert!(!v.is_viewed(&key, "h1"));
        assert!(v.is_viewed(&Viewed::key(&repo(), 43, "a.rs"), "other"));
        assert!(!v.prune(&repo(), 42, &[file("a.rs", "h2")]));
    }

    #[test]
    fn viewed_marks_round_trip_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = Viewed::default();
        v.set(Viewed::key(&repo(), 1, "a"), "h".into());
        v.save(dir.path()).unwrap();
        assert_eq!(Viewed::load(dir.path()), v);
        assert_eq!(Viewed::load(&dir.path().join("nothing")), Viewed::default());
    }

    #[test]
    fn the_selection_follows_its_file() {
        let mut view = ReviewView::default();
        let files = vec![file("a", ""), file("b", ""), file("c", "")];
        view.select(&files, 1);
        assert_eq!(view.selected_path(), Some("b"));
        let moved = vec![file("c", ""), file("b", "")];
        assert_eq!(view.selected_row(&moved), Some(1));
        view.settle(&moved);
        view.move_selection(&moved, -5);
        assert_eq!(view.selected_path(), Some("c"));
        // Its file left: the selection stays at its row.
        view.select(&files, 2);
        assert_eq!(view.selected_row(&[file("a", "")]), Some(0));
    }
}
