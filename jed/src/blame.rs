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
//! [`compute`] runs off the UI thread in two steps: HEAD's file (a [`Base`]: its text and, per
//! line, the commit that last changed it once known), then the buffer's text against that HEAD
//! text (a context-0 line diff — `hunks`), so lines changed since the last commit, unsaved
//! edits included, are uncommitted. The commits come from a streaming job (`app/blame.rs`
//! running `git::blame::stream`): its chunks fill the `Base` in as the walk settles lines, and
//! each chunk makes the hunks computed again (the diff only; the walk is not restarted by edits).
//! A line whose commit isn't known yet is [`Line::Pending`] and draws no label. A finished
//! `Base` is kept for the same HEAD and path. Files not in HEAD (untracked, only staged, no
//! commits yet), binary files and files outside a repository have no blame.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use git2::{ErrorCode, Oid};
use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::git::blame::{Settled, Walk};
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

/// Whose a run of lines is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// Changed since the last commit (saved or not).
    Uncommitted,
    /// Unchanged since HEAD, but the walk hasn't settled these lines yet.
    Pending,
    Commit(Arc<Commit>),
}

/// Lines from `start` (a line start) to the next hunk's start: one commit's, or uncommitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub start: ByteIdx,
    pub commit: Owner,
}

/// The cursor line's blame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    Committed(&'a Commit),
    /// Changed since the last commit (saved or not).
    Uncommitted,
    /// Not changed, but its commit isn't known yet (the walk is still running): no label.
    Pending,
}

/// A file at HEAD and what each of its lines' commit is, as far as the walk has got: reused
/// while HEAD and the path stay the same.
#[derive(Debug, Clone)]
pub struct Base {
    pub head: Oid,
    pub path: PathBuf,
    /// The file's text at HEAD.
    text: Arc<Vec<u8>>,
    /// Each line's commit (0-based, in `text`); `None` until the walk settles it.
    lines: Vec<Option<Arc<Commit>>>,
    /// The walk is over: every line is settled.
    pub done: bool,
}

impl Base {
    /// A base of `text` with no line settled.
    pub fn new(head: Oid, path: PathBuf, text: &[u8]) -> Base {
        Base {
            head,
            path,
            lines: vec![None; diff::lines(text).len()],
            text: Arc::new(text.to_vec()),
            done: false,
        }
    }

    /// The walk settled these lines.
    fn settle(&mut self, settled: &[Settled]) {
        for s in settled {
            let end = (s.line + s.count).min(self.lines.len());
            for slot in self.lines.get_mut(s.line..end).unwrap_or_default() {
                *slot = Some(s.commit.clone());
            }
        }
    }
}

/// The running walk's handle on a buffer's blame: dropped (HEAD moved, the blame turned off, the
/// path changed, the buffer closed), it tells the job to stop.
#[derive(Debug)]
struct Stream {
    id: u64,
    cancel: Arc<AtomicBool>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A walk to run on `spawn_blocking` (`git::blame::stream`), sending its chunks tagged `id`.
pub struct WalkStart {
    pub walk: Box<Walk>,
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
}

/// A buffer's blame and the state of its computation.
#[derive(Debug, Default)]
pub struct Blame {
    /// Sorted by start; each runs to the next one's start, the last to `end`.
    hunks: Vec<Hunk>,
    /// The end of the blamed text: a line starting there (the empty line after a final line
    /// break) has no blame.
    end: ByteIdx,
    /// HEAD's file and the commits of its lines known so far.
    base: Option<Arc<Base>>,
    /// The walk filling `base` in, while it runs.
    stream: Option<Stream>,
    /// Chunks applied to `base`, and the count the hunks were computed with: unequal, the
    /// hunks are computed again.
    lines_rev: u64,
    hunks_rev: u64,
    streams: u64,
    /// The buffer revision the hunks were computed for; `None` before the first computation.
    computed: Option<u64>,
    /// HEAD may have moved since (a git status refresh): compute again (cheap when it didn't).
    head_stale: bool,
    /// A computation is running: its generation, the change sets applied since its snapshot,
    /// to map its result through, and the `lines_rev` it saw.
    running: Option<(u64, Vec<Arc<ChangeSet>>, u64)>,
    generation: u64,
}

/// Whether a buffer's blame needs computing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    Nothing,
    /// Never computed, HEAD may have moved, or new commits are known: at once.
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
    /// The last HEAD file, reused when HEAD and the path are the same.
    pub base: Option<Arc<Base>>,
    /// A walk is filling `base` in (else an unfinished base is walked again).
    pub walking: bool,
    /// The current hunks are for `revision` and `base`: nothing to do if HEAD is the same.
    pub same_text: bool,
    /// The new walk's time slice (tests make it short; `None`: the default).
    pub slice: Option<Duration>,
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
    /// HEAD, the text and the known commits are the ones the hunks came from.
    Unchanged,
    Blamed {
        hunks: Vec<Hunk>,
        end: ByteIdx,
        /// HEAD isn't the base's: its file, and the walk to fill it in.
        started: Option<(Arc<Base>, Box<Walk>)>,
    },
}

