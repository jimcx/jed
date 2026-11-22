//! `git blame` of a file at HEAD as a resumable walk, so inline blame can show the newest lines'
//! commits at once and the oldest later (no drawing, no app state; blocking: opens the
//! repository with `Repo::open`). libgit2's `blame_file` waits for the whole file and is 15-20x
//! slower than `git blame`, which is why this is our own.
//!
//! The walk is git's blame scoreboard without `-M` / `-C` (`blame.c`'s `pass_blame`). Each queued
//! commit holds [`Run`]s: the HEAD lines it is still suspected of, as line ranges of its own
//! version of the file. Commits come off a heap by committer date, newest first (equal dates
//! keep the order they were queued in, as in [`super::log::Walker`]); a commit queued again after
//! it was taken (a later child passed it more lines) is taken again. For the commit taken:
//!
//! - a parent whose version of the file has the same blob id takes every suspect, with no diff
//!   (the first such parent; all parents are checked before any is diffed, as git does);
//! - otherwise each parent in order takes the suspects its version shares with this one (the
//!   unchanged runs of `diff::diff(parent, ours, 0)`); a parent without the file takes none;
//! - what is left is this commit's: settled. A root commit keeps all its suspects.
//!
//! A parent that lacks the path is looked up for a rename source, as `git blame` and `git log
//! --follow` do ([`super::log::renamed_from`]), and the suspects continue under the old name.
//!
//! [`Walk::step`] walks until a time slice runs out or the walk ends and returns the HEAD lines
//! settled since the last step. [`stream`] is the job on `spawn_blocking` that sends them on in
//! chunks.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use git2::{ErrorCode, Oid, Repository};

use super::log::{Entries, FLUSH, SLICE, renamed_from};
use super::{Repo, diff};
use crate::blame::{self, Commit};

/// How many blobs a walk keeps.
const BLOBS: usize = 8;

/// HEAD lines `line..line + count`, which are lines `own..own + count` of the version of the file
/// the run's commit holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    line: usize,
    own: usize,
    count: usize,
}

/// A queued commit's suspects, and the name its version of the file has.
struct Pending {
    path: PathBuf,
    /// Sorted by `own`.
    runs: Vec<Run>,
}

/// HEAD lines `line..line + count` last changed by `commit`; they are lines `orig..orig + count`
/// of the file as that commit has it (what `git blame --porcelain` calls the original line).
#[derive(Debug, Clone)]
pub struct Settled {
    pub line: usize,
    pub count: usize,
    #[cfg_attr(not(test), allow(dead_code))]
    pub orig: usize,
    pub commit: Arc<Commit>,
}

/// What one [`Walk::step`] settled.
#[derive(Debug, Clone, Default)]
pub struct Step {
    /// The HEAD lines settled since the last step.
    pub settled: Vec<Settled>,
    /// Every line is settled: no further step finds anything.
    pub done: bool,
}

/// A resumable blame of a file at HEAD. Blocking: [`Self::new`] and [`Self::step`] read objects,
/// so call both from `spawn_blocking`.
pub struct Walk {
    repo: Repository,
    head: Oid,
    text: Vec<u8>,
    email: Option<String>,
    /// `(commit time, Reverse(queue order), id)`: newest first.
    heap: BinaryHeap<(i64, Reverse<u64>, Oid)>,
    seq: u64,
    pending: HashMap<Oid, Pending>,
    entries: Entries,
    /// Each commit's metadata, read once.
    described: HashMap<Oid, Arc<Commit>>,
    /// The last blobs read, oldest first.
    blobs: RefCell<Vec<(Oid, Arc<Vec<u8>>)>>,
    slice: Duration,
    visited: u64,
}

