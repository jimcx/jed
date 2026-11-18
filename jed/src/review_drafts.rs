//! Review comments written locally and not posted yet (no app state, no drawing): [`Draft`]s kept
//! per merge request in `review-drafts.json` in the state directory, so they survive a restart and
//! a failed submit. `app/review_comment.rs` drives them, `review_blocks` shows them.
//!
//! A draft remembers the text of the lines it was written on. Where those lines no longer read the
//! same (a local edit, a rebase) the draft is *held back* ([`Draft::drift`]): named in the PENDING
//! list, not drawn in the diff, and never posted at a guessed position. [`check`] judges drafts
//! against the files on disk (off-thread, at lookup time); a review diff tab judges the drafts of
//! its file against its live text.
//!
//! Every change writes the whole file ([`DraftStore::save`], from `spawn_blocking`). Writes carry a
//! sequence number, so a write that lost the race to a newer one is skipped and the last change
//! always lands. Drafts leave the file only when asked (the forge confirming them comes with the
//! submit, later).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::forge::{Anchor, Kind, RepoRef, Side};
use crate::git::Repo;
use crate::git::diff::Hunk;
use crate::git::range::FileChange;
use crate::logs::ToolLog;

const FILE: &str = "review-drafts.json";

/// Why a draft is held back.
pub const LINE_CHANGED: &str = "line changed";
pub const LINES_CHANGED: &str = "lines changed";
pub const FILE_CHANGED: &str = "file changed";
/// A draft found in the forge's pending review whose commit the clone lacks (so its lines are unknown).
pub const COMMIT_MISSING: &str = "commit not in the clone: fetch";
/// GitHub refuses a comment on a line outside the merge request's diff hunks.
pub const NOT_IN_DIFF: &str = "not in the merge request's diff";

/// One comment not posted yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    /// Relative to the work tree.
    pub path: String,
    /// The side the lines belong to (`true`: the work tree's, `false`: the merge base's).
    pub right: bool,
    /// The first and last line, 1-based, inclusive.
    pub start: u32,
    pub end: u32,
    /// The commit the lines are numbered in: the merge request's head (right side) or the merge
    /// base (left side).
    pub commit: String,
    /// The text of the lines when the draft was written.
    pub lines: Vec<String>,
    pub body: String,
    /// Seconds since the epoch.
    pub created: u64,
    /// A reply to this forge thread (a reply draft: no line anchor of its own, so it is never
    /// held back and never part of the review's line comments; Submit Review posts it after the
    /// review). Old store files have none.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// A reply draft: resolve the thread once the reply is posted.
    #[serde(default)]
    pub resolve: bool,
    /// Written by Claude through the MCP tool (PENDING tags it `claude`).
    #[serde(default)]
    pub by_claude: bool,
    /// Where the draft lives in the forge's pending review, once the forge took it (old store
    /// files have none: local only).
    #[serde(default)]
    pub forge: Option<ForgeDraft>,
}

/// A draft's place in the forge's pending review (`forge::DraftRef`, saved with the draft).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeDraft {
    pub review_id: String,
    pub comment_id: String,
}

impl ForgeDraft {
    pub fn of(r: &crate::forge::DraftRef) -> ForgeDraft {
        ForgeDraft {
            review_id: r.review_id.clone(),
            comment_id: r.comment_id.clone(),
        }
    }

    pub fn to_ref(&self) -> crate::forge::DraftRef {
        crate::forge::DraftRef {
            review_id: self.review_id.clone(),
            comment_id: self.comment_id.clone(),
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Draft {
    pub fn new(
        path: String,
        right: bool,
        start: u32,
        end: u32,
        commit: String,
        lines: Vec<String>,
        body: String,
    ) -> Draft {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Draft {
            id: format!("{:x}-{n:x}", now.as_nanos()),
            path,
            right,
            start,
            end: end.max(start),
            commit,
            lines,
            body,
            created: now.as_secs(),
            reply_to: None,
            resolve: false,
            by_claude: false,
            forge: None,
        }
    }

    /// A reply draft to thread `thread` (`path` and the thread's lines `start..=end` are only
    /// there so the panel can sort and show it).
    pub fn reply(path: String, right: bool, lines: (u32, u32), thread: String, body: String, resolve: bool) -> Draft {
        Draft {
            reply_to: Some(thread),
            resolve,
            ..Draft::new(path, right, lines.0, lines.1, String::new(), Vec::new(), body)
        }
    }

    /// What the forge calls the draft's place (Submit Review posts it there).
    pub fn anchor(&self) -> Anchor {
        Anchor {
            path: self.path.clone(),
            side: if self.right { Side::Right } else { Side::Left },
            start_line: self.start,
            end_line: self.end,
            commit: self.commit.clone(),
        }
    }

    /// `path:line` (the first line).
    pub fn place(&self) -> String {
        if self.reply_to.is_some() {
            return format!("reply {}:{}", self.path, self.start);
        }
        format!("{}:{}", self.path, self.start)
    }

    /// What PENDING shows before the body: `[claude]` for Claude's, `+ resolve` for a reply that
    /// resolves its thread.
    pub fn tags(&self) -> String {
        let mut tags = String::new();
        if self.by_claude {
            tags.push_str("[claude] ");
        }
        if self.resolve {
            tags.push_str("+ resolve ");
        }
        tags
    }

    /// Why the draft is held back when its lines, found from `first` (0-based: where local edits
    /// above moved them), no longer read as they did; `None` when they do (a reply draft never is).
    pub fn drift_from<'a>(&self, first: usize, line: impl Fn(usize) -> Option<&'a str>) -> Option<&'static str> {
        if self.reply_to.is_some() {
            return None;
        }
        let same = self
            .lines
            .iter()
            .enumerate()
            .all(|(i, text)| line(first + i) == Some(text.as_str()));
        if same {
            None
        } else if self.lines.len() > 1 {
            Some(LINES_CHANGED)
        } else {
            Some(LINE_CHANGED)
        }
    }

    /// Whether the forge `kind` takes the draft where it is, given the merge request's hunks:
    /// false for GitHub when its lines aren't all inside one hunk. A reply, another forge or a
    /// file with unknown hunks passes.
    pub fn outside_ok(&self, kind: Kind, hunks: &BTreeMap<String, FileHunks>) -> bool {
        if kind != Kind::GitHub || self.reply_to.is_some() {
            return true;
        }
        hunks
            .get(&self.path)
            .is_none_or(|h| h.in_diff(self.right, self.start, self.end))
    }

    /// `drift` against a whole file's bytes (`None`: the file is gone).
    pub fn drift_in(&self, text: Option<&[u8]>) -> Option<&'static str> {
        if self.reply_to.is_some() {
            return None;
        }
        let Some(text) = text else { return Some(FILE_CHANGED) };
        let text = String::from_utf8_lossy(text);
        let lines: Vec<&str> = text.lines().collect();
        self.drift_from((self.start.max(1) - 1) as usize, |i| lines.get(i).copied())
    }

