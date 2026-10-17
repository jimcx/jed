//! Inline git blame: who last changed the primary cursor's line, and when (VS Code's
//! `git.blame.editorDecoration`). No drawing (`ui::editor` draws the label after the cursor
//! line's text) and no app state (`app/blame.rs` schedules the computations while
//! "Git: Toggle Inline Blame" is on).
//!
//! A buffer's blame ([`Blame`], on every `Buffer` as `buf.blame`) is a list of hunks — runs of
//! lines last changed by one commit, or not committed — kept as byte positions of their first
//! line, mapped through every change set applied (as quick diff's marks), so the cursor line's
//! commit is a binary search and moving the cursor costs nothing.
//!
//! [`compute`] runs off the UI thread in two steps: `git blame` of the file at HEAD
//! (`Repository::blame_file`, the slow part: it walks history), kept as a [`Base`] with the HEAD
//! text and reused while HEAD and the path stay the same; then the buffer's text against that
//! HEAD text (a context-0 line diff — what `Blame::blame_buffer` does), so lines changed since
//! the last commit, unsaved edits included, are uncommitted. After an edit only the second step
//! runs again. Files not in HEAD (untracked, only staged, no commits yet), binary files and
//! files outside a repository have no blame.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use git2::{ErrorCode, Oid};
use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::git::{Repo, diff};

/// How long after an edit the blame is computed again.
pub const DELAY: Duration = Duration::from_millis(300);

/// Blank cells between a line's end and its blame.
pub const GAP: usize = 3;

/// The blame isn't drawn in fewer cells than this (it would be little more than "…").
pub const MIN_WIDTH: usize = 6;

/// A commit as the blame shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: Oid,
    /// The author's name (VS Code shows the author, not the committer).
    pub author: String,
    /// The author's email is the repository's `user.email`: shown as "You".
    pub you: bool,
    /// The author time, Unix seconds.
    pub time: i64,
    /// The message's first line.
    pub summary: String,
}

/// Lines from `start` (a line start) to the next hunk's start: one commit's, or uncommitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub start: ByteIdx,
    pub commit: Option<Arc<Commit>>,
}

/// The cursor line's blame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    Committed(&'a Commit),
    /// Changed since the last commit (saved or not).
    Uncommitted,
}

/// `git blame` of a file at HEAD: what each HEAD line's commit is, reused while HEAD and the
/// path stay the same.
#[derive(Debug, PartialEq, Eq)]
pub struct Base {
    pub head: Oid,
    pub path: PathBuf,
    /// The file's text at HEAD.
    text: Vec<u8>,
    /// Each run's first line (0-based, in `text`) and commit, in order.
    lines: Vec<(usize, Arc<Commit>)>,
}

/// A buffer's blame and the state of its computation.
#[derive(Debug, Default)]
pub struct Blame {
    /// Sorted by start; each runs to the next one's start, the last to `end`.
    hunks: Vec<Hunk>,
    /// The end of the blamed text: a line starting there (the empty line after a final line
    /// break) has no blame.
    end: ByteIdx,
    /// What the hunks were computed from (HEAD's blame).
    base: Option<Arc<Base>>,
    /// The buffer revision the hunks were computed for; `None` before the first computation.
    computed: Option<u64>,
    /// HEAD may have moved since (a git status refresh): compute again (cheap when it didn't).
    head_stale: bool,
    /// A computation is running: its generation, and the change sets applied since its
    /// snapshot, to map its result through.
    running: Option<(u64, Vec<Arc<ChangeSet>>)>,
    generation: u64,
}

/// Whether a buffer's blame needs computing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    Nothing,
    /// Never computed, or HEAD may have moved: at once.
    Now,
    /// Edited since: after `DELAY`.
    Later,
}

