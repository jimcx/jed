//! Quick diff: the gutter's markers for lines added, modified and deleted in the buffer against
//! the file's index blob (VS Code's dirty diff decorations). No drawing (`ui::editor` draws the
//! markers, `ui::quick_diff` the peek) and no app state (`app/quick_diff.rs` schedules the
//! computations and stages).
//!
//! The markers are computed off the UI thread ([`compute`]: the index blob against a snapshot of
//! the buffer, `git::diff::diff` with context 0) and kept on the buffer as byte ranges
//! ([`QuickDiff`]), mapped through every change set applied after (so they follow edits
//! between computations without flickering), including those applied while a computation ran.
//! A file not in the index (untracked) has none.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use git2::Oid;
use ropey::Rope;

use crate::buffer::change::{Assoc, ChangeSet};
use crate::buffer::pos::{self, ByteIdx};
use crate::git::stage::{self, IndexBlob};
use crate::git::{HunkKind, Repo, diff};

/// How long after an edit the markers are computed again (VS Code waits 200 ms too).
pub const DELAY: Duration = Duration::from_millis(150);

/// The peek shows at most this many of a change's removed lines.
pub const MAX_REMOVED: usize = 200;

/// One change: lines `start..end` of the buffer (`start` a line start; `end` the start of the
/// line after, or the text's end) replaced `removed` of the index text. A deletion is a point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub kind: HunkKind,
    /// From the index → HEAD diff: the change is staged, not (or not only) in the working tree.
    /// The gutter shows the unstaged ones only; the scroll bar shows both.
    pub staged: bool,
    pub start: ByteIdx,
    pub end: ByteIdx,
    /// The index text's lines the change replaced (at most `MAX_REMOVED`, line breaks dropped,
    /// invalid UTF-8 replaced), and how many there were.
    pub removed: Arc<[String]>,
    pub removed_count: usize,
}

impl Mark {
    /// The theme key the scroll bar draws it in: the gutter's colour or, for a staged change,
    /// its darker sibling (the gutter itself shows the unstaged ones only).
    pub fn theme_key(&self) -> &'static str {
        match (self.kind, self.staged) {
            (HunkKind::Added, false) => "editorGutter.addedBackground",
            (HunkKind::Added, true) => "editorGutter.stagedAddedBackground",
            (HunkKind::Modified, false) => "editorGutter.modifiedBackground",
            (HunkKind::Modified, true) => "editorGutter.stagedModifiedBackground",
            (HunkKind::Deleted, false) => "editorGutter.deletedBackground",
            (HunkKind::Deleted, true) => "editorGutter.stagedDeletedBackground",
        }
    }
}

/// What a gutter cell shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Added,
    Modified,
    /// Lines were deleted below this line.
    DeletedBelow,
    /// Lines were deleted above this line (the first).
    DeletedAbove,
}

impl Marker {
    pub fn theme_key(self) -> &'static str {
        match self {
            Marker::Added => "editorGutter.addedBackground",
            Marker::Modified => "editorGutter.modifiedBackground",
            Marker::DeletedBelow | Marker::DeletedAbove => "editorGutter.deletedBackground",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Marker::Added | Marker::Modified => "▎",
            Marker::DeletedBelow => "▁",
            Marker::DeletedAbove => "▔",
        }
    }
}

/// A cached staged diff: the index blob it was computed against and the diff itself (HEAD →
/// index, which depends only on HEAD and the index, never the buffer's text).
pub type StagedDiff = (Oid, Arc<stage::FileDiff>);

/// A buffer's markers and the state of their computation.
#[derive(Debug, Default)]
pub struct QuickDiff {
    /// Sorted, in the buffer's current coordinates: the unstaged changes (buffer → index) and
    /// the staged ones (index → HEAD, tagged [`Mark::staged`]), the unstaged ranges cut out of
    /// the staged ones (unstaged wins).
    marks: Vec<Mark>,
    /// What the marks were computed from: the buffer's revision and the index blob (`None`: the
    /// file wasn't in the index, or not in a repository). `None` before the first computation.
    computed: Option<(u64, Option<Oid>)>,
    /// The staged half (HEAD → index) of the current marks, keyed by the index blob it was
    /// computed against. It doesn't depend on the buffer's text, so it is reused across
    /// recomputes while that blob is unchanged; `head_changed` forces it to be read again.
    staged: Option<StagedDiff>,
    /// The index may have changed since: compute again (cheap when the blob is the same).
    index_stale: bool,
    /// HEAD moved since: the staged marks may differ even where the index didn't, so the blob
    /// the marks came from doesn't make them current (cleared by `begin`).
    head_stale: bool,
    /// A computation is running: its generation, and the change sets applied since its
    /// snapshot, to map its result through.
    running: Option<(u64, Vec<Arc<ChangeSet>>)>,
    generation: u64,
}

/// Whether a buffer's markers need computing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    Nothing,
    /// Never computed, or the index changed: at once.
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
    /// The blob the current marks came from, when they are for `revision` and HEAD hasn't moved
    /// since: not diffed again.
    pub known: Option<Oid>,
    /// The staged half (HEAD → index) of the last computation, keyed by the index blob it came
    /// from: reused while that blob is unchanged (the buffer's text doesn't affect it).
    pub staged: Option<StagedDiff>,
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
    /// The file isn't in the index: no markers.
    NotTracked,
    /// The index blob is the one the marks came from, and the text is the same.
    Unchanged,
    Marks {
        index: Oid,
        marks: Vec<Mark>,
        /// The staged diff the marks were derived from (freshly read, or the reused cache), so
        /// `finish` keeps it for the next recompute.
        staged: Option<StagedDiff>,
    },
}