    /// Why the draft is held back when its lines were changed in place (`LINE(S)_CHANGED`).
    pub fn changed(&self) -> &'static str {
        if self.lines.len() > 1 {
            LINES_CHANGED
        } else {
            LINE_CHANGED
        }
    }

    /// Where the draft's lines are in the text a review diff shows (0-based): `Some(None)` when a
    /// commit since changed them, `None` when its commit wasn't compared there.
    pub fn place_in(&self, live: &crate::review_anchor::Live) -> Option<Option<std::ops::Range<usize>>> {
        match live.place(&self.commit, self.start, self.end)? {
            crate::review_anchor::Placement::Placed { lines, .. } => Some(Some(lines)),
            _ => Some(None),
        }
    }
}

/// A draft with where it stands now: what `Submit Review` will consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow<'a> {
    pub draft: &'a Draft,
    /// Why it is held back (not posted): `None` when its lines are unchanged.
    pub held: Option<&'a str>,
}

/// What [`DraftStore::reconcile`] changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Drafts made from comments found in the pending review.
    pub added: usize,
    /// The ids of drafts that were on the forge and are gone (removed from the store).
    pub dropped: Vec<String>,
    /// Drafts whose body the forge's text replaced.
    pub changed: usize,
}

/// Every merge request's drafts, by [`DraftStore::key`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftStore {
    map: BTreeMap<String, Vec<Draft>>,
    /// The overall body of the review to submit, per merge request (never empty).
    summaries: BTreeMap<String, String>,
}

/// The file: the bare drafts map older versions wrote, or (when a summary is kept) both.
#[derive(Deserialize)]
#[serde(untagged)]
enum FileForm {
    Both {
        drafts: BTreeMap<String, Vec<Draft>>,
        #[serde(default)]
        summaries: BTreeMap<String, String>,
    },
    Bare(BTreeMap<String, Vec<Draft>>),
}

static SEQ: AtomicU64 = AtomicU64::new(1);
/// The sequence number last written to each directory.
static WRITTEN: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

impl DraftStore {
    pub fn key(repo: &RepoRef, number: u64) -> String {
        format!("{}/{}#{number}", repo.host, repo.path)
    }

    pub fn get(&self, key: &str) -> &[Draft] {
        self.map.get(key).map_or(&[], |v| v.as_slice())
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty() && self.summaries.is_empty()
    }

    /// The review summary kept for merge request `key` (empty when none).
    pub fn summary(&self, key: &str) -> &str {
        self.summaries.get(key).map_or("", |s| s.as_str())
    }

    /// Keeps `text` as the summary of `key`; empty text removes it.
    pub fn set_summary(&mut self, key: &str, text: &str) {
        if text.is_empty() {
            self.summaries.remove(key);
        } else {
            self.summaries.insert(key.to_string(), text.to_string());
        }
    }

    pub fn add(&mut self, key: &str, draft: Draft) {
        self.map.entry(key.to_string()).or_default().push(draft);
    }

    /// The reply draft to thread `thread` of merge request `key`, if there is one.
    pub fn reply_for(&self, key: &str, thread: &str) -> Option<&Draft> {
        self.get(key).iter().find(|d| d.reply_to.as_deref() == Some(thread))
    }

    /// Adds reply draft `draft`, or replaces the body of the one its thread already has (a
    /// thread holds one, which keeps its id); whether it replaced.
    pub fn set_reply(&mut self, key: &str, draft: Draft) -> bool {
        let list = self.map.entry(key.to_string()).or_default();
        if let Some(old) = list
            .iter_mut()
            .find(|d| d.reply_to.is_some() && d.reply_to == draft.reply_to)
        {
            old.body = draft.body;
            old.resolve = draft.resolve;
            old.by_claude = draft.by_claude;
            return true;
        }
        list.push(draft);
        false
    }