/// A computation's input: what `compute` reads off-thread.
pub struct Job {
    pub repo: Repo,
    /// Relative to the work tree.
    pub path: PathBuf,
    pub rope: Rope,
    pub bom: bool,
    pub revision: u64,
    pub generation: u64,
    /// The last HEAD blame, reused when HEAD and the path are the same.
    pub base: Option<Arc<Base>>,
    /// The current hunks are for `revision`: nothing to do if HEAD is the same.
    pub same_text: bool,
}

/// A computation's result, for `revision` of the buffer.
#[derive(Debug)]
pub struct Computed {
    pub revision: u64,
    pub generation: u64,
    pub result: Result<Found, String>,
}

#[derive(Debug)]
pub enum Found {
    /// Not in HEAD (untracked, only staged, no commits yet) or binary: no blame.
    NotTracked,
    /// HEAD and the text are the ones the hunks came from.
    Unchanged,
    Blamed {
        base: Arc<Base>,
        hunks: Vec<Hunk>,
        end: ByteIdx,
    },
}

impl Blame {
    /// A computation is running (tests wait for it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// The hunks (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    /// The buffer's text is `revision`: what needs doing.
    pub fn want(&self, revision: u64) -> Want {
        match self.computed {
            _ if self.running.is_some() => Want::Nothing,
            None => Want::Now,
            Some(_) if self.head_stale => Want::Now,
            Some(r) if r != revision => Want::Later,
            Some(_) => Want::Nothing,
        }
    }

    /// HEAD may have moved (a git status read): compute again.
    pub fn head_changed(&mut self) {
        self.head_stale = true;
    }

    /// No repository, or the file is outside it: no blame, nothing to compute until HEAD may
    /// have changed (a repository appearing refreshes the status, which calls `head_changed`).
    pub fn clear(&mut self, revision: u64) {
        self.hunks.clear();
        self.base = None;
        self.computed = Some(revision);
        self.head_stale = false;
    }

    /// Forgets everything (the blame turned off): computed afresh when it's on again; a
    /// running computation's result is dropped.
    pub fn reset(&mut self) {
        *self = Blame {
            generation: self.generation,
            ..Default::default()
        };
    }

    /// A computation starts from the text at `revision`: its generation, the HEAD blame to
    /// reuse and whether the hunks are already for this text.
    pub fn begin(&mut self, revision: u64) -> (u64, Option<Arc<Base>>, bool) {
        self.generation += 1;
        self.running = Some((self.generation, Vec::new()));
        self.head_stale = false;
        (self.generation, self.base.clone(), self.computed == Some(revision))
    }

    /// A computation ended: its hunks, mapped through the edits made since its snapshot,
    /// replace the current ones. A result of an older generation (reset since) is dropped.
    pub fn finish(&mut self, done: Computed) {
        let sets = match self.running.take() {
            Some((g, sets)) if g == done.generation => sets,
            other => {
                self.running = other;
                return;
            }
        };
        match done.result {
            Ok(Found::NotTracked) => {
                self.hunks.clear();
                self.base = None;
                self.computed = Some(done.revision);
            }
            Ok(Found::Unchanged) => self.computed = Some(done.revision),
            Ok(Found::Blamed {
                base,
                mut hunks,
                mut end,
            }) => {
                for set in &sets {
                    end = map_hunks(&mut hunks, end, set);
                }
                self.hunks = hunks;
                self.end = end;
                self.base = Some(base);
                self.computed = Some(done.revision);
            }
            Err(e) => {
                tracing::warn!("git blame failed: {e}");
                self.hunks.clear();
                self.base = None;
                self.computed = Some(done.revision);
            }
        }
        // Edits since the snapshot: `want` sees a newer revision and computes again later.
    }

    /// Follows a change set applied to the buffer (`Buffer::apply`).
    pub fn map(&mut self, changes: &Arc<ChangeSet>) {
        if let Some((_, sets)) = &mut self.running {
            sets.push(changes.clone());
        }
        self.end = map_hunks(&mut self.hunks, self.end, changes);
    }