impl QuickDiff {
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// A computation is running (tests wait for it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// The buffer's text is `revision`: what needs doing.
    pub fn want(&self, revision: u64) -> Want {
        match self.computed {
            _ if self.running.is_some() => Want::Nothing,
            None => Want::Now,
            Some(_) if self.index_stale => Want::Now,
            Some((r, _)) if r != revision => Want::Later,
            Some(_) => Want::Nothing,
        }
    }

    /// The index changed (staging, a status refresh): compute again.
    pub fn index_changed(&mut self) {
        self.index_stale = true;
    }

    /// HEAD moved (a commit, a checkout, a reset): the staged marks read HEAD, so they are
    /// computed again even where the index blob is still the one they came from.
    pub fn head_changed(&mut self) {
        self.index_stale = true;
        self.head_stale = true;
    }

    /// No repository, or the file is outside it: no markers, nothing to compute until the
    /// index changes (a repository appearing refreshes the status, which calls `index_changed`).
    pub fn clear(&mut self, revision: u64) {
        self.marks.clear();
        self.computed = Some((revision, None));
        self.staged = None;
        self.index_stale = false;
        self.head_stale = false;
    }

    /// The buffer's file changed (Save As, a rename): the marks were for another path. Forgets
    /// them and the blob they came from, so the next turn computes afresh; a running
    /// computation's result (for the old path) is dropped.
    pub fn reset(&mut self) {
        *self = QuickDiff {
            generation: self.generation,
            ..Default::default()
        };
    }

    /// A computation starts from the text at `revision`: its generation, the job's `known`
    /// blob (none when HEAD moved: the staged marks read it too) and the cached staged diff
    /// (none when HEAD moved, so it is read again).
    pub fn begin(&mut self, revision: u64) -> (u64, Option<Oid>, Option<StagedDiff>) {
        self.generation += 1;
        self.running = Some((self.generation, Vec::new()));
        self.index_stale = false;
        let known = match self.head_stale {
            true => None,
            false => self.computed.filter(|(r, _)| *r == revision).and_then(|(_, id)| id),
        };
        // The staged half is recomputed only when HEAD moved since; otherwise the cached diff
        // is reused even though the buffer's revision advanced (typing doesn't affect it).
        // Clearing it here (not just returning None) matters when the recompute short-circuits
        // (a binary buffer): the stale HEAD diff must not survive into the next computation.
        if self.head_stale {
            self.staged = None;
        }
        let staged = self.staged.clone();
        self.head_stale = false;
        (self.generation, known, staged)
    }

    /// The running computation ended: its marks, mapped through the edits made since its
    /// snapshot, replace the current ones. A result of an older generation (reset since) is
    /// dropped.
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
                self.marks.clear();
                self.computed = Some((done.revision, None));
                self.staged = None;
            }
            Ok(Found::Unchanged) => {
                if let Some(c) = &mut self.computed {
                    c.0 = done.revision;
                }
            }
            Ok(Found::Marks {
                index,
                mut marks,
                staged,
            }) => {
                for set in &sets {
                    map_marks(&mut marks, set);
                }
                self.marks = marks;
                self.computed = Some((done.revision, Some(index)));
                if staged.is_some() {
                    self.staged = staged;
                }
            }
            Err(e) => {
                tracing::warn!("quick diff failed: {e}");
                self.computed = Some((done.revision, None));
            }
        }
        // Edits since the snapshot: `want` sees a newer revision and computes again later.
    }

    /// Follows a change set applied to the buffer (`Buffer::apply`).
    pub fn map(&mut self, changes: &Arc<ChangeSet>) {
        if let Some((_, sets)) = &mut self.running {
            sets.push(changes.clone());
        }
        map_marks(&mut self.marks, changes);
    }

    /// The markers on lines `first..=last`, by line (one per line: a bar wins over a deletion).
    /// The unstaged changes only: staging takes a change out of the working tree's diff.
    pub fn visible(&self, rope: &Rope, first: usize, last: usize) -> Vec<(usize, Marker)> {
        let from = pos::line_start(rope, first);
        // A deletion drawn on `last` sits at the next line's start.
        let to = match last + 1 {
            next if next < pos::line_count(rope) => pos::line_start(rope, next),
            _ => ByteIdx(rope.len()),
        };
        let i = self.marks.partition_point(|m| m.end < from);
        let mut out: Vec<(usize, Marker)> = Vec::new();
        for m in self.marks[i..]
            .iter()
            .take_while(|m| m.start <= to)
            .filter(|m| !m.staged)
        {
            let Some((lines, marker)) = drawn(rope, m) else {
                continue;
            };
            for line in lines.start.max(first)..lines.end.min(last + 1) {
                match out.binary_search_by_key(&line, |(l, _)| *l) {
                    Ok(k) if matches!(marker, Marker::Added | Marker::Modified) => out[k].1 = marker,
                    Ok(_) => {}
                    Err(k) => out.insert(k, (line, marker)),
                }
            }
        }
        out
    }

    /// The mark drawn on `line` (a bar before a deletion), with its index; unstaged changes only.
    pub fn at_line(&self, rope: &Rope, line: usize) -> Option<(usize, &Mark)> {
        let from = pos::line_start(rope, line.min(pos::line_count(rope) - 1));
        let i = self.marks.partition_point(|m| m.end < from);
        let mut deletion = None;
        for (k, m) in self.marks.iter().enumerate().skip(i).filter(|(_, m)| !m.staged) {
            let Some((lines, marker)) = drawn(rope, m) else {
                continue;
            };
            if lines.start > line {
                break;
            }
            if lines.contains(&line) {
                match marker {
                    Marker::Added | Marker::Modified => return Some((k, m)),
                    _ => deletion = deletion.or(Some((k, m))),
                }
            }
        }
        deletion
    }

    /// The first line of the next change after `line` (forward) or the last one starting before
    /// it, wrapping around; `None` without changes. Unstaged changes only.
    pub fn step(&self, rope: &Rope, line: usize, forward: bool) -> Option<usize> {
        let firsts: Vec<usize> = self
            .marks
            .iter()
            .filter(|m| !m.staged)
            .filter_map(|m| drawn(rope, m).map(|(l, _)| l.start))
            .collect();
        if forward {
            firsts.iter().find(|&&l| l > line).or(firsts.first()).copied()
        } else {
            firsts.iter().rev().find(|&&l| l < line).or(firsts.last()).copied()
        }
    }

    /// The lines a mark is drawn on.
    pub fn lines_of(&self, rope: &Rope, mark: &Mark) -> Option<Range<usize>> {
        drawn(rope, mark).map(|(l, _)| l)
    }
}