    /// Sets the body of draft `id`; whether it exists.
    pub fn set_body(&mut self, key: &str, id: &str, body: String) -> bool {
        let Some(d) = self.map.get_mut(key).and_then(|v| v.iter_mut().find(|d| d.id == id)) else {
            return false;
        };
        d.body = body;
        true
    }

    /// The draft `id` of merge request `key`.
    pub fn find(&self, key: &str, id: &str) -> Option<&Draft> {
        self.get(key).iter().find(|d| d.id == id)
    }

    /// Records where the forge keeps draft `id` (`None`: it is local only); whether it exists.
    pub fn set_forge(&mut self, key: &str, id: &str, forge: Option<ForgeDraft>) -> bool {
        let Some(d) = self.map.get_mut(key).and_then(|v| v.iter_mut().find(|d| d.id == id)) else {
            return false;
        };
        d.forge = forge;
        true
    }

    /// Makes draft `id` the user's (an edit of Claude's draft): it is no longer tagged `claude`.
    pub fn set_by_user(&mut self, key: &str, id: &str) {
        if let Some(d) = self.map.get_mut(key).and_then(|v| v.iter_mut().find(|d| d.id == id)) {
            d.by_claude = false;
        }
    }

    pub fn remove(&mut self, key: &str, id: &str) -> Option<Draft> {
        let list = self.map.get_mut(key)?;
        let i = list.iter().position(|d| d.id == id)?;
        let draft = list.remove(i);
        if list.is_empty() {
            self.map.remove(key);
        }
        Some(draft)
    }

    /// Adds `other`'s drafts that this store doesn't have (a read that finished after a draft was
    /// added).
    pub fn merge(&mut self, other: DraftStore) {
        for (key, text) in other.summaries {
            self.summaries.entry(key).or_insert(text);
        }
        for (key, list) in other.map {
            for d in list {
                if !self.get(&key).iter().any(|e| e.id == d.id) {
                    self.add(&key, d);
                }
            }
        }
    }

    /// Folds the forge's pending review `pending` into merge request `key`'s drafts: a comment
    /// with no draft of its own (made on the website or another machine) becomes a draft with its
    /// forge ids; a draft that was on the forge and no longer is (submitted or deleted there) is
    /// dropped; a body changed there wins; a draft whose lines weren't known gets them once the
    /// commit is read. `busy(id)`: a draft with a write on its way is left alone.
    pub fn reconcile(
        &mut self,
        key: &str,
        pending: &crate::review::Pending,
        threads: &[crate::review::ThreadRow],
        busy: &dyn Fn(&str) -> bool,
    ) -> Reconciled {
        let mut out = Reconciled::default();
        let list = self.map.entry(key.to_string()).or_default();
        let comment = |id: &str| pending.review.comments.iter().find(|c| c.id == id);
        list.retain_mut(|d| {
            let Some(f) = d.forge.as_mut() else { return true };
            if busy(&d.id) {
                return true;
            }
            let Some(c) = comment(&f.comment_id) else {
                out.dropped.push(d.id.clone());
                return false;
            };
            f.review_id = pending.review.id.clone();
            if c.body != d.body {
                d.body = c.body.clone();
                out.changed += 1;
            }
            if d.reply_to.is_none()
                && d.lines.is_empty()
                && let Some(Some(lines)) = pending.lines.get(&c.id)
            {
                d.lines = lines.clone();
            }
            true
        });
        for c in &pending.review.comments {
            if list
                .iter()
                .any(|d| d.forge.as_ref().is_some_and(|f| f.comment_id == c.id))
            {
                continue;
            }
            let place = ForgeDraft {
                review_id: pending.review.id.clone(),
                comment_id: c.id.clone(),
            };
            let mut draft = match &c.reply_to {
                Some(to) => {
                    let thread = threads.iter().find(|t| match to {
                        crate::forge::ReplyTo::Thread(id) => &t.thread.id == id,
                        crate::forge::ReplyTo::Comment(id) => t.thread.comments.iter().any(|m| &m.id == id),
                    });
                    let Some(t) = thread else { continue };
                    if list.iter().any(|d| d.reply_to.as_deref() == Some(t.thread.id.as_str())) {
                        continue;
                    }
                    let a = &t.thread.anchor;
                    Draft::reply(
                        a.path.clone(),
                        a.side == Side::Right,
                        (a.start_line, a.end_line),
                        t.thread.id.clone(),
                        c.body.clone(),
                        c.resolve,
                    )
                }
                None => {
                    let Some(a) = &c.anchor else { continue };
                    let lines = pending.lines.get(&c.id).cloned().flatten().unwrap_or_default();
                    Draft::new(
                        a.path.clone(),
                        a.side == Side::Right,
                        a.start_line,
                        a.end_line,
                        a.commit.clone(),
                        lines,
                        c.body.clone(),
                    )
                }
            };
            draft.forge = Some(place);
            list.push(draft);
            out.added += 1;
        }
        if list.is_empty() {
            self.map.remove(key);
        }
        out
    }