    /// The blame of `line` (0-based): `None` when there is none (no blame computed, the line
    /// after a final line break).
    pub fn line(&self, rope: &Rope, line: usize) -> Option<Line<'_>> {
        let start = pos::line_start(rope, line.min(pos::line_count(rope) - 1));
        if start >= self.end {
            return None;
        }
        let i = self.hunks.partition_point(|h| h.start <= start);
        let h = &self.hunks[i.checked_sub(1)?];
        Some(match &h.commit {
            Some(c) => Line::Committed(c),
            None => Line::Uncommitted,
        })
    }
}

/// Maps hunk starts (sorted, so in order) and the end through a change set. `Before`: text
/// typed at a hunk's first line start stays on that line, so in that hunk.
fn map_hunks(hunks: &mut [Hunk], end: ByteIdx, changes: &ChangeSet) -> ByteIdx {
    if changes.is_empty() {
        return end;
    }
    let mapper = changes.mapper();
    for h in hunks.iter_mut() {
        h.start = mapper.map(h.start, Assoc::Before);
    }
    mapper.map(end, Assoc::Before)
}

/// The label drawn after the line: `Ann Smith, 3 days ago • Fix crash` (`You` for the
/// repository's user), or `Uncommitted changes` (no time: unsaved edits have none).
pub fn label(line: Line, now_ms: u64) -> String {
    match line {
        Line::Uncommitted => "Uncommitted changes".into(),
        Line::Committed(c) => {
            let who = if c.you { "You" } else { &c.author };
            let when = crate::history::ago(c.time.max(0) as u64 * 1000, now_ms);
            if c.summary.is_empty() {
                format!("{who}, {when}")
            } else {
                format!("{who}, {when} • {}", c.summary)
            }
        }
    }
}

/// `label` in at most `width` cells: whole, or cut with "…"; `None` below `MIN_WIDTH`.
pub fn fit(label: &str, width: usize) -> Option<String> {
    if width < MIN_WIDTH {
        return None;
    }
    let mut out = String::new();
    let mut used = 0;
    for c in label.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            // Back off to leave a cell for "…".
            while used + 1 > width {
                let Some(last) = out.pop() else { break };
                used -= last.width().unwrap_or(0);
            }
            out.push('…');
            return Some(out);
        }
        out.push(c);
        used += w;
    }
    Some(out)
}

/// Blocking: the blame of `job`'s text.
pub fn compute(job: Job) -> Computed {
    Computed {
        revision: job.revision,
        generation: job.generation,
        result: find(&job).map_err(|e| e.message().to_string()),
    }
}