/// The lines a mark is drawn on and how: a bar over its lines (to the line holding `end`
/// unless `end` starts a line), a deletion below the line before it or above line 0. An added or
/// modified mark whose text was all deleted isn't drawn.
fn drawn(rope: &Rope, m: &Mark) -> Option<(Range<usize>, Marker)> {
    let first = pos::line_of(rope, m.start);
    match m.kind {
        HunkKind::Deleted if first == 0 && m.start == ByteIdx(0) => Some((0..1, Marker::DeletedAbove)),
        HunkKind::Deleted => {
            let line = if pos::line_start(rope, first) == m.start {
                first - 1
            } else {
                first
            };
            Some((line..line + 1, Marker::DeletedBelow))
        }
        _ if m.start == m.end => None,
        kind => {
            // The end's line via two O(log n) lookups, not a scan of the mark's bytes (a
            // whole-file replacement would be O(hunk size) per drawn frame). A mark ending on a
            // line start covers up to the line before `m.end`.
            let last = pos::line_of(rope, m.end);
            let end = if pos::line_start(rope, last) == m.end {
                last
            } else {
                last + 1
            };
            let marker = if kind == HunkKind::Added {
                Marker::Added
            } else {
                Marker::Modified
            };
            Some((first..end.max(first + 1), marker))
        }
    }
}

/// Maps marks (sorted, so their positions come in order) through a change set: text typed at
/// either edge of a changed range stays outside it, a deleted one becomes a point.
fn map_marks(marks: &mut [Mark], changes: &ChangeSet) {
    if marks.is_empty() || changes.is_empty() {
        return;
    }
    let mapper = changes.mapper();
    for m in marks.iter_mut() {
        // A deletion stays below the line before it, whatever is typed at its line's start.
        let assoc = if m.kind == HunkKind::Deleted {
            Assoc::Before
        } else {
            Assoc::After
        };
        m.start = mapper.map(m.start, assoc);
        m.end = mapper.map(m.end, Assoc::Before).max(m.start);
    }
}

/// Blocking: the markers of `job`'s text against the file's index blob.
pub fn compute(job: Job) -> Computed {
    Computed {
        revision: job.revision,
        generation: job.generation,
        result: find(&job).map_err(|e| e.to_string()),
    }
}

fn find(job: &Job) -> Result<Found, stage::Error> {
    let (index, text) = match stage::index_blob(&job.repo, &job.path, job.known)? {
        IndexBlob::Missing => return Ok(Found::NotTracked),
        IndexBlob::Same => return Ok(Found::Unchanged),
        IndexBlob::Text(id, text) => (id, text.bytes),
    };
    let ours = crate::buffer::file::encode(&job.rope, job.bom);
    let skip = if job.bom { 3 } else { 0 };
    let unstaged_diff = diff::diff(&text, &ours, 0)?;
    let unstaged = line_marks(&unstaged_diff, &text, &ours, skip);
    let (staged, staged_diff) = staged_marks(job, index, &ours, skip, &unstaged_diff)?;
    Ok(Found::Marks {
        index,
        marks: merge_staged(unstaged, staged),
        staged: staged_diff,
    })
}

/// The unstaged marks: each hunk's changed buffer lines as byte ranges (none for a binary text).
fn line_marks(d: &diff::Diff, old: &[u8], new: &[u8], skip: usize) -> Vec<Mark> {
    if d.binary {
        return Vec::new();
    }
    let old_lines = diff::lines(old);
    let starts = line_starts(new);
    d.hunks
        .iter()
        .map(|h| Mark {
            kind: h.kind(),
            staged: false,
            start: line_at(&starts, h.new.start, skip),
            end: line_at(&starts, h.new.end, skip),
            removed: removed_lines(&old_lines, &h.old),
            removed_count: h.old.len(),
        })
        .collect()
}

/// The staged marks: the index → HEAD diff's changed index lines, mapped into the buffer through
/// the index → buffer diff (`unstaged`: what the working tree did to them) and tagged
/// [`Mark::staged`]. The mapping takes each end line's *start*, so a change covers the buffer
/// lines its index lines stand for and nothing more. The staged diff itself (HEAD → index)
/// depends only on HEAD and the index, so it is returned alongside the marks (reused across
/// recomputes while the index blob is unchanged) rather than read again each time.
fn staged_marks(
    job: &Job,
    index: Oid,
    ours: &[u8],
    skip: usize,
    unstaged: &diff::Diff,
) -> Result<(Vec<Mark>, Option<StagedDiff>), stage::Error> {
    if unstaged.binary {
        return Ok((Vec::new(), None));
    }
    // Reuse the cached staged diff when it is for this index blob; else read it again.
    let staged_diff = match job.staged.as_ref().filter(|(oid, _)| *oid == index) {
        Some((_, d)) => d.clone(),
        None => {
            #[cfg(test)]
            {
                STAGED_DIFFS.with(|n| n.set(n.get() + 1));
            }
            Arc::new(stage::file_diff(&job.repo, &job.path, stage::Side::Staged, 0)?)
        }
    };
    if staged_diff.diff.binary {
        return Ok((Vec::new(), Some((index, staged_diff))));
    }
    let head_lines = diff::lines(&staged_diff.old);
    let starts = line_starts(ours);
    Ok((
        staged_diff
            .diff
            .hunks
            .iter()
            .map(|h| {
                let start = line_at(&starts, diff::new_lines(&unstaged.hunks, h.new.start).start, skip);
                let end = line_at(&starts, diff::new_lines(&unstaged.hunks, h.new.end).start, skip);
                Mark {
                    kind: h.kind(),
                    staged: true,
                    start,
                    end: end.max(start),
                    removed: removed_lines(&head_lines, &h.old),
                    removed_count: h.old.len(),
                }
            })
            .collect(),
        Some((index, staged_diff)),
    ))
}

