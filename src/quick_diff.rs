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
    pub start: ByteIdx,
    pub end: ByteIdx,
    /// The index text's lines the change replaced (at most `MAX_REMOVED`, line breaks dropped,
    /// invalid UTF-8 replaced), and how many there were.
    pub removed: Arc<[String]>,
    pub removed_count: usize,
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

/// A buffer's markers and the state of their computation.
#[derive(Debug, Default)]
pub struct QuickDiff {
    /// Sorted, non-overlapping, in the buffer's current coordinates.
    marks: Vec<Mark>,
    /// What the marks were computed from: the buffer's revision and the index blob (`None`: the
    /// file wasn't in the index, or not in a repository). `None` before the first computation.
    computed: Option<(u64, Option<Oid>)>,
    /// The index may have changed since: compute again (cheap when the blob is the same).
    index_stale: bool,
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
    /// The blob the current marks came from, when they are for `revision`: not diffed again.
    pub known: Option<Oid>,
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
    },
}

impl QuickDiff {
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// No repository, or the file is outside it: no markers, nothing to compute until the
    /// index changes (a repository appearing refreshes the status, which calls `index_changed`).
    pub fn clear(&mut self, revision: u64) {
        self.marks.clear();
        self.computed = Some((revision, None));
        self.index_stale = false;
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

    /// A computation starts from the text at `revision`: its generation and the job's `known`
    /// blob.
    pub fn begin(&mut self, revision: u64) -> (u64, Option<Oid>) {
        self.generation += 1;
        self.running = Some((self.generation, Vec::new()));
        self.index_stale = false;
        let known = self.computed.filter(|(r, _)| *r == revision).and_then(|(_, id)| id);
        (self.generation, known)
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
            }
            Ok(Found::Unchanged) => {
                if let Some(c) = &mut self.computed {
                    c.0 = done.revision;
                }
            }
            Ok(Found::Marks { index, mut marks }) => {
                for set in &sets {
                    map_marks(&mut marks, set);
                }
                self.marks = marks;
                self.computed = Some((done.revision, Some(index)));
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
    pub fn visible(&self, rope: &Rope, first: usize, last: usize) -> Vec<(usize, Marker)> {
        let from = pos::line_start(rope, first);
        // A deletion drawn on `last` sits at the next line's start.
        let to = match last + 1 {
            next if next < pos::line_count(rope) => pos::line_start(rope, next),
            _ => ByteIdx(rope.len()),
        };
        let i = self.marks.partition_point(|m| m.end < from);
        let mut out: Vec<(usize, Marker)> = Vec::new();
        for m in self.marks[i..].iter().take_while(|m| m.start <= to) {
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

    /// The mark drawn on `line` (a bar before a deletion), with its index.
    pub fn at_line(&self, rope: &Rope, line: usize) -> Option<(usize, &Mark)> {
        let from = pos::line_start(rope, line.min(pos::line_count(rope) - 1));
        let i = self.marks.partition_point(|m| m.end < from);
        let mut deletion = None;
        for (k, m) in self.marks.iter().enumerate().skip(i) {
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
    /// it, wrapping around; `None` without changes.
    pub fn step(&self, rope: &Rope, line: usize, forward: bool) -> Option<usize> {
        let firsts: Vec<usize> = self
            .marks
            .iter()
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
    Ok(Found::Marks {
        index,
        marks: marks(&text, &ours, if job.bom { 3 } else { 0 })?,
    })
}

/// The marks from the index text `old` to the buffer's bytes `new` (`skip`: the BOM's length at
/// its start, which the buffer's positions don't count).
fn marks(old: &[u8], new: &[u8], skip: usize) -> Result<Vec<Mark>, stage::Error> {
    let d = diff::diff(old, new, 0)?;
    if d.binary {
        return Ok(Vec::new());
    }
    let old_lines = diff::lines(old);
    // Each line's start in `new`, and its end.
    let mut starts = vec![0];
    starts.extend(new.iter().enumerate().filter(|(_, b)| **b == b'\n').map(|(i, _)| i + 1));
    if starts.last() == Some(&new.len()) && !new.is_empty() {
        starts.pop();
    }
    starts.push(new.len());
    let at = |line: usize| ByteIdx(starts[line.min(starts.len() - 1)].saturating_sub(skip));
    Ok(d.hunks
        .iter()
        .map(|h| {
            let removed: Vec<String> = old_lines[h.old.start.min(old_lines.len())..h.old.end.min(old_lines.len())]
                .iter()
                .take(MAX_REMOVED)
                .map(|l| {
                    let l = l.strip_suffix(b"\n").unwrap_or(l);
                    String::from_utf8_lossy(l.strip_suffix(b"\r").unwrap_or(l)).into_owned()
                })
                .collect();
            Mark {
                kind: h.kind(),
                start: at(h.new.start),
                end: at(h.new.end),
                removed: removed.into(),
                removed_count: h.old.len(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::change::Change;

    fn marks_of(old: &str, new: &str) -> Vec<Mark> {
        marks(old.as_bytes(), new.as_bytes(), 0).unwrap()
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
        let m = marks("\u{feff}a\nb\n".as_bytes(), "\u{feff}a\nB\n".as_bytes(), 3).unwrap();
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
        let (generation, _) = q.begin(1);
        assert_eq!(q.want(1), Want::Nothing);
        let snapshot = rope.clone();
        let set = Arc::new(ChangeSet::new(vec![Change {
            start: ByteIdx(0),
            end: ByteIdx(0),
            text: "z\n".into(),
        }]));
        set.apply(&mut rope);
        q.map(&set);
        let marks = marks(b"a\nb\nc\n", snapshot.to_string().as_bytes(), 0).unwrap();
        q.finish(Computed {
            revision: 1,
            generation,
            result: Ok(Found::Marks {
                index: Oid::ZERO_SHA1,
                marks,
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
        let (old, _) = q.begin(0);
        q.reset();
        assert!(q.marks().is_empty() && !q.running());
        assert_eq!(q.want(0), Want::Now, "computed afresh for the new path");
        let (new, known) = q.begin(0);
        assert!(new != old && known.is_none());
        let found = |text: &[u8]| Found::Marks {
            index: Oid::ZERO_SHA1,
            marks: marks(text, b"a\nB\n", 0).unwrap(),
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
}