fn find(job: &Job) -> Result<Found, git2::Error> {
    let r = job.repo.open()?;
    let head = match r.head().and_then(|h| h.peel_to_commit()) {
        Ok(c) => c.id(),
        // No commits yet.
        Err(e) if matches!(e.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => return Ok(Found::NotTracked),
        Err(e) => return Err(e),
    };
    let base = match &job.base {
        Some(b) if b.head == head && b.path == job.path => {
            if job.same_text {
                return Ok(Found::Unchanged);
            }
            b.clone()
        }
        _ => match blame_head(&r, head, &job.path)? {
            Some(b) => Arc::new(b),
            None => return Ok(Found::NotTracked),
        },
    };
    let ours = crate::buffer::file::encode(&job.rope, job.bom);
    match hunks(&base, &ours, if job.bom { 3 } else { 0 })? {
        Some((hunks, end)) => Ok(Found::Blamed { base, hunks, end }),
        None => Ok(Found::NotTracked),
    }
}

/// Blocking: `git blame` of `path` at `head`; `None` when the file isn't in that commit.
fn blame_head(r: &git2::Repository, head: Oid, path: &std::path::Path) -> Result<Option<Base>, git2::Error> {
    let tree = r.find_commit(head)?.tree()?;
    let entry = match tree.get_path(path) {
        Ok(e) => e,
        Err(e) if e.code() == ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let Ok(blob) = r.find_blob(entry.id()) else {
        // A folder or a submodule.
        return Ok(None);
    };
    let text = blob.content().to_vec();
    let mut opts = git2::BlameOptions::new();
    opts.newest_commit(head);
    let blame = r.blame_file(path, Some(&mut opts))?;
    let email = r
        .config()
        .and_then(|c| c.get_string("user.email"))
        .ok()
        .filter(|e| !e.is_empty());
    let mut commits: HashMap<Oid, Arc<Commit>> = HashMap::new();
    let mut lines = Vec::with_capacity(blame.len());
    for h in blame.iter() {
        let id = h.final_commit_id();
        let commit = match commits.get(&id) {
            Some(c) => c.clone(),
            None => {
                let c = r.find_commit(id)?;
                let author = c.author();
                let summary = String::from_utf8_lossy(c.message_bytes())
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let commit = Arc::new(Commit {
                    id,
                    author: clean(&String::from_utf8_lossy(author.name_bytes())),
                    you: email
                        .as_deref()
                        .is_some_and(|e| e.eq_ignore_ascii_case(&String::from_utf8_lossy(author.email_bytes()))),
                    time: author.when().seconds(),
                    summary: clean(&summary),
                });
                commits.insert(id, commit.clone());
                commit
            }
        };
        lines.push((h.final_start_line().saturating_sub(1), commit));
    }
    lines.sort_by_key(|(l, _)| *l);
    Ok(Some(Base {
        head,
        path: path.to_path_buf(),
        text,
        lines,
    }))
}

/// Control characters as spaces (a label is one row).
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// The hunks of the buffer's bytes `ours` against HEAD's blame (`skip`: the BOM's length at
/// its start, which the buffer's positions don't count): unchanged lines keep their HEAD line's
/// commit, changed and added ones are uncommitted. `None` for a binary text.
fn hunks(base: &Base, ours: &[u8], skip: usize) -> Result<Option<(Vec<Hunk>, ByteIdx)>, git2::Error> {
    let d = diff::diff(&base.text, ours, 0)?;
    if d.binary {
        return Ok(None);
    }
    // Each line's start in `ours`, and its end.
    let mut starts = vec![0];
    starts.extend(
        ours.iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1),
    );
    if starts.last() == Some(&ours.len()) && !ours.is_empty() {
        starts.pop();
    }
    starts.push(ours.len());
    let count = starts.len() - 1;
    let at = |line: usize| ByteIdx(starts[line.min(count)].saturating_sub(skip));

    let mut out: Vec<Hunk> = Vec::new();
    let mut push = |line: usize, commit: Option<&Arc<Commit>>| {
        let same = out.last().is_some_and(|h| match (&h.commit, commit) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        });
        if !same {
            out.push(Hunk {
                start: at(line),
                commit: commit.cloned(),
            });
        }
    };
    // Unchanged lines `new` of ours are HEAD's lines from `old`: their runs' commits.
    let unchanged = |push: &mut dyn FnMut(usize, Option<&Arc<Commit>>), new: std::ops::Range<usize>, old: usize| {
        if new.is_empty() || base.lines.is_empty() {
            return;
        }
        let mut k = base.lines.partition_point(|(l, _)| *l <= old).saturating_sub(1);
        let mut line = new.start;
        while line < new.end {
            push(line, Some(&base.lines[k].1));
            match base.lines.get(k + 1) {
                Some((next, _)) => line = new.start + next.saturating_sub(old).max(line - new.start + 1),
                None => break,
            }
            k += 1;
        }
    };
    let (mut new, mut old) = (0, 0);
    for h in &d.hunks {
        unchanged(&mut push, new..h.new.start, old);
        if !h.new.is_empty() {
            push(h.new.start, None);
        }
        new = h.new.end;
        old = h.old.end;
    }
    unchanged(&mut push, new..count, old);
    Ok(Some((out, ByteIdx(ours.len().saturating_sub(skip)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::change::Change;
    use crate::git::test_repo::{git, init};

    fn commit(name: &str) -> Arc<Commit> {
        Arc::new(Commit {
            id: Oid::ZERO_SHA1,
            author: name.into(),
            you: false,
            time: 0,
            summary: format!("{name}'s change"),
        })
    }

    fn make_base(text: &str, lines: &[(usize, &Arc<Commit>)]) -> Base {
        Base {
            head: Oid::ZERO_SHA1,
            path: "f".into(),
            text: text.as_bytes().to_vec(),
            lines: lines.iter().map(|(l, c)| (*l, (*c).clone())).collect(),
        }
    }

    fn blamed(base: &Base, ours: &str) -> Blame {
        let (hunks, end) = hunks(base, ours.as_bytes(), 0).unwrap().unwrap();
        Blame {
            hunks,
            end,
            computed: Some(0),
            ..Default::default()
        }
    }

    /// Each line's author ("-" uncommitted, "" none).
    fn authors(b: &Blame, rope: &Rope) -> Vec<String> {
        (0..pos::line_count(rope))
            .map(|l| match b.line(rope, l) {
                Some(Line::Committed(c)) => c.author.clone(),
                Some(Line::Uncommitted) => "-".into(),
                None => "".into(),
            })
            .collect()
    }

    #[test]
    fn unchanged_lines_keep_heads_commit_changed_ones_are_uncommitted() {
        let (a, b) = (commit("a"), commit("b"));
        let base = make_base("1\n2\n3\n4\n", &[(0, &a), (2, &b)]);
        // Unchanged.
        let text = "1\n2\n3\n4\n";
        let bl = blamed(&base, text);
        assert_eq!(bl.hunks.len(), 2);
        assert_eq!(authors(&bl, &Rope::from_str(text)), ["a", "a", "b", "b", ""]);
        // A line added at the top, line 3 modified, line 2 deleted.
        let text = "0\n1\nX\n4\n";
        let bl = blamed(&base, text);
        assert_eq!(authors(&bl, &Rope::from_str(text)), ["-", "a", "-", "b", ""]);
        // The last line without a line break, changed: the line still has a blame.
        let text = "1\n2\n3\n5";
        let bl = blamed(&base, text);
        assert_eq!(authors(&bl, &Rope::from_str(text)), ["a", "a", "b", "-"]);
        // Lines inserted inside a run split it.
        let text = "1\nnew\n2\n3\n4\n";
        let bl = blamed(&base, text);
        assert_eq!(authors(&bl, &Rope::from_str(text)), ["a", "-", "a", "b", "b", ""]);
        // A BOM in the file's bytes isn't in the buffer's positions.
        let mut bom = make_base("\u{feff}1\n2\n", &[(0, &a), (1, &b)]);
        bom.lines = vec![(0, a.clone()), (1, b.clone())];
        let (h, end) = hunks(&bom, "\u{feff}1\nX\n".as_bytes(), 3).unwrap().unwrap();
        assert_eq!(h[0].start, ByteIdx(0));
        assert_eq!((h[1].start, h[1].commit.is_none()), (ByteIdx(2), true));
        assert_eq!(end, ByteIdx(4));
        // Empty text: nothing.
        let bl = blamed(&base, "");
        assert_eq!(bl.line(&Rope::from_str(""), 0), None);
    }

    fn insert(b: &mut Blame, rope: &mut Rope, at: usize, text: &str) -> Arc<ChangeSet> {
        let set = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(at),
            end: ByteIdx(at),
            text: text.into(),
        }]));
        set.apply(rope);
        b.map(&set);
        set
    }

    #[test]
    fn hunks_follow_edits_and_results_map_through_edits_made_meanwhile() {
        let (a, b) = (commit("a"), commit("b"));
        let base = Arc::new(make_base("1\n2\n3\n", &[(0, &a), (1, &b)]));
        let text = "1\n2\n3\n";
        let mut bl = blamed(&base, text);
        let mut rope = Rope::from_str(text);
        // Two lines typed above: each line keeps its author.
        insert(&mut bl, &mut rope, 0, "x\ny\n");
        assert_eq!(authors(&bl, &rope), ["a", "a", "a", "b", "b", ""]);
        // Text typed at a run's first line start stays on that line, in its run.
        insert(&mut bl, &mut rope, 6, "z");
        assert_eq!(rope.to_string(), "x\ny\n1\nz2\n3\n");
        assert_eq!(authors(&bl, &rope), ["a", "a", "a", "b", "b", ""]);
        // Typed on the empty line after the last: still no blame there.
        let len = rope.len();
        insert(&mut bl, &mut rope, len, "end");
        assert_eq!(authors(&bl, &rope)[5], "");

        // A computation started at revision 1 ends after another edit: mapped through it.
        assert_eq!(bl.want(1), Want::Later);
        let (generation, kept, same) = bl.begin(1);
        assert!(kept.is_none() && !same);
        assert_eq!(bl.want(1), Want::Nothing);
        let snapshot = rope.clone();
        insert(&mut bl, &mut rope, 0, "w\n");
        let (hunks, end) = hunks(&base, snapshot.to_string().as_bytes(), 0).unwrap().unwrap();
        bl.finish(Computed {
            revision: 1,
            generation,
            result: Ok(Found::Blamed {
                base: base.clone(),
                hunks,
                end,
            }),
        });
        assert_eq!(authors(&bl, &rope), ["-", "-", "-", "a", "-", "b", "-"]);
        assert_eq!(bl.want(2), Want::Later, "edited since the snapshot");
        assert_eq!(bl.want(1), Want::Nothing);
        bl.head_changed();
        assert_eq!(bl.want(1), Want::Now);
        let (_, kept, same) = bl.begin(1);
        assert!(kept.is_some_and(|k| Arc::ptr_eq(&k, &base)) && same);

        // Reset (turned off): an older computation's result is dropped.
        bl.reset();
        assert_eq!(bl.want(1), Want::Now);
        assert_eq!(bl.line(&rope, 0), None);
        bl.finish(Computed {
            revision: 1,
            generation,
            result: Ok(Found::NotTracked),
        });
        assert_eq!(bl.want(1), Want::Now, "a stale result changes nothing");
    }

    #[test]
    fn labels_and_cutting() {
        let mut c = Commit {
            id: Oid::ZERO_SHA1,
            author: "Ann Smith".into(),
            you: false,
            time: 1_000_000,
            summary: "Fix crash opening a file".into(),
        };
        let now = (1_000_000 + 3 * 86_400) * 1000;
        assert_eq!(
            label(Line::Committed(&c), now),
            "Ann Smith, 3 days ago • Fix crash opening a file"
        );
        c.you = true;
        assert_eq!(
            label(Line::Committed(&c), now),
            "You, 3 days ago • Fix crash opening a file"
        );
        assert_eq!(
            label(Line::Committed(&c), (1_000_000 + 120) * 1000),
            "You, 2 min ago • Fix crash opening a file"
        );
        c.summary.clear();
        assert_eq!(label(Line::Committed(&c), now), "You, 3 days ago");
        assert_eq!(label(Line::Uncommitted, now), "Uncommitted changes");

        assert_eq!(fit("Uncommitted changes", 40).as_deref(), Some("Uncommitted changes"));
        assert_eq!(fit("Uncommitted changes", 19).as_deref(), Some("Uncommitted changes"));
        assert_eq!(fit("Uncommitted changes", 18).as_deref(), Some("Uncommitted chang…"));
        assert_eq!(fit("Uncommitted changes", 6).as_deref(), Some("Uncom…"));
        assert_eq!(fit("Uncommitted changes", 5), None);
        // Wide chars never overflow.
        let cut = fit("日本語のコミット", 7).unwrap();
        assert_eq!(cut, "日本語…");
    }

    fn commit_as(dir: &std::path::Path, who: &str, date: &str, message: &str) {
        git(dir, &["add", "-A"]);
        git(
            dir,
            &[
                "-c",
                &format!("user.name={who}"),
                "commit",
                "-q",
                "--author",
                &format!("{who} <{}@example.com>", who.to_lowercase()),
                "--date",
                date,
                "-m",
                message,
            ],
        );
    }

    fn job(root: &std::path::Path, path: &str, text: &str, base: Option<Arc<Base>>, same_text: bool) -> Job {
        Job {
            repo: Repo::discover(root).unwrap(),
            path: path.into(),
            rope: Rope::from_str(text),
            bom: false,
            revision: 7,
            generation: 1,
            base,
            same_text,
        }
    }

    #[test]
    fn compute_blames_head_then_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        init(&root);
        std::fs::write(root.join("f.txt"), "one\ntwo\n").unwrap();
        commit_as(&root, "Ann", "2020-01-01T00:00:00Z", "First\n\nBody text");
        std::fs::write(root.join("f.txt"), "one\nTWO\nthree\n").unwrap();
        commit_as(&root, "Bob", "2021-06-01T12:00:00Z", "Second change");
        git(&root, &["config", "user.email", "bob@example.com"]);

        let done = compute(job(&root, "f.txt", "one\nTWO\nthree\n", None, false));
        assert_eq!(done.revision, 7);
        let Ok(Found::Blamed { base, hunks, end }) = done.result else {
            panic!("blamed: {:?}", done.result)
        };
        let bl = Blame {
            hunks,
            end,
            computed: Some(7),
            ..Default::default()
        };
        let rope = Rope::from_str("one\nTWO\nthree\n");
        assert_eq!(authors(&bl, &rope), ["Ann", "Bob", "Bob", ""]);
        let Some(Line::Committed(c)) = bl.line(&rope, 0) else {
            panic!()
        };
        assert_eq!((c.summary.as_str(), c.you, c.time), ("First", false, 1_577_836_800));
        let Some(Line::Committed(c)) = bl.line(&rope, 1) else {
            panic!()
        };
        assert_eq!((c.summary.as_str(), c.you), ("Second change", true));

        // The same HEAD and text: nothing to do. An edit: HEAD's blame reused.
        let done = compute(job(&root, "f.txt", "x", Some(base.clone()), true));
        assert!(matches!(done.result, Ok(Found::Unchanged)));
        let done = compute(job(&root, "f.txt", "one\nedited\nthree\n", Some(base.clone()), false));
        let Ok(Found::Blamed {
            base: again,
            hunks,
            end,
        }) = done.result
        else {
            panic!()
        };
        assert!(Arc::ptr_eq(&again, &base));
        let bl = Blame {
            hunks,
            end,
            ..Default::default()
        };
        assert_eq!(
            authors(&bl, &Rope::from_str("one\nedited\nthree\n")),
            ["Ann", "-", "Bob", ""]
        );

        // A new commit: blamed again.
        std::fs::write(root.join("f.txt"), "ONE\nTWO\nthree\n").unwrap();
        commit_as(&root, "Cy", "2022-01-01T00:00:00Z", "Third");
        let done = compute(job(&root, "f.txt", "ONE\nTWO\nthree\n", Some(base.clone()), true));
        let Ok(Found::Blamed { base: newer, .. }) = done.result else {
            panic!()
        };
        assert_ne!(newer.head, base.head);

        // Untracked, only staged: no blame.
        std::fs::write(root.join("new.txt"), "n\n").unwrap();
        assert!(matches!(
            compute(job(&root, "new.txt", "n\n", None, false)).result,
            Ok(Found::NotTracked)
        ));
        git(&root, &["add", "new.txt"]);
        assert!(matches!(
            compute(job(&root, "new.txt", "n\n", None, false)).result,
            Ok(Found::NotTracked)
        ));
    }

    #[test]
    fn compute_without_commits_finds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        init(&root);
        std::fs::write(root.join("f.txt"), "a\n").unwrap();
        assert!(matches!(
            compute(job(&root, "f.txt", "a\n", None, false)).result,
            Ok(Found::NotTracked)
        ));
    }
}