    /// Blocking: the drafts saved in `dir` (none when absent; an unreadable or corrupt file is
    /// ignored and said in `log`).
    pub fn load(dir: &Path, log: &ToolLog) -> DraftStore {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DraftStore::default(),
            Err(e) => {
                log.error(&format!("can't read the saved drafts: {e}"));
                return DraftStore::default();
            }
        };
        match serde_json::from_str(&text) {
            Ok(FileForm::Both { drafts, summaries }) => DraftStore { map: drafts, summaries },
            Ok(FileForm::Bare(map)) => DraftStore {
                map,
                ..DraftStore::default()
            },
            Err(e) => {
                log.error(&format!("the saved drafts are unreadable and ignored: {e}"));
                DraftStore::default()
            }
        }
    }

    /// A number for the next [`DraftStore::save`]: later calls get larger ones.
    pub fn next_seq() -> u64 {
        SEQ.fetch_add(1, Ordering::SeqCst)
    }

    /// Blocking: writes the drafts to `dir` (through a temporary file), unless a write with a
    /// larger `seq` already happened there.
    pub fn save(&self, dir: &Path, seq: u64) -> std::io::Result<()> {
        let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        if written.get(dir).is_some_and(|&last| last > seq) {
            return Ok(());
        }
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        // The bare map while there is no summary: an older jed still reads the file.
        let text = if self.summaries.is_empty() {
            serde_json::to_string_pretty(&self.map)?
        } else {
            serde_json::to_string_pretty(&serde_json::json!({"drafts": self.map, "summaries": self.summaries}))?
        };
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, dir.join(FILE))?;
        written.insert(dir.to_path_buf(), seq);
        Ok(())
    }
}

/// A file's hunks from the merge base to the merge request's head: what the forge's diff
/// contains, per side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileHunks {
    /// With GitHub's 3 lines of context: the lines a comment may be on.
    pub context: Vec<Hunk>,
    /// Without context: the changed lines.
    pub changes: Vec<Hunk>,
}

impl FileHunks {
    /// Lines `start..=end` (1-based) of the right (or left) side all inside one hunk.
    pub fn in_diff(&self, right: bool, start: u32, end: u32) -> bool {
        let (first, last) = (start.max(1) as usize - 1, end.max(start) as usize);
        self.context.iter().any(|h| {
            let r = if right { &h.new } else { &h.old };
            r.start <= first && last <= r.end
        })
    }

    /// The other side's number (1-based) of an unchanged `line` of this side; `None` when the
    /// line is itself changed.
    pub fn counterpart(&self, right: bool, line: u32) -> Option<u32> {
        let idx = line.max(1) as usize - 1;
        let mine = |h: &Hunk| if right { h.new.clone() } else { h.old.clone() };
        let other = |h: &Hunk| if right { h.old.clone() } else { h.new.clone() };
        if self.changes.iter().any(|h| mine(h).contains(&idx)) {
            return None;
        }
        let delta: isize = self
            .changes
            .iter()
            .filter(|h| mine(h).end <= idx)
            .map(|h| other(h).len() as isize - mine(h).len() as isize)
            .sum();
        Some((idx as isize + delta + 1) as u32)
    }
}

/// The hunks of one file, by its path in the merge request (`None`: a text or commit is
/// missing, or the file is binary, so nothing is known).
///
/// Blocking (git): call from spawn_blocking.
pub fn file_hunks(
    repo: &Repo,
    merge_base: Option<&str>,
    head_sha: &str,
    files: &[FileChange],
) -> BTreeMap<String, FileHunks> {
    let mut out = BTreeMap::new();
    let Ok(git) = repo.open() else { return out };
    let find = |rev: &str| git2::Oid::from_str(rev).ok().and_then(|o| git.find_commit(o).ok());
    let (Some(base), Some(head)) = (merge_base.and_then(find), find(head_sha)) else {
        return out;
    };
    for f in files {
        let old = f.old_path.as_deref().unwrap_or(&f.path);
        let old = crate::review_anchor::blob_at(&git, &base, old).unwrap_or_default();
        let new = crate::review_anchor::blob_at(&git, &head, &f.path).unwrap_or_default();
        let (Ok(context), Ok(changes)) = (
            crate::git::diff::diff(&old, &new, 3),
            crate::git::diff::diff(&old, &new, 0),
        ) else {
            continue;
        };
        if context.binary {
            continue;
        }
        out.insert(
            f.path.clone(),
            FileHunks {
                context: context.hunks,
                changes: changes.hunks,
            },
        );
    }
    out
}