// Test probe: how many times [`staged_marks`] read the staged half from git on this thread.
#[cfg(test)]
thread_local! {
    static STAGED_DIFFS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn staged_diff_count() -> usize {
    STAGED_DIFFS.with(|n| n.get())
}

#[cfg(test)]
fn reset_staged_diff_count() {
    STAGED_DIFFS.with(|n| n.set(0));
}

/// The unstaged marks with the staged ones merged in: a line staged and not edited since stays
/// staged, one edited in the working tree too shows unstaged (each staged range loses the
/// unstaged ranges it meets), sorted by position.
fn merge_staged(unstaged: Vec<Mark>, mut staged: Vec<Mark>) -> Vec<Mark> {
    if staged.is_empty() {
        return unstaged;
    }
    let mut cover: Vec<(ByteIdx, ByteIdx)> = unstaged.iter().map(|m| (m.start, m.end)).collect();
    cover.sort();
    staged.sort_by_key(|m| (m.start, m.end));
    let mut out = unstaged;
    // Both are sorted: walk the cover once for all the staged marks.
    let mut first = 0;
    for m in staged {
        while first < cover.len() && cover[first].1 < m.start {
            first += 1;
        }
        for (start, end) in uncovered(&cover[first..], m.start, m.end) {
            out.push(Mark {
                start,
                end,
                ..m.clone()
            });
        }
    }
    out.sort_by_key(|m| (m.start, m.end));
    out
}

/// `[start,end)` with the sorted ranges `cover` cut out of it. A point (a deletion) survives
/// unless a range contains it or another point sits on it.
fn uncovered(cover: &[(ByteIdx, ByteIdx)], start: ByteIdx, end: ByteIdx) -> Vec<(ByteIdx, ByteIdx)> {
    if start == end {
        let covered = cover
            .iter()
            .take_while(|(s, _)| *s <= start)
            .any(|&(s, e)| if s == e { s == start } else { start < e });
        return if covered { Vec::new() } else { vec![(start, end)] };
    }
    let mut out = Vec::new();
    let mut at = start;
    for &(s, e) in cover {
        if e <= at {
            continue;
        }
        if s >= end {
            break;
        }
        if s > at {
            out.push((at, s));
        }
        at = at.max(e);
        if at >= end {
            break;
        }
    }
    if at < end {
        out.push((at, end));
    }
    out
}

/// Each line's start in `text`, then the text's end (so line `n`, past the last, is the end).
fn line_starts(text: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        text.iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1),
    );
    if starts.last() == Some(&text.len()) && !text.is_empty() {
        starts.pop();
    }
    starts.push(text.len());
    starts
}

/// The buffer position of `line` of a text with `starts` (clamped to its end), without the BOM's
/// `skip` bytes at its start.
fn line_at(starts: &[usize], line: usize, skip: usize) -> ByteIdx {
    ByteIdx(starts[line.min(starts.len() - 1)].saturating_sub(skip))
}