impl Walk {
    /// Blocking: a walk of `path` (work-tree relative) at HEAD. `None`: no commits yet, or the
    /// path is not a file in HEAD.
    pub fn new(repo: &Repo, path: &Path) -> Result<Option<Walk>, git2::Error> {
        let r = repo.open()?;
        let (tip, time) = match r.head().and_then(|h| h.peel_to_commit()) {
            Ok(c) => (c.id(), c.time().seconds()),
            Err(e) if matches!(e.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let entry = match r.find_commit(tip)?.tree()?.get_path(path) {
            Ok(e) => e,
            Err(e) if e.code() == ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        // A folder or a submodule is no file.
        let Ok(blob) = r.find_blob(entry.id()) else {
            return Ok(None);
        };
        let text = blob.content().to_vec();
        drop(blob);
        let email = blame::user_email(&r);
        let mut walk = Walk {
            head: tip,
            email,
            heap: BinaryHeap::new(),
            seq: 0,
            pending: HashMap::new(),
            entries: Entries::default(),
            described: HashMap::new(),
            blobs: RefCell::default(),
            slice: SLICE,
            visited: 0,
            text,
            repo: r,
        };
        let count = diff::lines(&walk.text).len();
        if count > 0 {
            let runs = vec![Run { line: 0, own: 0, count }];
            walk.give(tip, time, path.to_path_buf(), runs);
        }
        Ok(Some(walk))
    }

    /// The longest one [`Self::step`] runs before returning what it has (at least one commit is
    /// always taken).
    pub fn with_slice(mut self, slice: Duration) -> Walk {
        self.slice = slice;
        self
    }

    /// HEAD's commit id.
    pub fn head(&self) -> Oid {
        self.head
    }

    /// The file's text at HEAD.
    pub fn text(&self) -> &[u8] {
        &self.text
    }

    /// How many commits the walk has taken off its queue so far.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn visited(&self) -> u64 {
        self.visited
    }

    /// Blocking: walks on until the time slice runs out or no suspects are left, and returns the
    /// lines settled meanwhile. Resumes where the last step stopped.
    pub fn step(&mut self) -> Result<Step, git2::Error> {
        let start = Instant::now();
        let mut settled = Vec::new();
        while let Some((_, _, id)) = self.heap.pop() {
            // Taken already under an earlier entry (it was queued again in between).
            let Some(pending) = self.pending.remove(&id) else {
                continue;
            };
            self.visited += 1;
            self.take(id, pending, &mut settled)?;
            if start.elapsed() >= self.slice {
                break;
            }
        }
        Ok(Step {
            settled,
            done: self.heap.is_empty(),
        })
    }

    /// Passes one commit's suspects on to its parents and settles what no parent shares.
    fn take(&mut self, id: Oid, pending: Pending, settled: &mut Vec<Settled>) -> Result<(), git2::Error> {
        let (gives, rest) = self.plan(id, &pending.path, pending.runs)?;
        for (parent, time, name, runs) in gives {
            self.give(parent, time, name, runs);
        }
        self.settle(id, rest, settled)
    }

    /// Which suspects of commit `id` (the file under `path`) each parent takes (its id, commit
    /// time, the file's name there and the runs, as lines of its version), and the ones left.
    #[allow(clippy::type_complexity)]
    fn plan(
        &self,
        id: Oid,
        path: &Path,
        runs: Vec<Run>,
    ) -> Result<(Vec<(Oid, i64, PathBuf, Vec<Run>)>, Vec<Run>), git2::Error> {
        let commit = self.repo.find_commit(id)?;
        let Some((_, blob)) = self.entries.get(&commit, path)? else {
            return Ok((Vec::new(), runs));
        };
        // Each parent's version of the file: its name there and blob.
        let mut origins: Vec<(Oid, i64, PathBuf, Oid)> = Vec::new();
        for parent in commit.parents() {
            let mut name = path.to_path_buf();
            let mut entry = self.entries.get(&parent, path)?;
            if entry.is_none()
                && let Some(old) = renamed_from(&self.repo, &parent, &commit, path)?
            {
                entry = self.entries.get(&parent, &old)?;
                name = old;
            }
            // A folder where the file was has no version.
            if let Some((mode, blob)) = entry
                && mode != 0o040000
            {
                origins.push((parent.id(), parent.time().seconds(), name, blob));
            }
        }
        if let Some((parent, time, name, _)) = origins.iter().find(|o| o.3 == blob) {
            return Ok((vec![(*parent, *time, name.clone(), runs)], Vec::new()));
        }
        let mut gives = Vec::new();
        let mut rest = runs;
        if !origins.is_empty() && !rest.is_empty() {
            let ours = self.blob(blob)?;
            for (parent, time, name, parent_blob) in origins {
                let theirs = self.blob(parent_blob)?;
                let d = diff::diff(&theirs, &ours, 0)?;
                if d.binary {
                    continue;
                }
                let (taken, left) = split(&rest, &d.hunks);
                rest = left;
                if !taken.is_empty() {
                    gives.push((parent, time, name, taken));
                }
                if rest.is_empty() {
                    break;
                }
            }
        }
        Ok((gives, rest))
    }

    /// A blob's content: a parent read for a diff is this commit's version when its turn comes,
    /// so the last few are kept.
    fn blob(&self, id: Oid) -> Result<Arc<Vec<u8>>, git2::Error> {
        let mut recent = self.blobs.borrow_mut();
        if let Some((_, b)) = recent.iter().find(|(i, _)| *i == id) {
            return Ok(b.clone());
        }
        let b = Arc::new(self.repo.find_blob(id)?.content().to_vec());
        if recent.len() >= BLOBS {
            recent.remove(0);
        }
        recent.push((id, b.clone()));
        Ok(b)
    }

    /// Queues `runs` as suspects of `id` (committed at `time`, under `path`).
    fn give(&mut self, id: Oid, time: i64, path: PathBuf, runs: Vec<Run>) {
        match self.pending.get_mut(&id) {
            Some(p) => {
                p.runs.extend(runs);
                p.runs.sort_by_key(|r| r.own);
                let mut merged: Vec<Run> = Vec::with_capacity(p.runs.len());
                for run in p.runs.drain(..) {
                    push(&mut merged, run);
                }
                p.runs = merged;
            }
            None => {
                self.pending.insert(id, Pending { path, runs });
                self.heap.push((time, Reverse(self.seq), id));
                self.seq += 1;
            }
        }
    }

    /// Settles `runs` on commit `id`.
    fn settle(&mut self, id: Oid, runs: Vec<Run>, settled: &mut Vec<Settled>) -> Result<(), git2::Error> {
        if runs.is_empty() {
            return Ok(());
        }
        let commit = match self.described.get(&id) {
            Some(c) => c.clone(),
            None => {
                let c = blame::describe(&self.repo, id, self.email.as_deref())?;
                self.described.insert(id, c.clone());
                c
            }
        };
        settled.extend(runs.into_iter().map(|r| Settled {
            line: r.line,
            count: r.count,
            orig: r.own,
            commit: commit.clone(),
        }));
        Ok(())
    }
}

impl std::fmt::Debug for Walk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Walk")
            .field("head", &self.head)
            .field("visited", &self.visited)
            .finish_non_exhaustive()
    }
}

