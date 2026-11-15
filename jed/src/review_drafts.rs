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

use crate::forge::{Anchor, RepoRef, Side};
use crate::git::Repo;
use crate::git::range::FileChange;
use crate::logs::ToolLog;

const FILE: &str = "review-drafts.json";

/// Why a draft is held back.
pub const LINE_CHANGED: &str = "line changed";
pub const LINES_CHANGED: &str = "lines changed";
pub const FILE_CHANGED: &str = "file changed";

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
    #[allow(dead_code)] // consumed by the submit subtask
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

    /// Why the draft can't be placed where it was written, given the lines `line` (0-based, on
    /// the draft's side) reads now; `None`: unchanged.
    pub fn drift<'a>(&self, line: impl Fn(usize) -> Option<&'a str>) -> Option<&'static str> {
        if self.reply_to.is_some() {
            return None;
        }
        let first = (self.start.max(1) - 1) as usize;
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

    /// `drift` against a whole file's bytes (`None`: the file is gone).
    pub fn drift_in(&self, text: Option<&[u8]>) -> Option<&'static str> {
        if self.reply_to.is_some() {
            return None;
        }
        let Some(text) = text else { return Some(FILE_CHANGED) };
        let text = String::from_utf8_lossy(text);
        let lines: Vec<&str> = text.lines().collect();
        self.drift(|i| lines.get(i).copied())
    }
}

/// A draft with where it stands now: what `Submit Review` will consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow<'a> {
    pub draft: &'a Draft,
    /// Why it is held back (not posted): `None` when its lines are unchanged.
    pub held: Option<&'a str>,
}

/// Every merge request's drafts, by [`DraftStore::key`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftStore {
    map: BTreeMap<String, Vec<Draft>>,
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
        self.map.is_empty()
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
        for (key, list) in other.map {
            for d in list {
                if !self.get(&key).iter().any(|e| e.id == d.id) {
                    self.add(&key, d);
                }
            }
        }
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
            Ok(map) => DraftStore { map },
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
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.map)?)?;
        std::fs::rename(tmp, dir.join(FILE))?;
        written.insert(dir.to_path_buf(), seq);
        Ok(())
    }
}

/// Blocking (git, files): which of `drafts` are held back now, by id: the file on disk for a
/// right-side draft, the merge base's blob for a left-side one.
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
            (d.id.clone(), d.drift_in(text.as_deref()).map(String::from))
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
        let mut r = Draft::reply("p.rs".into(), true, (3, 3), "t1".into(), "ok".into(), true);
        r.by_claude = true;
        assert_eq!(r.drift_in(None), None);
        assert_eq!(r.drift(|_| None), None);
        let mut store = store;
        assert!(!store.set_reply("k", r.clone()));
        r.body = "again".into();
        assert!(store.set_reply("k", r));
        assert_eq!(store.get("k").len(), 2);
        assert_eq!(store.reply_for("k", "t1").unwrap().body, "again");
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
}