/// Blocking (git, files): which of `drafts` are held back now, by id: for a right-side draft,
/// whether a commit since the draft's changed its lines (its commit's file against HEAD's; local
/// edits don't hold a draft back), else the file on disk; the merge base's blob for a left-side one.
/// (A draft outside the diff ([`Draft::outside_ok`]) is judged where the rows are built, so it
/// holds for live text too.)
pub fn check(
    repo: &Repo,
    merge_base: Option<&str>,
    files: &[FileChange],
    drafts: &[Draft],
) -> Vec<(String, Option<String>)> {
    let git = repo.open().ok();
    drafts
        .iter()
        .map(|d| {
            let text = if d.right {
                std::fs::read(repo.workdir.join(&d.path)).ok()
            } else {
                let old = files
                    .iter()
                    .find(|f| f.path == d.path)
                    .and_then(|f| f.old_path.as_deref())
                    .unwrap_or(&d.path);
                git.as_ref().zip(merge_base).and_then(|(git, base)| {
                    let commit = git.find_commit(git2::Oid::from_str(base).ok()?).ok()?;
                    crate::review_anchor::blob_at(git, &commit, old)
                })
            };
            // The draft's commit against HEAD: only a commit changing its lines holds it back.
            let compared = (d.right && d.reply_to.is_none())
                .then(|| {
                    let git = git.as_ref()?;
                    let commit = git.find_commit(git2::Oid::from_str(&d.commit).ok()?).ok()?;
                    let anchored = crate::review_anchor::blob_at(git, &commit, &d.path)?;
                    let head = crate::review_anchor::head_blob(git, &d.path);
                    Some(head.and_then(|head| crate::review_anchor::Compared::new(&anchored, &head)))
                })
                .flatten();
            // A draft found on the forge whose commit isn't here has no lines to compare.
            let unread = d.reply_to.is_none() && d.lines.is_empty() && d.forge.is_some();
            let held = match compared {
                _ if unread => Some(COMMIT_MISSING),
                Some(Some(c)) => match c.place(Side::Right, d.start, d.end) {
                    crate::review_anchor::Placement::Placed { .. } => None,
                    _ => Some(d.changed()),
                },
                Some(None) => Some(FILE_CHANGED),
                None => d.drift_in(text.as_deref()),
            };
            (d.id.clone(), held.map(String::from))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::{Logs, Tool};

    fn draft(path: &str, start: u32, end: u32, lines: &[&str], body: &str) -> Draft {
        Draft::new(
            path.into(),
            true,
            start,
            end,
            "c0".into(),
            lines.iter().map(|l| l.to_string()).collect(),
            body.into(),
        )
    }

    fn repo(host: &str) -> RepoRef {
        RepoRef {
            host: host.into(),
            path: "alice/project".into(),
        }
    }

    #[test]
    fn drafts_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = DraftStore::default();
        let key = DraftStore::key(&repo("git.example.com"), 7);
        store.add(&key, draft("a.txt", 2, 3, &["x", "y"], "Why?"));
        store.save(dir.path(), DraftStore::next_seq()).unwrap();
        let log = Logs::default().tool(Tool::Review);
        assert_eq!(DraftStore::load(dir.path(), &log), store);
    }

    #[test]
    fn a_summary_round_trips_and_an_old_file_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let log = Logs::default().tool(Tool::Review);
        let mut store = DraftStore::default();
        store.add("k", draft("a.txt", 2, 3, &["x"], "Why?"));
        store.save(dir.path(), DraftStore::next_seq()).unwrap();
        // Without a summary the file is still the bare map an older jed reads.
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(serde_json::from_str::<BTreeMap<String, Vec<Draft>>>(&text).is_ok());
        assert_eq!(DraftStore::load(dir.path(), &log).summary("k"), "");
        store.set_summary("k", "Looks good overall");
        store.save(dir.path(), DraftStore::next_seq()).unwrap();
        let back = DraftStore::load(dir.path(), &log);
        assert_eq!((back.summary("k"), back.get("k").len()), ("Looks good overall", 1));
        assert_eq!(back, store);
        let mut gone = back;
        gone.set_summary("k", "");
        assert_eq!(gone.summary("k"), "");
        // Removed again: the file is the bare map once more; a summary of another request is apart.
        gone.set_summary("other", "x");
        assert_eq!(gone.summary("k"), "");
        gone.set_summary("other", "");
        gone.save(dir.path(), DraftStore::next_seq()).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(serde_json::from_str::<BTreeMap<String, Vec<Draft>>>(&text).is_ok());
        // merge keeps a summary the other store has and this one lacks, never overwriting.
        let mut a = DraftStore::default();
        a.set_summary("k", "mine");
        let mut b = DraftStore::default();
        b.set_summary("k", "theirs");
        b.set_summary("j", "j's");
        a.merge(b);
        assert_eq!((a.summary("k"), a.summary("j")), ("mine", "j's"));
    }

    fn pending_comment(id: &str, body: &str, line: u32) -> crate::forge::PendingComment {
        crate::forge::PendingComment {
            id: id.into(),
            reply_to: None,
            anchor: Some(Anchor {
                path: "a.txt".into(),
                side: Side::Right,
                start_line: line,
                end_line: line,
                commit: "c1".into(),
            }),
            body: body.into(),
            resolve: false,
        }
    }

    fn pending(comments: Vec<crate::forge::PendingComment>, lines: &[(&str, Option<&str>)]) -> crate::review::Pending {
        crate::review::Pending {
            review: crate::forge::PendingReview {
                id: "5".into(),
                comments,
            },
            lines: lines
                .iter()
                .map(|(id, l)| (id.to_string(), l.map(|l| vec![l.to_string()])))
                .collect(),
        }
    }

    fn on_forge(id: &str, body: &str) -> Draft {
        let mut d = draft("a.txt", 1, 1, &["x"], body);
        d.forge = Some(ForgeDraft {
            review_id: "5".into(),
            comment_id: id.into(),
        });
        d
    }

    #[test]
    fn reconcile_adds_drops_and_updates_by_forge_id() {
        let mut store = DraftStore::default();
        store.add("k", on_forge("1", "old text"));
        store.add("k", on_forge("2", "submitted elsewhere"));
        store.add("k", draft("a.txt", 4, 4, &["local"], "never pushed"));
        let p = pending(
            vec![
                pending_comment("1", "new text", 1),
                pending_comment("3", "from the web", 7),
            ],
            &[("1", Some("x")), ("3", Some("seven"))],
        );
        let r = store.reconcile("k", &p, &[], &|_| false);
        assert_eq!((r.added, r.changed, r.dropped.len()), (1, 1, 1));
        let bodies: Vec<_> = store.get("k").iter().map(|d| d.body.as_str()).collect();
        assert_eq!(bodies, ["new text", "never pushed", "from the web"]);
        let web = &store.get("k")[2];
        assert_eq!(
            (web.start, web.lines.clone(), web.by_claude),
            (7, vec!["seven".to_string()], false)
        );
        assert_eq!(web.forge.as_ref().unwrap().comment_id, "3");
        assert_eq!(web.commit, "c1");
        // Again: nothing changes.
        assert_eq!(store.reconcile("k", &p, &[], &|_| false), Reconciled::default());
    }

    #[test]
    fn reconcile_leaves_a_busy_draft_alone_and_drops_everything_when_nothing_is_pending() {
        let mut store = DraftStore::default();
        let mut busy = on_forge("1", "mine, being edited");
        busy.id = "busy".into();
        store.add("k", busy);
        let p = pending(vec![pending_comment("1", "forge text", 1)], &[]);
        store.reconcile("k", &p, &[], &|id| id == "busy");
        assert_eq!(store.get("k")[0].body, "mine, being edited");
        let r = store.reconcile("k", &pending(Vec::new(), &[]), &[], &|id| id == "busy");
        assert!(r.dropped.is_empty(), "a draft with a write on its way stays");
        let r = store.reconcile("k", &pending(Vec::new(), &[]), &[], &|_| false);
        assert_eq!(r.dropped.len(), 1);
        assert!(store.get("k").is_empty());
    }

    #[test]
    fn reconcile_without_the_commit_has_no_lines_until_it_can_read_them() {
        let mut store = DraftStore::default();
        let p = pending(vec![pending_comment("9", "web", 2)], &[("9", None)]);
        store.reconcile("k", &p, &[], &|_| false);
        assert!(store.get("k")[0].lines.is_empty());
        let p = pending(vec![pending_comment("9", "web", 2)], &[("9", Some("two"))]);
        store.reconcile("k", &p, &[], &|_| false);
        assert_eq!(store.get("k")[0].lines, vec!["two".to_string()]);
    }

    #[test]
    fn reconcile_makes_a_reply_draft_in_its_thread_by_thread_or_comment_id() {
        use crate::forge::{Comment, ReplyTo, Thread};
        let comment = |id: &str| Comment {
            id: id.into(),
            author: "bob".into(),
            body: "q".into(),
            created_at: String::new(),
            updated_at: String::new(),
            url: String::new(),
        };
        let row = crate::review::ThreadRow {
            thread: Thread {
                id: "T1".into(),
                anchor: Anchor {
                    path: "a.txt".into(),
                    side: Side::Right,
                    start_line: 2,
                    end_line: 3,
                    commit: "c1".into(),
                },
                resolved: false,
                outdated: false,
                comments: vec![comment("501")],
                hunk: None,
            },
            place: crate::review_anchor::Placement::Missing,
            code: None,
            behind: false,
        };
        let reply = |id: &str, to: ReplyTo| {
            let mut c = pending_comment(id, "answer", 1);
            c.anchor = None;
            c.reply_to = Some(to);
            c.resolve = true;
            c
        };
        for to in [ReplyTo::Thread("T1".into()), ReplyTo::Comment("501".into())] {
            let mut store = DraftStore::default();
            let p = pending(vec![reply("r1", to)], &[]);
            let r = store.reconcile("k", &p, std::slice::from_ref(&row), &|_| false);
            assert_eq!(r.added, 1);
            let d = &store.get("k")[0];
            assert_eq!(
                (d.reply_to.as_deref(), d.resolve, d.start, d.end),
                (Some("T1"), true, 2, 3)
            );
        }
        // A reply to a thread that isn't known is skipped.
        let mut store = DraftStore::default();
        let p = pending(vec![reply("r2", ReplyTo::Thread("nope".into()))], &[]);
        assert_eq!(store.reconcile("k", &p, &[row], &|_| false).added, 0);
    }

    #[test]
    fn drafts_are_kept_per_host_repository_and_request() {
        let mut store = DraftStore::default();
        let a = DraftStore::key(&repo("git.example.com"), 7);
        let b = DraftStore::key(&repo("git.example.com"), 8);
        let c = DraftStore::key(&repo("other.example.com"), 7);
        store.add(&a, draft("a.txt", 1, 1, &["x"], "one"));
        store.add(&b, draft("a.txt", 1, 1, &["x"], "two"));
        assert_eq!(
            (store.get(&a).len(), store.get(&b).len(), store.get(&c).len()),
            (1, 1, 0)
        );
        let id = store.get(&a)[0].id.clone();
        assert!(store.set_body(&a, &id, "changed".into()));
        assert_eq!(store.get(&a)[0].body, "changed");
        assert!(store.remove(&a, &id).is_some());
        assert!(store.get(&a).is_empty() && !store.is_empty());
    }

    #[test]
    fn an_old_store_without_reply_fields_loads_and_a_reply_draft_is_never_held_back() {
        let dir = tempfile::tempdir().unwrap();
        let old = r#"{"k":[{"id":"a","path":"p.rs","right":true,"start":1,"end":1,"commit":"c","lines":["x"],"body":"b","created":1}]}"#;
        std::fs::write(dir.path().join(FILE), old).unwrap();
        let logs = Logs::default();
        let store = DraftStore::load(dir.path(), &logs.tool(Tool::Review));
        let d = &store.get("k")[0];
        assert_eq!((d.reply_to.clone(), d.resolve, d.by_claude), (None, false, false));
        assert_eq!(d.forge, None);
        let mut r = Draft::reply("p.rs".into(), true, (3, 3), "t1".into(), "ok".into(), true);
        r.by_claude = true;
        assert_eq!(r.drift_in(None), None);
        assert_eq!(r.drift_from(0, |_| None), None);
        let mut store = store;
        assert!(!store.set_reply("k", r.clone()));
        r.body = "again".into();
        assert!(store.set_reply("k", r));
        assert_eq!(store.get("k").len(), 2);
        assert_eq!(store.reply_for("k", "t1").unwrap().body, "again");
    }

    #[test]
    fn forge_ids_are_kept_with_the_draft_and_round_trip() {
        let mut store = DraftStore::default();
        store.add(
            "k",
            Draft::new("p.rs".into(), true, 1, 1, "c".into(), vec![], "b".into()),
        );
        let id = store.get("k")[0].id.clone();
        let r = crate::forge::DraftRef {
            review_id: "5".into(),
            comment_id: "99".into(),
        };
        assert!(store.set_forge("k", &id, Some(ForgeDraft::of(&r))));
        assert!(!store.set_forge("k", "nope", None));
        let dir = tempfile::tempdir().unwrap();
        store.save(dir.path(), DraftStore::next_seq()).unwrap();
        let back = DraftStore::load(dir.path(), &Logs::default().tool(Tool::Review));
        assert_eq!(back.find("k", &id).unwrap().forge.as_ref().unwrap().to_ref(), r);
    }

    #[test]
    fn a_corrupt_file_is_ignored_and_said_in_the_log() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "{ not json").unwrap();
        let logs = Logs::default();
        let store = DraftStore::load(dir.path(), &logs.tool(Tool::Review));
        assert!(store.is_empty());
        assert!(logs.texts(&Tool::Review).iter().any(|l| l.contains("unreadable")));
        // Absent: nothing to say.
        let empty = tempfile::tempdir().unwrap();
        let logs = Logs::default();
        assert!(DraftStore::load(empty.path(), &logs.tool(Tool::Review)).is_empty());
        assert!(logs.texts(&Tool::Review).is_empty());
    }

    #[test]
    fn an_older_write_never_replaces_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (DraftStore::next_seq(), DraftStore::next_seq());
        let mut a = DraftStore::default();
        a.add("k", draft("a.txt", 1, 1, &["x"], "old"));
        let mut b = DraftStore::default();
        b.add("k", draft("a.txt", 1, 1, &["x"], "new"));
        b.save(dir.path(), new).unwrap();
        a.save(dir.path(), old).unwrap();
        let log = Logs::default().tool(Tool::Review);
        assert_eq!(DraftStore::load(dir.path(), &log).get("k")[0].body, "new");
    }

    #[test]
    fn merge_adds_only_the_drafts_it_lacks() {
        let mut mine = DraftStore::default();
        let d = draft("a.txt", 1, 1, &["x"], "one");
        mine.add("k", d.clone());
        let mut theirs = DraftStore::default();
        theirs.add("k", d);
        theirs.add("k", draft("b.txt", 1, 1, &["y"], "two"));
        mine.merge(theirs);
        assert_eq!(mine.get("k").len(), 2);
    }

    #[test]
    fn drift_compares_the_remembered_lines() {
        let d = draft("a.txt", 2, 3, &["two", "three"], "b");
        assert_eq!(d.drift_in(Some(b"one\ntwo\nthree\nfour\n")), None);
        assert_eq!(
            d.drift_in(Some(b"one\ntwo\r\nthree\r\n")),
            None,
            "line ends don't count"
        );
        assert_eq!(d.drift_in(Some(b"one\ntwo\nTHREE\n")), Some(LINES_CHANGED));
        assert_eq!(
            d.drift_in(Some(b"zero\none\ntwo\nthree\n")),
            Some(LINES_CHANGED),
            "moved down"
        );
        assert_eq!(d.drift_in(None), Some(FILE_CHANGED));
        let one = draft("a.txt", 1, 1, &["one"], "b");
        assert_eq!(one.drift_in(Some(b"ONE\n")), Some(LINE_CHANGED));
        assert_eq!(one.drift_in(Some(b"")), Some(LINE_CHANGED));
    }

    #[test]
    fn odd_text_and_unknown_fields_survive_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = Logs::default().tool(Tool::Review);
        let body = "a\r\nb\t\u{1}\u{7f} \"q\" \\ é 日本\n".repeat(20_000);
        let mut store = DraftStore::default();
        store.add("k", draft("a.txt", 1, 1, &["x\ty"], &body));
        store.save(dir.path(), DraftStore::next_seq()).unwrap();
        assert_eq!(DraftStore::load(dir.path(), &log), store);
        // A file from a newer version with more fields still reads.
        let one = serde_json::to_string(&store.get("k")[0]).unwrap();
        let newer = one.replacen('{', "{\"future\":[1,2],", 1);
        std::fs::write(dir.path().join(FILE), format!("{{\"k\":[{newer}]}}")).unwrap();
        assert_eq!(DraftStore::load(dir.path(), &log), store);
    }

    #[test]
    fn the_anchor_is_one_based_on_the_drafts_side() {
        let mut d = draft("a.txt", 4, 6, &["a", "b", "c"], "b");
        let a = d.anchor();
        assert_eq!(
            (a.start_line, a.end_line, a.side, a.commit.as_str()),
            (4, 6, Side::Right, "c0")
        );
        d.right = false;
        assert_eq!(d.anchor().side, Side::Left);
        assert_eq!(d.place(), "a.txt:4");
    }

    /// Ten lines; line 5 changed, lines 8-9 replaced by three new ones.
    fn hunks() -> FileHunks {
        let old: Vec<String> = (1..=10).map(|i| format!("l{i}\n")).collect();
        let mut new = old.clone();
        new[4] = "five\n".into();
        new.splice(7..9, ["a\n".into(), "b\n".into(), "c\n".into()]);
        let (old, new) = (old.concat(), new.concat());
        let d = |c| crate::git::diff::diff(old.as_bytes(), new.as_bytes(), c).unwrap().hunks;
        FileHunks {
            context: d(3),
            changes: d(0),
        }
    }

    #[test]
    fn lines_inside_one_context_hunk_are_in_the_diff() {
        let h = hunks();
        assert!(h.in_diff(true, 5, 5));
        assert!(h.in_diff(true, 2, 11));
        assert!(!h.in_diff(true, 1, 1));
        assert!(!h.in_diff(true, 1, 5), "a range must sit in one hunk");
        assert!(!h.in_diff(true, 12, 12));
    }

    #[test]
    fn the_diff_has_no_gap_in_a_file_of_few_lines() {
        let h = FileHunks {
            context: crate::git::diff::diff(b"a\nb\nc\n", b"a\nB\nc\n", 3).unwrap().hunks,
            changes: Vec::new(),
        };
        assert!(h.in_diff(true, 1, 3) && h.in_diff(false, 1, 3));
    }

    #[test]
    fn an_unchanged_line_knows_the_other_sides_number() {
        let h = hunks();
        assert_eq!(h.counterpart(true, 2), Some(2));
        assert_eq!(h.counterpart(true, 5), None, "changed");
        assert_eq!(h.counterpart(true, 7), Some(7));
        assert_eq!(h.counterpart(true, 8), None);
        // Old line 10 follows the replaced 8-9 (three new lines): new line 11.
        assert_eq!(h.counterpart(false, 10), Some(11));
        assert_eq!(h.counterpart(true, 11), Some(10));
        assert_eq!(h.counterpart(false, 9), None);
    }

    fn file_of(old: &str, new: &str) -> FileHunks {
        let d = |c| crate::git::diff::diff(old.as_bytes(), new.as_bytes(), c).unwrap().hunks;
        FileHunks {
            context: d(3),
            changes: d(0),
        }
    }

    fn lines(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("l{i}\n")).collect()
    }

    #[test]
    fn the_hunk_edges_keep_the_last_context_line_and_hold_the_next() {
        let old = lines(30);
        let mut new = old.clone();
        new[9] = "ten\n".into(); // line 10: context is lines 7..=13
        let h = file_of(&old.concat(), &new.concat());
        assert!(h.in_diff(true, 7, 7), "first context line");
        assert!(h.in_diff(true, 13, 13), "last context line");
        assert!(!h.in_diff(true, 6, 6));
        assert!(!h.in_diff(true, 14, 14));
        assert!(h.in_diff(true, 7, 13));
        assert!(!h.in_diff(true, 13, 14), "straddles the end");
        assert!(!h.in_diff(true, 6, 8), "straddles the start");
        assert!(h.in_diff(false, 7, 13) && !h.in_diff(false, 14, 14));
    }

    #[test]
    fn the_left_side_is_judged_against_the_old_lines() {
        let old = lines(30);
        let mut new = old.clone();
        new.drain(0..5); // old 1-5 deleted
        let h = file_of(&old.concat(), &new.concat());
        // Old context 1..=8; the new side has only 3 context lines (1..=3).
        assert!(h.in_diff(false, 1, 8) && !h.in_diff(false, 9, 9));
        assert!(h.in_diff(true, 1, 3) && !h.in_diff(true, 4, 4));
    }

    #[test]
    fn counterparts_follow_insertions_and_deletions() {
        let old = lines(12);
        let mut new = old.clone();
        new.splice(3..3, ["x\n".to_string(), "y\n".to_string()]); // 2 inserted before old 4
        new.remove(new.len() - 3); // old 10 deleted
        let h = file_of(&old.concat(), &new.concat());
        assert_eq!(h.counterpart(true, 3), Some(3), "before the first hunk");
        assert_eq!(h.counterpart(true, 4), None, "inserted");
        assert_eq!(h.counterpart(true, 6), Some(4), "between the hunks");
        assert_eq!(h.counterpart(false, 4), Some(6));
        assert_eq!(h.counterpart(false, 10), None, "deleted");
        assert_eq!(h.counterpart(false, 11), Some(12), "after both hunks");
        assert_eq!(h.counterpart(true, 12), Some(11));
        assert_eq!(h.counterpart(true, 10), Some(8));
    }

    #[test]
    fn a_reply_is_never_held() {
        let mut map = BTreeMap::new();
        map.insert("a.txt".to_string(), hunks());
        let mut d = draft("a.txt", 1, 1, &["l1"], "x");
        d.reply_to = Some("t1".into());
        assert!(d.outside_ok(Kind::GitHub, &map));
    }

    #[test]
    fn only_github_holds_a_draft_outside_the_diff() {
        let mut map = BTreeMap::new();
        map.insert("a.txt".to_string(), hunks());
        let outside = draft("a.txt", 1, 1, &["l1"], "x");
        let inside = draft("a.txt", 5, 5, &["five"], "x");
        assert!(!outside.outside_ok(Kind::GitHub, &map));
        assert!(inside.outside_ok(Kind::GitHub, &map));
        assert!(outside.outside_ok(Kind::Gitea, &map));
        assert!(outside.outside_ok(Kind::GitLab, &map));
        // A file without known hunks passes.
        assert!(draft("b.txt", 1, 1, &["x"], "x").outside_ok(Kind::GitHub, &map));
    }
}