/// What the walk's job sends: the HEAD lines settled since the last chunk.
#[derive(Debug)]
pub struct Chunk {
    pub settled: Vec<Settled>,
    /// The walk is over (the last chunk, possibly with nothing in it).
    pub done: bool,
}

/// Blocking, for `spawn_blocking`: steps `walk` and sends what it settles as `send` chunks: the
/// first at once, then every [`FLUSH`], and the last when the walk ends. Ends on done, an error
/// (sent as `Err`), `cancel` (checked before every step and every send: nothing is sent after
/// it is set) or `send` returning false.
pub fn stream(mut walk: Walk, cancel: &AtomicBool, mut send: impl FnMut(Result<Chunk, String>) -> bool) {
    let mut pending: Vec<Settled> = Vec::new();
    let mut first = true;
    let mut flushed = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let step = match walk.step() {
            Ok(step) => step,
            Err(e) => {
                send(Err(e.message().to_string()));
                return;
            }
        };
        pending.extend(step.settled);
        let due = !pending.is_empty() && (first || flushed.elapsed() >= FLUSH);
        if step.done || due {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            first = false;
            flushed = Instant::now();
            let chunk = Chunk {
                settled: std::mem::take(&mut pending),
                done: step.done,
            };
            if !send(Ok(chunk)) || step.done {
                return;
            }
        }
    }
}