/// What a computation needs from the buffer.
pub struct Begin {
    pub generation: u64,
    pub base: Option<Arc<Base>>,
    pub walking: bool,
    pub same_text: bool,
}

impl Blame {
    /// A computation is running (tests wait for it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// The walk is running (tests wait for it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn walking(&self) -> bool {
        self.stream.is_some()
    }

    /// The running walk's cancel flag (tests).
    #[cfg(test)]
    pub fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        self.stream.as_ref().map(|s| s.cancel.clone())
    }

    /// The base isn't finished (tests).
    #[cfg(test)]
    pub fn unfinished(&self) -> bool {
        self.base.as_ref().is_some_and(|b| !b.done)
    }

    /// The chunks of the current walk taken so far (tests).
    #[cfg(test)]
    pub fn chunks_taken(&self) -> u64 {
        self.lines_rev
    }

    /// The HEAD lines whose commit is known (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settled(&self) -> usize {
        self.base
            .as_ref()
            .map_or(0, |b| b.lines.iter().filter(|l| l.is_some()).count())
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
            Some(_) if self.head_stale || self.lines_rev != self.hunks_rev => Want::Now,
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
        self.stream = None;
        self.computed = Some(revision);
        self.head_stale = false;
    }

    /// Forgets everything (the blame turned off) but a finished `Base`: computed afresh when
    /// it's on again; a running computation's result is dropped, and the walk is stopped.
    pub fn reset(&mut self) {
        let base = self.base.take().filter(|b| b.done);
        *self = Blame {
            generation: self.generation,
            streams: self.streams,
            base,
            ..Default::default()
        };
    }

    /// A computation starts from the text at `revision`.
    pub fn begin(&mut self, revision: u64) -> Begin {
        self.generation += 1;
        self.running = Some((self.generation, Vec::new(), self.lines_rev));
        self.head_stale = false;
        Begin {
            generation: self.generation,
            base: self.base.clone(),
            walking: self.stream.is_some(),
            same_text: self.computed == Some(revision) && self.lines_rev == self.hunks_rev,
        }
    }

    /// A computation ended: its hunks, mapped through the edits made since its snapshot,
    /// replace the current ones. A result of an older generation (reset since) is dropped. A
    /// new HEAD file replaces the base and stops the old walk; the new walk is returned to run.
    pub fn finish(&mut self, done: Computed) -> Option<WalkStart> {
        let (sets, seen) = match self.running.take() {
            Some((g, sets, seen)) if g == done.generation => (sets, seen),
            other => {
                self.running = other;
                return None;
            }
        };
        let mut start = None;
        match done.result {
            Ok(Found::NotTracked) => {
                self.hunks.clear();
                self.base = None;
                self.stream = None;
                self.computed = Some(done.revision);
            }
            Ok(Found::Unchanged) => self.computed = Some(done.revision),
            Ok(Found::Blamed {
                mut hunks,
                mut end,
                started,
            }) => {
                for set in &sets {
                    end = map_hunks(&mut hunks, end, set);
                }
                self.hunks = hunks;
                self.end = end;
                self.computed = Some(done.revision);
                self.hunks_rev = seen;
                if let Some((base, walk)) = started {
                    self.streams += 1;
                    let cancel = Arc::new(AtomicBool::new(false));
                    // Replacing the handle stops the old walk.
                    self.stream = Some(Stream {
                        id: self.streams,
                        cancel: cancel.clone(),
                    });
                    self.base = Some(base);
                    self.lines_rev = 0;
                    self.hunks_rev = 0;
                    start = Some(WalkStart {
                        walk,
                        id: self.streams,
                        cancel,
                    });
                }
            }
            Err(e) => {
                tracing::warn!("git blame failed: {e}");
                self.hunks.clear();
                self.base = None;
                self.stream = None;
                self.computed = Some(done.revision);
            }
        }
        // Edits since the snapshot: `want` sees a newer revision and computes again later.
        start
    }

    /// The walk `id` settled lines (`Err`: it failed), and `done` when it is over. A chunk of
    /// another walk (stopped since) is dropped. Returns whether it was taken.
    pub fn chunk(&mut self, id: u64, settled: Result<&[Settled], &str>, done: bool) -> bool {
        if self.stream.as_ref().is_none_or(|s| s.id != id) {
            return false;
        }
        match settled {
            Ok(settled) => {
                if let Some(base) = &mut self.base {
                    let base = Arc::make_mut(base);
                    base.settle(settled);
                    base.done |= done;
                    self.lines_rev += 1;
                }
            }
            Err(e) => tracing::warn!("git blame failed: {e}"),
        }
        if done || settled.is_err() {
            self.stream = None;
        }
        true
    }

    /// Follows a change set applied to the buffer (`Buffer::apply`).
    pub fn map(&mut self, changes: &Arc<ChangeSet>) {
        if let Some((_, sets, _)) = &mut self.running {
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
            Owner::Commit(c) => Line::Committed(c),
            Owner::Uncommitted => Line::Uncommitted,
            Owner::Pending => Line::Pending,
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

/// Characters of the commit id the label starts with.
pub const HASH_LEN: usize = 7;

/// The label drawn after the line: `7f3a2c1 Ann Smith, 3 days ago • Fix crash` (`You` for the
/// repository's user), `Uncommitted changes` (no hash, no time: unsaved edits have none).
pub fn label(line: Line, now_ms: u64) -> String {
    match line {
        Line::Uncommitted => "Uncommitted changes".into(),
        // No label until the commit is known.
        Line::Pending => String::new(),
        Line::Committed(c) => {
            let who = if c.you { "You" } else { &c.author };
            let when = crate::history::ago(c.time.max(0) as u64 * 1000, now_ms);
            let hash = short_id(&c.id);
            if c.summary.is_empty() {
                format!("{hash} {who}, {when}")
            } else {
                format!("{hash} {who}, {when} • {}", c.summary)
            }
        }
    }
}

/// The id's first `HASH_LEN` characters.
pub fn short_id(id: &Oid) -> String {
    id.to_string().chars().take(HASH_LEN).collect()
}

/// `label` in at most `width` cells: whole, or cut with "…"; `None` below `MIN_WIDTH`.
pub fn fit(label: &str, width: usize) -> Option<String> {
    fit_keeping(label, 0, width)
}

/// `fit` for a label starting with a hash of `keep` characters: a cut spares the hash while
/// the width has room for it plus the "…".
pub fn fit_keeping(label: &str, keep: usize, width: usize) -> Option<String> {
    if width < MIN_WIDTH {
        return None;
    }
    let keep = if width > keep { keep } else { 0 };
    let mut out = String::new();
    let mut used = 0;
    for c in label.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            // Back off to leave a cell for "…".
            while used + 1 > width && out.chars().count() > keep {
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
    let (base, started) = match &job.base {
        Some(b) if b.head == head && b.path == job.path && (b.done || job.walking) => {
            if job.same_text {
                return Ok(Found::Unchanged);
            }
            (b.clone(), None)
        }
        _ => {
            let Some(walk) = Walk::new(&job.repo, &job.path)? else {
                return Ok(Found::NotTracked);
            };
            let walk = match job.slice {
                Some(slice) => walk.with_slice(slice),
                None => walk,
            };
            let base = Arc::new(Base::new(walk.head(), job.path.clone(), walk.text()));
            (base.clone(), Some((base, Box::new(walk))))
        }
    };
    let ours = crate::buffer::file::encode(&job.rope, job.bom);
    match hunks(&base, &ours, if job.bom { 3 } else { 0 })? {
        Some((hunks, end)) => Ok(Found::Blamed { hunks, end, started }),
        None => Ok(Found::NotTracked),
    }
}

/// The repository's `user.email` (the author whose email it is shows as "You").
pub(crate) fn user_email(r: &git2::Repository) -> Option<String> {
    r.config()
        .and_then(|c| c.get_string("user.email"))
        .ok()
        .filter(|e| !e.is_empty())
}

/// Blocking: commit `id`'s blame metadata (`email`: [`user_email`]).
pub(crate) fn describe(r: &git2::Repository, id: Oid, email: Option<&str>) -> Result<Arc<Commit>, git2::Error> {
    let c = r.find_commit(id)?;
    let author = c.author();
    let summary = String::from_utf8_lossy(c.message_bytes())
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(Arc::new(Commit {
        id,
        author: clean(&String::from_utf8_lossy(author.name_bytes())),
        you: email.is_some_and(|e| e.eq_ignore_ascii_case(&String::from_utf8_lossy(author.email_bytes()))),
        time: author.when().seconds(),
        summary: clean(&summary),
    }))
}

/// Control characters as spaces (a label is one row).
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// A line's commit for `hunks`: `None` uncommitted, `Some(None)` not settled yet.
type Slot<'a> = Option<&'a Option<Arc<Commit>>>;

/// The hunks of the buffer's bytes `ours` against HEAD's file (`skip`: the BOM's length at its
/// start, which the buffer's positions don't count): unchanged lines keep their HEAD line's
/// commit (pending while the walk hasn't settled it), changed and added ones are uncommitted.
/// `None` for a binary text.
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
    // `line` starts a run of `commit` (`None`: uncommitted; `Some(None)`: not settled yet).
    let mut push = |line: usize, commit: Slot| {
        let same = out.last().is_some_and(|h| match (&h.commit, commit) {
            (Owner::Commit(a), Some(Some(b))) => Arc::ptr_eq(a, b),
            (Owner::Pending, Some(None)) | (Owner::Uncommitted, None) => true,
            _ => false,
        });
        if !same {
            out.push(Hunk {
                start: at(line),
                commit: match commit {
                    Some(Some(c)) => Owner::Commit(c.clone()),
                    Some(None) => Owner::Pending,
                    None => Owner::Uncommitted,
                },
            });
        }
    };
    // Unchanged lines `new` of ours are HEAD's lines from `old`.
    let unchanged = |push: &mut dyn FnMut(usize, Slot), new: std::ops::Range<usize>, old: usize| {
        for (i, line) in new.enumerate() {
            match base.lines.get(old + i) {
                Some(c) => push(line, Some(c)),
                None => push(line, None),
            }
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
        let mut base = Base::new(Oid::ZERO_SHA1, "f".into(), text.as_bytes());
        for (i, slot) in base.lines.iter_mut().enumerate() {
            *slot = lines.iter().rev().find(|(l, _)| *l <= i).map(|(_, c)| (*c).clone());
        }
        base
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
                Some(Line::Pending) => "?".into(),
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
        let bom = make_base("\u{feff}1\n2\n", &[(0, &a), (1, &b)]);
        let (h, end) = hunks(&bom, "\u{feff}1\nX\n".as_bytes(), 3).unwrap().unwrap();
        assert_eq!(h[0].start, ByteIdx(0));
        assert_eq!((h[1].start, h[1].commit == Owner::Uncommitted), (ByteIdx(2), true));
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
        let begin = bl.begin(1);
        let generation = begin.generation;
        assert!(begin.base.is_none() && !begin.same_text);
        assert_eq!(bl.want(1), Want::Nothing);
        let snapshot = rope.clone();
        insert(&mut bl, &mut rope, 0, "w\n");
        let (hunks, end) = hunks(&base, snapshot.to_string().as_bytes(), 0).unwrap().unwrap();
        bl.finish(Computed {
            revision: 1,
            generation,
            result: Ok(Found::Blamed {
                hunks,
                end,
                started: None,
            }),
        });
        assert_eq!(authors(&bl, &rope), ["-", "-", "-", "a", "-", "b", "-"]);
        assert_eq!(bl.want(2), Want::Later, "edited since the snapshot");
        assert_eq!(bl.want(1), Want::Nothing);
        bl.head_changed();
        assert_eq!(bl.want(1), Want::Now);
        bl.base = Some(base.clone());
        let begin = bl.begin(1);
        assert!(begin.base.is_some_and(|k| Arc::ptr_eq(&k, &base)) && begin.same_text);

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
    fn chunks_fill_the_base_in_and_a_stopped_walks_are_dropped() {
        let a = commit("a");
        let settled = |line, count| Settled {
            line,
            count,
            orig: line,
            commit: a.clone(),
        };
        let base = Arc::new(Base::new(Oid::ZERO_SHA1, "f".into(), b"1\n2\n3\n"));
        let mut bl = Blame {
            base: Some(base),
            stream: Some(Stream {
                id: 3,
                cancel: Arc::new(AtomicBool::new(false)),
            }),
            ..Default::default()
        };
        let flag = bl.cancel_flag().unwrap();
        assert!(!bl.chunk(2, Ok(&[settled(0, 3)]), false), "another walk's chunk");
        assert_eq!(bl.settled(), 0);
        assert!(bl.chunk(3, Ok(&[settled(1, 2)]), false));
        assert_eq!(bl.settled(), 2);
        assert!(bl.walking() && !flag.load(Ordering::SeqCst));
        assert!(bl.chunk(3, Ok(&[settled(0, 1)]), true));
        assert!(!bl.walking() && bl.base.as_ref().unwrap().done);
        // A finished base survives a reset (the blame turned off), an unfinished one doesn't.
        bl.reset();
        assert!(bl.base.is_some());
        bl.base = Some(Arc::new(Base::new(Oid::ZERO_SHA1, "f".into(), b"1\n")));
        bl.reset();
        assert!(bl.base.is_none());
        // Dropping the handle sets the flag.
        bl.stream = Some(Stream {
            id: 4,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let flag = bl.cancel_flag().unwrap();
        bl.reset();
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn labels_and_cutting() {
        let mut c = Commit {
            id: Oid::from_str("7f3a2c1d9e8b7a6f5e4d3c2b1a0f9e8d7c6b5a49").unwrap(),
            author: "Ann Smith".into(),
            you: false,
            time: 1_000_000,
            summary: "Fix crash opening a file".into(),
        };
        let now = (1_000_000 + 3 * 86_400) * 1000;
        assert_eq!(
            label(Line::Committed(&c), now),
            "7f3a2c1 Ann Smith, 3 days ago • Fix crash opening a file"
        );
        c.you = true;
        assert_eq!(
            label(Line::Committed(&c), now),
            "7f3a2c1 You, 3 days ago • Fix crash opening a file"
        );
        assert_eq!(
            label(Line::Committed(&c), (1_000_000 + 120) * 1000),
            "7f3a2c1 You, 2 min ago • Fix crash opening a file"
        );
        c.summary.clear();
        assert_eq!(label(Line::Committed(&c), now), "7f3a2c1 You, 3 days ago");
        assert_eq!(label(Line::Uncommitted, now), "Uncommitted changes");

        assert_eq!(fit("Uncommitted changes", 40).as_deref(), Some("Uncommitted changes"));
        assert_eq!(fit("Uncommitted changes", 19).as_deref(), Some("Uncommitted changes"));
        assert_eq!(fit("Uncommitted changes", 18).as_deref(), Some("Uncommitted chang…"));
        assert_eq!(fit("Uncommitted changes", 6).as_deref(), Some("Uncom…"));
        assert_eq!(fit("Uncommitted changes", 5), None);
        // A cut spares the hash while it fits with the "…".
        let l = label(Line::Committed(&c), now);
        assert_eq!(fit_keeping(&l, HASH_LEN, 8).as_deref(), Some("7f3a2c1…"));
        assert_eq!(fit_keeping(&l, HASH_LEN, 12).as_deref(), Some("7f3a2c1 You…"));
        assert_eq!(fit_keeping(&l, HASH_LEN, 6).as_deref(), Some("7f3a2…"));
        assert_eq!(fit_keeping(&l, HASH_LEN, 5), None);
        assert_eq!(fit_keeping(&l, HASH_LEN, 200).as_deref(), Some(l.as_str()));
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
        let walking = base.as_ref().is_some_and(|b| !b.done);
        Job {
            repo: Repo::discover(root).unwrap(),
            path: path.into(),
            rope: Rope::from_str(text),
            bom: false,
            revision: 7,
            generation: 1,
            base,
            walking,
            same_text,
            slice: None,
        }
    }

    /// Runs `walk` to its end into `base`'s lines, as the chunks would.
    fn finish_walk(mut base: Arc<Base>, mut walk: Walk) -> Arc<Base> {
        loop {
            let step = walk.step().unwrap();
            Arc::make_mut(&mut base).settle(&step.settled);
            if step.done {
                Arc::make_mut(&mut base).done = true;
                return base;
            }
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
        let Ok(Found::Blamed { hunks, end, started }) = done.result else {
            panic!("blamed: {:?}", done.result)
        };
        // Nothing settled yet: the unchanged lines are pending.
        let rope = Rope::from_str("one\nTWO\nthree\n");
        let bl = Blame {
            hunks,
            end,
            computed: Some(7),
            ..Default::default()
        };
        assert_eq!(authors(&bl, &rope), ["?", "?", "?", ""]);
        let (base, walk) = started.unwrap();
        let base = finish_walk(base, *walk);
        let done = compute(job(&root, "f.txt", "one\nTWO\nthree\n", Some(base.clone()), false));
        let Ok(Found::Blamed { hunks, end, started }) = done.result else {
            panic!()
        };
        assert!(started.is_none(), "a finished base is reused");
        let bl = Blame {
            hunks,
            end,
            computed: Some(7),
            ..Default::default()
        };
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
        let Ok(Found::Blamed { hunks, end, started }) = done.result else {
            panic!()
        };
        assert!(started.is_none());
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
        let Ok(Found::Blamed {
            started: Some((newer, _)),
            ..
        }) = done.result
        else {
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