/// A hunk's old lines as a mark's `removed`: at most `MAX_REMOVED`, line breaks dropped, invalid
/// UTF-8 replaced.
fn removed_lines(lines: &[&[u8]], range: &Range<usize>) -> Arc<[String]> {
    lines[range.start.min(lines.len())..range.end.min(lines.len())]
        .iter()
        .take(MAX_REMOVED)
        .map(|l| {
            let l = l.strip_suffix(b"\n").unwrap_or(l);
            String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)).into_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::change::Change;

    fn marks_of(old: &str, new: &str) -> Vec<Mark> {
        marks(old.as_bytes(), new.as_bytes(), 0)
    }

    /// The marks from the index text `old` to the buffer's bytes `new` (`skip`: the BOM's length
    /// at its start, which the buffer's positions don't count).
    fn marks(old: &[u8], new: &[u8], skip: usize) -> Vec<Mark> {
        let d = diff::diff(old, new, 0).unwrap();
        line_marks(&d, old, new, skip)
    }

    fn qd(old: &str, new: &str) -> QuickDiff {
        QuickDiff {
            marks: marks_of(old, new),
            computed: Some((0, None)),
            ..Default::default()
        }
    }

    #[test]
    fn marks_are_byte_ranges_of_the_changed_lines() {
        let m = marks_of("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        assert_eq!(m.len(), 2);
        assert_eq!(
            (m[0].kind, m[0].start, m[0].end),
            (HunkKind::Modified, ByteIdx(2), ByteIdx(4))
        );
        assert_eq!(&*m[0].removed, ["b".to_string()]);
        assert_eq!(
            (m[1].kind, m[1].start, m[1].end),
            (HunkKind::Added, ByteIdx(8), ByteIdx(10))
        );
        // A deletion is a point at the start of the line after it.
        let m = marks_of("a\nb\nc\n", "a\nc\n");
        assert_eq!(
            (m[0].kind, m[0].start, m[0].end),
            (HunkKind::Deleted, ByteIdx(2), ByteIdx(2))
        );
        // The last line without a line break; CRLF lines.
        let m = marks_of("a\nb", "a\nB");
        assert_eq!((m[0].start, m[0].end), (ByteIdx(2), ByteIdx(3)));
        let m = marks_of("a\r\nb\r\n", "a\r\nB\r\n");
        assert_eq!((m[0].start, m[0].end), (ByteIdx(3), ByteIdx(6)));
        assert_eq!(&*m[0].removed, ["b".to_string()]);
        // A BOM in the file's bytes isn't in the buffer's positions.
        let m = marks("\u{feff}a\nb\n".as_bytes(), "\u{feff}a\nB\n".as_bytes(), 3);
        assert_eq!((m[0].start, m[0].end), (ByteIdx(2), ByteIdx(4)));
    }

    #[test]
    fn visible_markers_by_line() {
        let new = "a\nB\nc\nd\nx\ny\n";
        let q = qd("a\nb\nc\nd\n", new);
        let rope = Rope::from_str(new);
        assert_eq!(
            q.visible(&rope, 0, 6),
            vec![(1, Marker::Modified), (4, Marker::Added), (5, Marker::Added)]
        );
        assert_eq!(q.visible(&rope, 5, 6), vec![(5, Marker::Added)]);
        // Deletions: below the line before, above line 0.
        let new = "b\nc\nd\n";
        let q = qd("a\nb\nc\nX\nd\n", new);
        let rope = Rope::from_str(new);
        assert_eq!(
            q.visible(&rope, 0, 3),
            vec![(0, Marker::DeletedAbove), (1, Marker::DeletedBelow)]
        );
        assert_eq!(q.at_line(&rope, 1).map(|(k, _)| k), Some(1));
        assert_eq!(q.at_line(&rope, 2), None);
        // A deletion below a modified line: the bar wins, both are found on the line.
        let new = "a\nB\nd\n";
        let q = qd("a\nb\nc\nd\n", new);
        let rope = Rope::from_str(new);
        assert_eq!(q.visible(&rope, 0, 3), vec![(1, Marker::Modified)]);
    }

    #[test]
    fn marks_follow_edits_and_results_map_through_edits_made_meanwhile() {
        let new = "a\nB\nc\n";
        let mut q = qd("a\nb\nc\n", new);
        let mut rope = Rope::from_str(new);
        // Two lines typed above: the mark moves down.
        let set = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(0),
            end: ByteIdx(0),
            text: "x\ny\n".into(),
        }]));
        set.apply(&mut rope);
        q.map(&set);
        assert_eq!(q.visible(&rope, 0, 5), vec![(3, Marker::Modified)]);
        // A computation started at revision 1 ends after another edit: mapped through it.
        assert_eq!(q.want(1), Want::Later);
        let (generation, _, _) = q.begin(1);
        assert_eq!(q.want(1), Want::Nothing);
        let snapshot = rope.clone();
        let set = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(0),
            end: ByteIdx(0),
            text: "z\n".into(),
        }]));
        set.apply(&mut rope);
        q.map(&set);
        let marks = marks(b"a\nb\nc\n", snapshot.to_string().as_bytes(), 0);
        q.finish(Computed {
            revision: 1,
            generation,
            result: Ok(Found::Marks {
                index: Oid::ZERO_SHA1,
                marks,
                staged: None,
            }),
        });
        assert_eq!(
            q.visible(&rope, 0, 7),
            vec![(1, Marker::Added), (2, Marker::Added), (4, Marker::Modified)]
        );
        assert_eq!(q.want(2), Want::Later, "edited since the snapshot");
        assert_eq!(q.want(1), Want::Nothing);
        q.index_changed();
        assert_eq!(q.want(1), Want::Now);
    }

    #[test]
    fn a_reset_drops_the_running_computations_result() {
        // A computation for the old path is running when the buffer's file changes.
        let mut q = qd("a\nb\n", "a\nB\n");
        let (old, _, _) = q.begin(0);
        q.reset();
        assert!(q.marks().is_empty() && !q.running());
        assert_eq!(q.want(0), Want::Now, "computed afresh for the new path");
        let (new, known, _) = q.begin(0);
        assert!(new != old && known.is_none());
        let found = |text: &[u8]| Found::Marks {
            index: Oid::ZERO_SHA1,
            marks: marks(text, b"a\nB\n", 0),
            staged: None,
        };
        // The old path's result arrives late: dropped, the new one still runs.
        q.finish(Computed {
            revision: 0,
            generation: old,
            result: Ok(found(b"a\nb\n")),
        });
        assert!(q.marks().is_empty() && q.running());
        q.finish(Computed {
            revision: 0,
            generation: new,
            result: Ok(Found::NotTracked),
        });
        assert!(q.marks().is_empty() && !q.running());
        assert_eq!(q.want(0), Want::Nothing);
    }

    fn insert(q: &mut QuickDiff, rope: &mut Rope, at: usize, text: &str) {
        let set = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(at),
            end: ByteIdx(at),
            text: text.into(),
        }]));
        set.apply(rope);
        q.map(&set);
    }

    #[test]
    fn text_typed_at_a_changes_edges_stays_outside_it() {
        // Line 1 modified, line "d" deleted below line 2.
        let new = "a\nB\nc\ne\n";
        let mut q = qd("a\nb\nc\nd\ne\n", new);
        let mut rope = Rope::from_str(new);
        assert_eq!(
            q.visible(&rope, 0, 4),
            vec![(1, Marker::Modified), (2, Marker::DeletedBelow)]
        );
        // A line typed at the start of the line after the modified one, and at the start of the
        // line after the deletion: neither joins the change; the deletion stays below "c".
        insert(&mut q, &mut rope, 6, "new\n");
        assert_eq!(rope.to_string(), "a\nB\nc\nnew\ne\n");
        assert_eq!(
            q.visible(&rope, 0, 5),
            vec![(1, Marker::Modified), (2, Marker::DeletedBelow)]
        );
        insert(&mut q, &mut rope, 4, "x\n");
        assert_eq!(
            q.visible(&rope, 0, 6),
            vec![(1, Marker::Modified), (3, Marker::DeletedBelow)]
        );
        // Typed at the modified line's start: outside too (the mark moves along the line).
        insert(&mut q, &mut rope, 2, "\n");
        assert_eq!(
            q.visible(&rope, 0, 7),
            vec![(2, Marker::Modified), (4, Marker::DeletedBelow)]
        );
    }

    #[test]
    fn a_bar_wins_over_a_deletion_on_its_line() {
        // As edits can leave them: a modified line 1 and a deletion drawn below it.
        let rope = Rope::from_str("a\nB\nc\n");
        let mark = |kind, start, end| Mark {
            kind,
            staged: false,
            start: ByteIdx(start),
            end: ByteIdx(end),
            removed: Arc::new([]),
            removed_count: 1,
        };
        let q = QuickDiff {
            marks: vec![mark(HunkKind::Modified, 2, 4), mark(HunkKind::Deleted, 4, 4)],
            computed: Some((0, None)),
            ..Default::default()
        };
        assert_eq!(q.visible(&rope, 0, 2), vec![(1, Marker::Modified)]);
        assert_eq!(q.at_line(&rope, 1).map(|(k, _)| k), Some(0));
        assert_eq!(q.at_line(&rope, 2), None);
        // The deletion first (mid-line, as mapping can leave it): the bar still wins.
        let q = QuickDiff {
            marks: vec![mark(HunkKind::Deleted, 3, 3), mark(HunkKind::Modified, 3, 4)],
            computed: Some((0, None)),
            ..Default::default()
        };
        assert_eq!(q.visible(&rope, 0, 2), vec![(1, Marker::Modified)]);
        assert_eq!(q.at_line(&rope, 1).map(|(k, _)| k), Some(1));
    }

    #[test]
    fn steps_between_changes_wrap() {
        let new = "a\nB\nc\nd\nE\nf\n";
        let q = qd("a\nb\nc\nd\ne\nf\n", new);
        let rope = Rope::from_str(new);
        assert_eq!(q.step(&rope, 0, true), Some(1));
        assert_eq!(q.step(&rope, 1, true), Some(4));
        assert_eq!(q.step(&rope, 4, true), Some(1), "wraps");
        assert_eq!(q.step(&rope, 4, false), Some(1));
        assert_eq!(q.step(&rope, 1, false), Some(4), "wraps");
        assert_eq!(q.step(&rope, 5, false), Some(4));
        assert_eq!(QuickDiff::default().step(&rope, 0, true), None);
    }

    /// A repository in a temp dir with `f.txt` holding `committed` in HEAD.
    fn test_repo(committed: &str) -> (tempfile::TempDir, Repo) {
        use crate::git::test_repo::{commit_all, init};
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("a");
        std::fs::create_dir(&root).unwrap();
        init(&root);
        std::fs::write(root.join("f.txt"), committed).unwrap();
        commit_all(&root, "one");
        let repo = Repo::discover(&root).unwrap();
        (dir, repo)
    }

    /// A computation's job for `f.txt` of `repo`, the buffer holding `text`.
    fn job_for(repo: &Repo, text: &str) -> Job {
        Job {
            repo: repo.clone(),
            path: PathBuf::from("f.txt"),
            rope: Rope::from_str(text),
            bom: false,
            revision: 0,
            generation: 0,
            known: None,
            staged: None,
        }
    }

    /// `(start, end, staged)` for each mark.
    fn ranges(marks: &[Mark]) -> Vec<(usize, usize, bool)> {
        marks.iter().map(|m| (m.start.0, m.end.0, m.staged)).collect()
    }

    /// A quick diff holding `marks`, computed for the buffer as it is.
    fn qd_with(marks: Vec<Mark>) -> QuickDiff {
        QuickDiff {
            marks,
            computed: Some((0, None)),
            ..Default::default()
        }
    }

    #[test]
    fn staged_marks_are_tagged_and_unstaged_wins_where_both_changed() {
        use crate::git::test_repo::git;
        let (_d, repo) = test_repo("a\nb\nc\nd\ne\n");
        // Staged: "b" -> "B", "d" -> "D".
        std::fs::write(repo.workdir.join("f.txt"), "a\nB\nc\nD\ne\n").unwrap();
        git(&repo.workdir, &["add", "f.txt"]);
        // The working tree edits line 1 again (the staged line: unstaged wins) and line 4.
        let text = "a\nBB\nc\nD\nE\n";
        std::fs::write(repo.workdir.join("f.txt"), text).unwrap();
        let Found::Marks { marks, .. } = find(&job_for(&repo, text)).unwrap() else {
            panic!("not marked");
        };
        assert_eq!(ranges(&marks), vec![(2, 5, false), (7, 9, true), (9, 11, false)]);
        assert!(marks.iter().all(|m| m.kind == HunkKind::Modified));
        // Staged marks are not the gutter's: only the working tree's changes are.
        let rope = Rope::from_str(text);
        let q = qd_with(marks);
        assert_eq!(
            q.visible(&rope, 0, 4),
            vec![(1, Marker::Modified), (4, Marker::Modified)]
        );
        assert_eq!(q.at_line(&rope, 1).map(|(_, m)| m.staged), Some(false));
        assert_eq!(q.at_line(&rope, 3), None, "line 3 is staged only");
        assert_eq!(q.step(&rope, 1, true), Some(4), "the staged line isn't a change here");
    }

    #[test]
    fn a_staged_change_is_marked_staged_and_stays_out_of_the_gutter() {
        use crate::git::test_repo::git;
        let (_d, repo) = test_repo("a\nb\nc\n");
        std::fs::write(repo.workdir.join("f.txt"), "a\nB\nc\n").unwrap();
        git(&repo.workdir, &["add", "f.txt"]);
        let text = "a\nB\nc\n";
        let Found::Marks { marks, .. } = find(&job_for(&repo, text)).unwrap() else {
            panic!("not marked");
        };
        assert_eq!(ranges(&marks), vec![(2, 4, true)]);
        assert_eq!(&*marks[0].removed, ["b".to_string()], "HEAD's line");
        let rope = Rope::from_str(text);
        let q = qd_with(marks);
        assert!(q.visible(&rope, 0, 2).is_empty());
        assert_eq!(q.at_line(&rope, 1), None);
        assert_eq!(q.step(&rope, 0, true), None);
    }

    #[test]
    fn a_staged_deletion_is_a_point_in_the_buffers_lines() {
        use crate::git::test_repo::git;
        let (_d, repo) = test_repo("a\nb\nc\n");
        // Staged: "b" deleted; the working tree is the index.
        std::fs::write(repo.workdir.join("f.txt"), "a\nc\n").unwrap();
        git(&repo.workdir, &["add", "f.txt"]);
        let text = "a\nc\n";
        let Found::Marks { marks, .. } = find(&job_for(&repo, text)).unwrap() else {
            panic!("not marked");
        };
        assert_eq!(ranges(&marks), vec![(2, 2, true)]);
        assert_eq!(marks[0].kind, HunkKind::Deleted);
        assert_eq!(&*marks[0].removed, ["b".to_string()]);
    }

    #[test]
    fn a_staged_addition_edited_in_the_working_tree_splits_into_both_kinds() {
        use crate::git::test_repo::git;
        let (_d, repo) = test_repo("a\n");
        // Staged: two lines added after "a".
        std::fs::write(repo.workdir.join("f.txt"), "a\nx\ny\n").unwrap();
        git(&repo.workdir, &["add", "f.txt"]);
        // Then the first of them edited: the staged range keeps only "y".
        let text = "a\nX\ny\n";
        std::fs::write(repo.workdir.join("f.txt"), text).unwrap();
        let Found::Marks { marks, .. } = find(&job_for(&repo, text)).unwrap() else {
            panic!("not marked");
        };
        assert_eq!(ranges(&marks), vec![(2, 4, false), (4, 6, true)]);
        assert_eq!((marks[0].kind, marks[1].kind), (HunkKind::Modified, HunkKind::Added));
    }

    #[test]
    fn a_newly_staged_file_is_marked_staged_throughout() {
        use crate::git::test_repo::git;
        let (_d, repo) = test_repo("a\n");
        // A brand-new file, staged but never committed: HEAD has none of it, the index all of it.
        std::fs::write(repo.workdir.join("new.txt"), "x\ny\n").unwrap();
        git(&repo.workdir, &["add", "new.txt"]);
        let job = Job {
            repo: repo.clone(),
            path: PathBuf::from("new.txt"),
            rope: Rope::from_str("x\ny\n"),
            bom: false,
            revision: 0,
            generation: 0,
            known: None,
            staged: None,
        };
        let Found::Marks { marks, .. } = find(&job).unwrap() else {
            panic!("not marked");
        };
        assert_eq!(ranges(&marks), vec![(0, 4, true)]);
        assert!(marks.iter().all(|m| m.kind == HunkKind::Added && m.staged));
        assert!(
            marks[0].removed.is_empty() && marks[0].removed_count == 0,
            "HEAD had no line"
        );
    }

    #[test]
    fn a_staged_change_maps_through_working_tree_insertions_above_it() {
        use crate::git::test_repo::git;
        let (_d, repo) = test_repo("a\nb\nc\nd\n");
        // Staged: "c" -> "C".
        std::fs::write(repo.workdir.join("f.txt"), "a\nb\nC\nd\n").unwrap();
        git(&repo.workdir, &["add", "f.txt"]);
        // The working tree then inserts "z" two lines above it: the staged line is buffer line 3,
        // not 2. The unchanged "b" between them is not covered by any unstaged mark, so a
        // mapping that ignores the insertion would leave the staged mark spanning it.
        let text = "a\nz\nb\nC\nd\n";
        std::fs::write(repo.workdir.join("f.txt"), text).unwrap();
        let Found::Marks { marks, .. } = find(&job_for(&repo, text)).unwrap() else {
            panic!("not marked");
        };
        assert_eq!(ranges(&marks), vec![(2, 4, false), (6, 8, true)]);
        assert_eq!((marks[0].kind, marks[1].kind), (HunkKind::Added, HunkKind::Modified));
    }

    #[test]
    fn theme_keys_are_the_gutters_or_its_darker_staged_siblings() {
        let mark = |kind, staged| Mark {
            kind,
            staged,
            start: ByteIdx(0),
            end: ByteIdx(0),
            removed: Arc::new([]),
            removed_count: 0,
        };
        assert_eq!(
            [
                mark(HunkKind::Added, false).theme_key(),
                mark(HunkKind::Modified, false).theme_key(),
                mark(HunkKind::Deleted, false).theme_key(),
            ],
            [
                "editorGutter.addedBackground",
                "editorGutter.modifiedBackground",
                "editorGutter.deletedBackground"
            ]
        );
        assert_eq!(
            [
                mark(HunkKind::Added, true).theme_key(),
                mark(HunkKind::Modified, true).theme_key(),
                mark(HunkKind::Deleted, true).theme_key(),
            ],
            [
                "editorGutter.stagedAddedBackground",
                "editorGutter.stagedModifiedBackground",
                "editorGutter.stagedDeletedBackground"
            ]
        );
    }

    #[test]
    fn merge_staged_cuts_staged_ranges_by_unstaged_ones() {
        let mark = |staged: bool, start: usize, end: usize, kind: HunkKind| Mark {
            kind,
            staged,
            start: ByteIdx(start),
            end: ByteIdx(end),
            removed: Arc::new([]),
            removed_count: 1,
        };
        // Staged only: kept as it is.
        let merged = merge_staged(
            vec![mark(false, 10, 20, HunkKind::Modified)],
            vec![mark(true, 30, 40, HunkKind::Added)],
        );
        assert_eq!(ranges(&merged), vec![(10, 20, false), (30, 40, true)]);
        // Staged and edited too: the unstaged mark wins.
        let merged = merge_staged(
            vec![mark(false, 10, 20, HunkKind::Modified)],
            vec![mark(true, 10, 20, HunkKind::Modified)],
        );
        assert_eq!(ranges(&merged), vec![(10, 20, false)]);
        // A partial overlap splits the staged range; its pieces keep its kind.
        let merged = merge_staged(
            vec![mark(false, 15, 18, HunkKind::Modified)],
            vec![mark(true, 10, 25, HunkKind::Added)],
        );
        assert_eq!(ranges(&merged), vec![(10, 15, true), (15, 18, false), (18, 25, true)]);
        assert!(merged[0].kind == HunkKind::Added && merged[2].kind == HunkKind::Added);
        // A deletion is a point: cut by a range over it, or by another point on it.
        let merged = merge_staged(
            vec![mark(false, 10, 20, HunkKind::Modified)],
            vec![mark(true, 15, 15, HunkKind::Deleted)],
        );
        assert_eq!(ranges(&merged), vec![(10, 20, false)]);
        let merged = merge_staged(
            vec![mark(false, 15, 15, HunkKind::Deleted)],
            vec![mark(true, 15, 15, HunkKind::Deleted)],
        );
        assert_eq!(ranges(&merged), vec![(15, 15, false)]);
        let merged = merge_staged(vec![], vec![mark(true, 15, 15, HunkKind::Deleted)]);
        assert_eq!(ranges(&merged), vec![(15, 15, true)]);
        // Several ranges on both sides: the result is sorted and non-overlapping.
        let merged = merge_staged(
            vec![
                mark(false, 30, 40, HunkKind::Modified),
                mark(false, 0, 10, HunkKind::Modified),
            ],
            vec![
                mark(true, 5, 35, HunkKind::Modified),
                mark(true, 45, 50, HunkKind::Added),
            ],
        );
        assert_eq!(
            ranges(&merged),
            vec![(0, 10, false), (10, 30, true), (30, 40, false), (45, 50, true)]
        );
        let sorted = merged.windows(2).all(|w| w[0].end <= w[1].start);
        assert!(sorted, "sorted, non-overlapping: {merged:?}");
    }

    #[test]
    fn head_changed_computes_both_diffs_again_even_for_the_same_index_blob() {
        let mut q = QuickDiff {
            marks: marks_of("a\nb\n", "a\nB\n"),
            computed: Some((1, Some(Oid::ZERO_SHA1))),
            ..Default::default()
        };
        assert_eq!(q.want(1), Want::Nothing);
        q.head_changed();
        assert_eq!(q.want(1), Want::Now);
        let (generation, known, _) = q.begin(1);
        assert!(
            known.is_none(),
            "the index blob is read again (the staged diff needs HEAD)"
        );
        q.finish(Computed {
            revision: 1,
            generation,
            result: Ok(Found::Unchanged),
        });
        assert!(!q.running());
        assert_eq!(q.want(1), Want::Nothing);
        // HEAD moving while a computation runs: its result is stale, compute again.
        let (generation, _, _) = q.begin(1);
        q.head_changed();
        q.finish(Computed {
            revision: 1,
            generation,
            result: Ok(Found::Marks {
                index: Oid::ZERO_SHA1,
                marks: marks_of("a\nb\n", "a\nB\n"),
                staged: None,
            }),
        });
        assert_eq!(q.want(1), Want::Now);
        let (_, known, _) = q.begin(1);
        assert!(known.is_none());
    }

    #[test]
    fn typing_after_a_compute_reuses_the_staged_half_while_marks_update() {
        use crate::git::test_repo::git;
        let (_d, repo) = test_repo("a\nb\nc\n");
        // Stage a change, so the file has a staged half (HEAD → index).
        std::fs::write(repo.workdir.join("f.txt"), "a\nB\nc\n").unwrap();
        git(&repo.workdir, &["add", "f.txt"]);

        let path = PathBuf::from("f.txt");
        let job = |revision: u64, generation: u64, known: Option<Oid>, staged, rope: &Rope| Job {
            repo: repo.clone(),
            path: path.clone(),
            rope: rope.clone(),
            bom: false,
            revision,
            generation,
            known,
            staged,
        };

        let mut q = QuickDiff::default();
        reset_staged_diff_count();

        // The first computation (the buffer equals the index) reads the staged half once.
        let rope0 = Rope::from_str("a\nB\nc\n");
        let (gen0, known0, staged0) = q.begin(0);
        assert!(known0.is_none() && staged0.is_none(), "nothing cached yet");
        q.finish(compute(job(0, gen0, known0, staged0, &rope0)));
        assert_eq!(staged_diff_count(), 1, "the staged half was read once");
        assert!(q.marks().iter().any(|m| m.staged), "the staged change is marked staged");

        // Typing changes the buffer's revision but not the index or HEAD: the staged half is
        // reused, the unstaged half recomputed against the new text.
        let rope1 = Rope::from_str("a\nB\nc\nx\n");
        let (gen1, known1, staged1) = q.begin(1);
        assert!(known1.is_none(), "the index blob is read again (the text changed)");
        assert!(staged1.is_some(), "the staged half is reused across the edit");
        q.finish(compute(job(1, gen1, known1, staged1, &rope1)));
        assert_eq!(staged_diff_count(), 1, "the staged half was not recomputed");
        assert_eq!(
            q.visible(&rope1, 0, 4),
            vec![(3, Marker::Added)],
            "the typed line is a new unstaged change; the staged one stays out of the gutter"
        );
    }
}