/// Appends `run`, joined to the last one when the two are one run.
fn push(runs: &mut Vec<Run>, run: Run) {
    if let Some(last) = runs.last_mut()
        && last.line + last.count == run.line
        && last.own + last.count == run.own
    {
        last.count += run.count;
        return;
    }
    runs.push(run);
}

/// Splits `runs` (sorted by `own`) by `hunks` (parent to ours, context 0): the parts in lines the
/// two versions share, as runs of the parent's version, and the parts in changed lines.
fn split(runs: &[Run], hunks: &[diff::Hunk]) -> (Vec<Run>, Vec<Run>) {
    // The shared stretches: our lines `start..end` are the parent's from `parent`.
    let mut shared: Vec<(usize, usize, usize)> = Vec::with_capacity(hunks.len() + 1);
    let (mut ours, mut theirs) = (0, 0);
    for h in hunks {
        if h.new.start > ours {
            shared.push((ours, h.new.start, theirs));
        }
        ours = h.new.end;
        theirs = h.old.end;
    }
    shared.push((ours, usize::MAX, theirs));
    let (mut taken, mut rest) = (Vec::new(), Vec::new());
    let mut i = 0;
    for run in runs {
        let (mut pos, end) = (run.own, run.own + run.count);
        while pos < end {
            while shared[i].1 <= pos {
                i += 1;
            }
            let (start, stop, parent) = shared[i];
            if pos < start {
                let to = end.min(start);
                push(
                    &mut rest,
                    Run {
                        line: run.line + (pos - run.own),
                        own: pos,
                        count: to - pos,
                    },
                );
                pos = to;
                continue;
            }
            let to = end.min(stop);
            push(
                &mut taken,
                Run {
                    line: run.line + (pos - run.own),
                    own: parent + (pos - start),
                    count: to - pos,
                },
            );
            pos = to;
        }
    }
    (taken, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Signature, Time};

    fn run(line: usize, own: usize, count: usize) -> Run {
        Run { line, own, count }
    }

    fn hunk(old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> diff::Hunk {
        diff::Hunk { old, new }
    }

    #[test]
    fn split_maps_shared_lines_to_the_parent() {
        // Ours lines 2..4 replace the parent's 2..3 (one line became two), a line was inserted
        // before our line 7.
        let hunks = [hunk(2..3, 2..4), hunk(6..6, 7..8)];
        let (taken, rest) = split(&[run(10, 0, 10)], &hunks);
        assert_eq!(taken, [run(10, 0, 2), run(14, 3, 3), run(18, 6, 2)]);
        assert_eq!(rest, [run(12, 2, 2), run(17, 7, 1)]);
    }

    #[test]
    fn split_handles_a_deletion_and_runs_past_the_last_hunk() {
        // Parent lines 1..3 were deleted: ours line 1 is the parent's 3.
        let (taken, rest) = split(&[run(0, 1, 2)], &[hunk(1..3, 1..1)]);
        assert_eq!(taken, [run(0, 3, 2)]);
        assert!(rest.is_empty());
    }

    // ---- fixtures: repositories built with git2 alone, with fixed names and times, so their
    // commit ids are the same on every run (the saved `git blame --porcelain` references name
    // them).

    /// A fixture repository: HEAD at `head`, the file at `path`.
    pub(super) struct Fixture {
        _dir: tempfile::TempDir,
        pub repo: Repo,
        pub path: &'static str,
        pub head: Oid,
    }

    struct Builder {
        dir: tempfile::TempDir,
        repo: Repository,
        n: i64,
    }

    impl Builder {
        fn new() -> Builder {
            let dir = tempfile::tempdir().unwrap();
            let repo = Repository::init(dir.path()).unwrap();
            Builder { dir, repo, n: 0 }
        }

        /// A commit of `parents` whose tree is `files` (top-level files): its time is 100 s
        /// after the previous commit's, its author Alice or Bob in turn.
        fn commit(&mut self, parents: &[Oid], files: &[(&str, &str)], msg: &str) -> Oid {
            let at = 100 * (self.n + 1);
            self.commit_at(at, parents, files, msg)
        }

        /// As [`Self::commit`], at `secs` after the fixtures' first second.
        fn commit_at(&mut self, secs: i64, parents: &[Oid], files: &[(&str, &str)], msg: &str) -> Oid {
            let mut tree = self.repo.treebuilder(None).unwrap();
            for (name, text) in files {
                let blob = self.repo.blob(text.as_bytes()).unwrap();
                tree.insert(name, blob, 0o100644).unwrap();
            }
            let tree = self.repo.find_tree(tree.write().unwrap()).unwrap();
            self.n += 1;
            let (name, email) = if self.n % 2 == 0 {
                ("Alice", "alice@example.com")
            } else {
                ("Bob", "bob@example.com")
            };
            let sig = Signature::new(name, email, &Time::new(1_600_000_000 + secs, 0)).unwrap();
            let parents: Vec<git2::Commit> = parents.iter().map(|p| self.repo.find_commit(*p).unwrap()).collect();
            let parents: Vec<&git2::Commit> = parents.iter().collect();
            self.repo.commit(None, &sig, &sig, msg, &tree, &parents).unwrap()
        }

        fn finish(self, head: Oid, path: &'static str) -> Fixture {
            self.repo.reference("refs/heads/main", head, true, "fixture").unwrap();
            self.repo.set_head("refs/heads/main").unwrap();
            let repo = Repo::discover(self.dir.path()).unwrap();
            Fixture {
                _dir: self.dir,
                repo,
                path,
                head,
            }
        }
    }

    fn numbered(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    /// Lines joined with a line break after each.
    fn join(lines: &[String]) -> String {
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    pub(super) const NAMES: &[&str] = &[
        "history",
        "merge_one_side",
        "merge_both",
        "rename",
        "moved",
        "readded",
        "empty",
        "no_final_newline",
        "skewed",
        "converge",
    ];

    pub(super) fn build(name: &str) -> Fixture {
        let mut b = Builder::new();
        let f = "f.txt";
        match name {
            // Lines changed, inserted and deleted over many commits.
            "history" => {
                let mut l = numbered(12);
                let mut tip = b.commit(&[], &[(f, &join(&l))], "create");
                l[2] = "line 3 v2".into();
                tip = b.commit(&[tip], &[(f, &join(&l))], "change line 3");
                l.insert(5, "new a".into());
                tip = b.commit(&[tip], &[(f, &join(&l))], "insert after 5");
                l.drain(8..10);
                tip = b.commit(&[tip], &[(f, &join(&l))], "delete two lines");
                l[0] = "line 1 v2".into();
                l[2] = "line 3 v3".into();
                tip = b.commit(&[tip], &[(f, &join(&l))], "change lines 1 and 3");
                l.extend(["tail 1".to_string(), "tail 2".to_string()]);
                tip = b.commit(&[tip], &[(f, &join(&l))], "append");
                l.splice(3..6, ["mid x".to_string(), "mid y".to_string()]);
                tip = b.commit(&[tip], &[(f, &join(&l))], "replace three with two");
                b.finish(tip, f)
            }
            // A merge where one side changed the file and the other didn't, with the changed
            // side first parent and then second.
            "merge_one_side" => {
                let mut l = numbered(10);
                let base = b.commit(&[], &[(f, &join(&l)), ("o.txt", "0\n")], "create");
                let unchanged = l.clone();
                l[3] = "line 4 v2".into();
                let changed = b.commit(&[base], &[(f, &join(&l)), ("o.txt", "0\n")], "change line 4");
                let other = b.commit(&[base], &[(f, &join(&unchanged)), ("o.txt", "1\n")], "other file");
                let m1 = b.commit(&[changed, other], &[(f, &join(&l)), ("o.txt", "1\n")], "merge 1");
                let c4 = b.commit(&[m1], &[(f, &join(&l)), ("o.txt", "2\n")], "other file again");
                l[8] = "line 9 v2".into();
                let c5 = b.commit(&[m1], &[(f, &join(&l)), ("o.txt", "1\n")], "change line 9");
                let m2 = b.commit(&[c4, c5], &[(f, &join(&l)), ("o.txt", "2\n")], "merge 2");
                b.finish(m2, f)
            }
            // A merge where both sides changed the file, in different places.
            "merge_both" => {
                let mut l = numbered(10);
                let base = b.commit(&[], &[(f, &join(&l))], "create");
                let mut l1 = l.clone();
                l1[1] = "line 2 left".into();
                let left = b.commit(&[base], &[(f, &join(&l1))], "left changes line 2");
                let mut l2 = l.clone();
                l2[7] = "line 8 right".into();
                let right = b.commit(&[base], &[(f, &join(&l2))], "right changes line 8");
                l[1] = "line 2 left".into();
                l[7] = "line 8 right".into();
                let m = b.commit(&[left, right], &[(f, &join(&l))], "merge");
                l[4] = "line 5 after".into();
                let tip = b.commit(&[m], &[(f, &join(&l))], "change line 5");
                b.finish(tip, f)
            }
            // A whole-file rename with the same content, and a later one with a small edit.
            "rename" => {
                let mut l = numbered(20);
                let mut tip = b.commit(&[], &[("a.txt", &join(&l))], "create a");
                l[4] = "line 5 v2".into();
                tip = b.commit(&[tip], &[("a.txt", &join(&l))], "change a");
                tip = b.commit(&[tip], &[("b.txt", &join(&l))], "rename a to b");
                l[9] = "line 10 v2".into();
                tip = b.commit(&[tip], &[("b.txt", &join(&l))], "change b");
                l[14] = "line 15 renamed".into();
                tip = b.commit(&[tip], &[("c.txt", &join(&l))], "rename b to c with an edit");
                l[1] = "line 2 v2".into();
                tip = b.commit(&[tip], &[("c.txt", &join(&l))], "change c");
                b.finish(tip, "c.txt")
            }
            // Lines moved within the file (no -M: they belong to the commit that moved them).
            "moved" => {
                let names = [
                    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
                ];
                let mut l: Vec<String> = names.iter().map(|s| s.to_string()).collect();
                let mut tip = b.commit(&[], &[(f, &join(&l))], "create");
                let moved: Vec<String> = l.drain(0..3).collect();
                l.extend(moved);
                tip = b.commit(&[tip], &[(f, &join(&l))], "move the top three to the bottom");
                let one = l.remove(5);
                l.insert(1, one);
                tip = b.commit(&[tip], &[(f, &join(&l))], "move one line up");
                b.finish(tip, f)
            }
            // The file deleted and added again with some of its old lines.
            "readded" => {
                let l = numbered(8);
                let mut tip = b.commit(&[], &[(f, &join(&l)), ("o.txt", "x\n")], "create");
                tip = b.commit(&[tip], &[("o.txt", "x\n")], "delete");
                tip = b.commit(&[tip], &[("o.txt", "y\n")], "other change");
                let mut again = l.clone();
                again[2] = "line 3 again".into();
                tip = b.commit(&[tip], &[(f, &join(&again)), ("o.txt", "y\n")], "add again");
                b.finish(tip, f)
            }
            // A file emptied: no lines to blame.
            "empty" => {
                let tip = b.commit(&[], &[(f, "")], "create empty");
                let tip = b.commit(&[tip], &[(f, "one\ntwo\n")], "fill");
                let tip = b.commit(&[tip], &[(f, "")], "empty again");
                b.finish(tip, f)
            }
            // A last line without a final line break, changed and extended.
            "no_final_newline" => {
                let tip = b.commit(&[], &[(f, "one\ntwo\nthree")], "create");
                let tip = b.commit(&[tip], &[(f, "one\ntwo\nthree!")], "change the last line");
                let tip = b.commit(&[tip], &[(f, "zero\none\ntwo\nthree!")], "add a line above");
                b.finish(tip, f)
            }
            // Commit times out of order: the root is dated after its descendants, so it comes
            // off the queue between its two children and is taken a second time.
            "skewed" => {
                let mut l = numbered(10);
                let base = b.commit_at(9000, &[], &[(f, &join(&l))], "create");
                let mut lx = l.clone();
                lx[4] = "line 5 x".into();
                let x = b.commit_at(100, &[base], &[(f, &join(&lx))], "x changes line 5");
                l[0] = "line 1 p".into();
                let p = b.commit_at(200, &[base], &[(f, &join(&l))], "p changes line 1");
                // The merge takes p's line 1, goes back to the base's line 5 and changes line 9.
                l[8] = "line 9 merged".into();
                let m = b.commit_at(300, &[x, p], &[(f, &join(&l))], "merge");
                b.finish(m, f)
            }
            // Two children pass lines to the same parent while it is still queued (dates in
            // order): the merge's first parent x takes most lines, p takes the ones x changed
            // and the merge undid, and both pass theirs on to the root.
            "converge" => {
                let mut l = numbered(10);
                let base = b.commit(&[], &[(f, &join(&l))], "create");
                let mut lx = l.clone();
                lx[4] = "line 5 x".into();
                let x = b.commit(&[base], &[(f, &join(&lx))], "x changes line 5");
                l[0] = "line 1 p".into();
                let p = b.commit(&[base], &[(f, &join(&l))], "p changes line 1");
                l[8] = "line 9 merged".into();
                let m = b.commit(&[x, p], &[(f, &join(&l))], "merge");
                b.finish(m, f)
            }
            _ => panic!("no fixture {name}"),
        }
    }

    /// The saved `git blame --porcelain` of each fixture.
    fn reference(name: &str) -> &'static str {
        match name {
            "history" => include_str!("../testdata/blame/history.porcelain"),
            "merge_one_side" => include_str!("../testdata/blame/merge_one_side.porcelain"),
            "merge_both" => include_str!("../testdata/blame/merge_both.porcelain"),
            "rename" => include_str!("../testdata/blame/rename.porcelain"),
            "moved" => include_str!("../testdata/blame/moved.porcelain"),
            "readded" => include_str!("../testdata/blame/readded.porcelain"),
            "empty" => include_str!("../testdata/blame/empty.porcelain"),
            "no_final_newline" => include_str!("../testdata/blame/no_final_newline.porcelain"),
            "skewed" => include_str!("../testdata/blame/skewed.porcelain"),
            "converge" => include_str!("../testdata/blame/converge.porcelain"),
            _ => panic!("no reference {name}"),
        }
    }

    /// Each final line's (commit id, original line, 1-based) from a porcelain blame: its header
    /// lines are `<id> <orig> <final> [<count>]`.
    fn parse(porcelain: &str) -> Vec<(String, usize)> {
        let mut lines: Vec<(String, usize)> = Vec::new();
        for l in porcelain.lines() {
            let f: Vec<&str> = l.split(' ').collect();
            if (f.len() == 3 || f.len() == 4)
                && f[0].len() == 40
                && f[0].bytes().all(|b| b.is_ascii_hexdigit())
                && let (Ok(orig), Ok(fin)) = (f[1].parse::<usize>(), f[2].parse::<usize>())
            {
                assert_eq!(fin, lines.len() + 1, "final lines in order");
                lines.push((f[0].to_string(), orig));
            }
        }
        lines
    }

    /// What the walk settles, per HEAD line (commit id, original line 1-based), taking steps of
    /// `slice`; also the number of steps.
    fn walked(fx: &Fixture, slice: Duration) -> (Vec<(String, usize)>, usize) {
        let mut walk = Walk::new(&fx.repo, Path::new(fx.path))
            .unwrap()
            .unwrap()
            .with_slice(slice);
        assert_eq!(walk.head(), fx.head);
        let lines = diff::lines(walk.text()).len();
        let mut got: Vec<Option<(String, usize)>> = vec![None; lines];
        let mut steps = 0;
        loop {
            let step = walk.step().unwrap();
            steps += 1;
            for s in step.settled {
                for i in 0..s.count {
                    let slot = &mut got[s.line + i];
                    assert!(slot.is_none(), "line {} settled twice", s.line + i);
                    *slot = Some((s.commit.id.to_string(), s.orig + i + 1));
                }
            }
            if step.done {
                break;
            }
        }
        assert!(lines == 0 || walk.visited() > 0);
        let got = got
            .into_iter()
            .enumerate()
            .map(|(i, l)| l.unwrap_or_else(|| panic!("line {i} never settled")))
            .collect();
        (got, steps)
    }

    #[test]
    fn blame_walk_matches_the_saved_references() {
        for name in NAMES {
            let fx = build(name);
            let want = parse(reference(name));
            let (got, _) = walked(&fx, SLICE);
            assert_eq!(got, want, "{name}: full walk");
        }
    }

    #[test]
    fn blame_walk_in_short_steps_matches_the_saved_references() {
        for name in NAMES {
            let fx = build(name);
            let want = parse(reference(name));
            let (got, steps) = walked(&fx, Duration::ZERO);
            assert_eq!(got, want, "{name}: short steps");
            // (The re-added file settles in its first commit.)
            if !want.is_empty() && *name != "readded" {
                assert!(steps > 1, "{name}: more than one step");
            }
        }
    }

    /// The fixtures' ids don't change between builds, and the references name commits they have.
    #[test]
    fn blame_fixtures_are_deterministic() {
        for name in NAMES {
            let (a, b) = (build(name), build(name));
            assert_eq!(a.head, b.head, "{name}");
            let repo = a.repo.open().unwrap();
            for (id, _) in parse(reference(name)) {
                repo.find_commit(Oid::from_str(&id).unwrap())
                    .unwrap_or_else(|_| panic!("{name}: the reference names a commit the fixture lacks: regenerate"));
            }
        }
    }

    /// A commit queued again after it was taken is taken again (the skewed fixture's root: after
    /// the merge, p, itself, then x), and what the later child passes it is settled too.
    #[test]
    fn blame_walk_takes_a_commit_again_when_a_later_child_passes_it_lines() {
        let fx = build("skewed");
        let mut walk = Walk::new(&fx.repo, Path::new(fx.path)).unwrap().unwrap();
        while !walk.step().unwrap().done {}
        assert_eq!(walk.visited(), 5, "merge, p, root, x, root again");
    }

    /// The walk moves to `spawn_blocking`.
    #[test]
    fn blame_walk_is_send() {
        fn send<T: Send>() {}
        send::<Walk>();
    }

    #[test]
    fn blame_walk_of_a_missing_file_is_none() {
        let fx = build("history");
        assert!(Walk::new(&fx.repo, Path::new("nope.txt")).unwrap().is_none());
    }

    /// Run by hand to make the references again after a fixture changed (it runs the `git`
    /// program, which no other test does): `cargo test -p jed regenerate_blame_references --
    /// --ignored`.
    #[test]
    #[ignore = "runs git; writes jed/src/testdata/blame"]
    fn regenerate_blame_references() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/blame");
        std::fs::create_dir_all(&dir).unwrap();
        for name in NAMES {
            let fx = build(name);
            assert_eq!(fx.head, build(name).head, "{name}: the fixture is not deterministic");
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&fx.repo.workdir)
                .args(["blame", "--porcelain", &fx.head.to_string(), "--", fx.path])
                .output()
                .unwrap();
            assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
            std::fs::write(dir.join(format!("{name}.porcelain")), &out.stdout).unwrap();
        }
    }
}
