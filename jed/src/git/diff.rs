//! Hunks between two texts: the one diff engine for the gutter, staging and the diff view
//! (`git2::Patch::from_buffers`, with git's defaults: Myers, the indent heuristic, no
//! inter-hunk merging beyond what the context does).
//!
//! Besides whole hunks ([`apply`], [`revert`]) a [`Pick`] of lines takes only some of a
//! (context 0) hunk's changed lines ([`apply_lines`], [`revert_lines`]): what `git add -p`'s
//! `e`dit makes of the hunk.

use std::ops::Range;

use git2::{DiffOptions, Patch};

/// The context `git add -p` and `git diff` use (`diff.context`'s default): staging one of these
/// hunks stages what `git add -p` stages for it. The gutter and the diff view use 0 (one hunk
/// per change).
#[cfg_attr(not(test), allow(dead_code))]
pub const GIT_CONTEXT: u32 = 3;

/// One hunk: lines `old` of the old text became lines `new` of the new text. Both are 0-based,
/// half-open line ranges; a line includes its terminator (`\n`, so CRLF lines keep their `\r`),
/// and a last line without one is a line too. An empty range is a point: lines were inserted
/// before (`old`) or deleted before (`new`) that line. With context the ranges include the
/// unchanged context lines around the change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

/// What a (context 0) hunk does, for gutter markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Deleted,
    Modified,
}

impl Hunk {
    pub fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (_, true) => HunkKind::Deleted,
            _ => HunkKind::Modified,
        }
    }
}

/// The hunks from one text to another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub hunks: Vec<Hunk>,
    /// Either text is binary (has a NUL): no hunks, only whole-file staging.
    pub binary: bool,
}

/// Lines of `text`, each with its terminator.
pub fn lines(text: &[u8]) -> Vec<&[u8]> {
    text.split_inclusive(|&b| b == b'\n').collect()
}

/// The hunks from `old` to `new` with `context` lines of context around each change.
pub fn diff(old: &[u8], new: &[u8], context: u32) -> Result<Diff, git2::Error> {
    let mut opts = DiffOptions::new();
    opts.context_lines(context).interhunk_lines(0).indent_heuristic(true);
    let patch = Patch::from_buffers(old, None, new, None, Some(&mut opts))?;
    if patch.delta().flags().is_binary() {
        return Ok(Diff {
            hunks: Vec::new(),
            binary: true,
        });
    }
    let mut hunks = Vec::with_capacity(patch.num_hunks());
    for i in 0..patch.num_hunks() {
        let (h, _) = patch.hunk(i)?;
        hunks.push(Hunk {
            old: range(h.old_start(), h.old_lines()),
            new: range(h.new_start(), h.new_lines()),
        });
    }
    Ok(Diff { hunks, binary: false })
}

/// A hunk header's 1-based start and count as a 0-based range (a count of 0 names the line
/// *after* which, so it is already the 0-based insertion point).
fn range(start: u32, count: u32) -> Range<usize> {
    let start = start as usize;
    let begin = if count == 0 { start } else { start - 1 };
    begin..begin + count as usize
}

/// The (context 0) hunk shown on `line` of the new text, as the gutter draws them: one whose new
/// lines include it, else a deletion drawn there ([`deletion_line`]).
pub fn hunk_at_line(hunks: &[Hunk], line: usize) -> Option<usize> {
    hunks
        .iter()
        .position(|h| h.new.contains(&line))
        .or_else(|| hunks.iter().position(|h| h.new.is_empty() && deletion_line(h) == line))
}

/// The new text's line a deletion's marker is drawn on: below the line before it (VS Code), or
/// above line 0 when the deletion is at the top.
pub fn deletion_line(h: &Hunk) -> usize {
    h.new.start.saturating_sub(1)
}

/// Lines `line` of the new text stands for in the old one, through `hunks` (context 0): itself,
/// shifted by the changes above it, or a changed line's hunk's old lines (empty: an added line,
/// the point it was added at).
pub fn old_lines(hunks: &[Hunk], line: usize) -> Range<usize> {
    let mut delta = 0isize;
    for h in hunks {
        if line < h.new.start {
            break;
        }
        if h.new.contains(&line) {
            return h.old.clone();
        }
        delta = h.old.end as isize - h.new.end as isize;
    }
    let at = (line as isize + delta).max(0) as usize;
    at..at + 1
}

/// The reverse of [`old_lines`]: the new text's lines the old text's `line` stands for (itself,
/// shifted by the changes above it, or a changed line's hunk's new lines — empty: a deleted line,
/// the point it was deleted at).
pub fn new_lines(hunks: &[Hunk], line: usize) -> Range<usize> {
    let mut delta = 0isize;
    for h in hunks {
        if line < h.old.start {
            break;
        }
        if h.old.contains(&line) {
            return h.new.clone();
        }
        delta = h.new.end as isize - h.old.end as isize;
    }
    let at = (line as isize + delta).max(0) as usize;
    at..at + 1
}

/// Whether a hunk whose new lines are `ours` touches the lines `lines` (both may be points: an
/// empty range between two lines).
pub fn touches(ours: &Range<usize>, lines: &Range<usize>) -> bool {
    match (ours.is_empty(), lines.is_empty()) {
        (false, false) => ours.start < lines.end && lines.start < ours.end,
        (true, false) => lines.start <= ours.start && ours.start <= lines.end,
        (false, true) => ours.start <= lines.start && lines.start <= ours.end,
        (true, true) => ours.start == lines.start,
    }
}

/// `old` with one hunk of the `old` → `new` diff applied: its old lines replaced by its new
/// ones (staging a hunk: `old` = the index text, `new` = the file on disk).
pub fn apply(old: &[u8], new: &[u8], hunk: &Hunk) -> Vec<u8> {
    splice(old, &hunk.old, new, &hunk.new)
}

/// `new` with one hunk of the `old` → `new` diff reverted: its new lines replaced by its old
/// ones (unstaging a hunk: `old` = HEAD's text, `new` = the index text).
pub fn revert(old: &[u8], new: &[u8], hunk: &Hunk) -> Vec<u8> {
    splice(new, &hunk.new, old, &hunk.old)
}

/// `base` with its lines `at` replaced by lines `from` of `other`.
fn splice(base: &[u8], at: &Range<usize>, other: &[u8], from: &Range<usize>) -> Vec<u8> {
    let base = lines(base);
    let other = lines(other);
    let at = at.start.min(base.len())..at.end.min(base.len());
    let from = from.start.min(other.len())..from.end.min(other.len());
    let mut out = Vec::new();
    for l in base[..at.start].iter().chain(&other[from]).chain(&base[at.end..]) {
        out.extend_from_slice(l);
    }
    out
}

/// Lines picked from a (context 0) diff: `old` are lines of the old text (removed lines to
/// take), `new` lines of the new text (added lines to take). Sorted, merged, no empty ranges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pick {
    old: Vec<Range<usize>>,
    new: Vec<Range<usize>>,
}

impl Pick {
    /// A pick of old lines and new lines, in any order, overlapping or not.
    pub fn new(old: impl IntoIterator<Item = Range<usize>>, new: impl IntoIterator<Item = Range<usize>>) -> Pick {
        Pick {
            old: normalize(old),
            new: normalize(new),
        }
    }

    /// The pick of a text tab's selections: the new lines `ranges` (each a line range; need not be
    /// sorted), plus the removed lines of every hunk that touches one of them ([`touches`]: a
    /// pure deletion at a range's boundary counts).
    pub fn from_new_lines(hunks: &[Hunk], ranges: impl IntoIterator<Item = Range<usize>>) -> Pick {
        let new = normalize(ranges);
        let mut old = Vec::new();
        for h in hunks {
            // From the first range ending at or after the hunk's start, while ranges start in reach.
            let first = new.partition_point(|r| r.end < h.new.start);
            if new[first..]
                .iter()
                .take_while(|r| r.start <= h.new.end)
                .any(|r| touches(&h.new, r))
            {
                old.push(h.old.clone());
            }
        }
        Pick::new(old, new)
    }

    pub fn has_old(&self, line: usize) -> bool {
        contains(&self.old, line)
    }

    pub fn has_new(&self, line: usize) -> bool {
        contains(&self.new, line)
    }

    /// How many changed lines of `hunks` (removed and added) the pick takes.
    pub fn count(&self, hunks: &[Hunk]) -> usize {
        let n = |ranges: &[Range<usize>], of: &Range<usize>| -> usize {
            let first = ranges.partition_point(|r| r.end <= of.start);
            ranges[first..]
                .iter()
                .take_while(|r| r.start < of.end)
                .map(|r| r.end.min(of.end) - r.start.max(of.start))
                .sum()
        };
        hunks.iter().map(|h| n(&self.old, &h.old) + n(&self.new, &h.new)).sum()
    }

    /// The changed lines of `hunks` this pick leaves out.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn complement(&self, hunks: &[Hunk]) -> Pick {
        Pick::new(
            hunks
                .iter()
                .flat_map(|h| h.old.clone())
                .filter(|&l| !self.has_old(l))
                .map(|l| l..l + 1),
            hunks
                .iter()
                .flat_map(|h| h.new.clone())
                .filter(|&l| !self.has_new(l))
                .map(|l| l..l + 1),
        )
    }
}

fn normalize(ranges: impl IntoIterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let mut v: Vec<Range<usize>> = ranges.into_iter().filter(|r| !r.is_empty()).collect();
    v.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::with_capacity(v.len());
    for r in v {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

fn contains(ranges: &[Range<usize>], line: usize) -> bool {
    let i = ranges.partition_point(|r| r.end <= line);
    ranges.get(i).is_some_and(|r| r.start <= line)
}

/// `old` with the changes `pick` takes from the `old` → `new` diff (`hunks`, context 0) applied
/// and the rest of each hunk left as it was: per hunk its old lines that aren't picked, then its
/// new lines that are (staging lines: `old` = the index text, `new` = the file on disk). Bytes
/// are kept exactly; a line without a terminator that something follows gets a `\n`.
pub fn apply_lines(old: &[u8], new: &[u8], hunks: &[Hunk], pick: &Pick) -> Vec<u8> {
    splice_picked(old, new, hunks, pick, false)
}

/// `new` with the changes `pick` takes undone, the rest left: per hunk its old lines that are
/// picked, then its new lines that aren't (unstaging or reverting lines: `old` = HEAD's / the
/// index's text, `new` = the index's / the file's).
pub fn revert_lines(old: &[u8], new: &[u8], hunks: &[Hunk], pick: &Pick) -> Vec<u8> {
    splice_picked(old, new, hunks, pick, true)
}

/// [`revert_lines`] as edits of `new`: for each hunk the pick takes lines of, the byte range of
/// its lines in `new` and what replaces them (its picked old lines, then its unpicked new ones).
/// Applying them all to `new` gives `revert_lines`' result; untouched hunks and the text between
/// are not in the list, so positions there keep their place (an open buffer's edit).
pub fn revert_line_edits(old: &[u8], new: &[u8], hunks: &[Hunk], pick: &Pick) -> Vec<(Range<usize>, Vec<u8>)> {
    let (old_l, new_l) = (lines(old), lines(new));
    let mut edits = Vec::new();
    // Byte offset of line `line` of `new`, walking forward only.
    let (mut line, mut offset) = (0, 0);
    let mut seek = |to: usize| {
        let to = to.min(new_l.len());
        while line < to {
            offset += new_l[line].len();
            line += 1;
        }
        offset
    };
    for h in hunks {
        if pick.count(std::slice::from_ref(h)) == 0 {
            continue;
        }
        let (start, end) = (seek(h.new.start), seek(h.new.end));
        let mut text: Vec<u8> = Vec::new();
        let mut push = |l: &[u8]| {
            if text.last().is_some_and(|&b| b != b'\n') {
                text.push(b'\n');
            }
            text.extend_from_slice(l);
        };
        h.old
            .clone()
            .filter(|&i| i < old_l.len() && pick.has_old(i))
            .for_each(|i| push(old_l[i]));
        h.new
            .clone()
            .filter(|&i| i < new_l.len() && !pick.has_new(i))
            .for_each(|i| push(new_l[i]));
        // Text follows this hunk: its last line needs its terminator.
        if h.new.end < new_l.len() && text.last().is_some_and(|&b| b != b'\n') {
            text.push(b'\n');
        }
        edits.push((start..end, text));
    }
    edits
}

/// One pass over `hunks`, copying the base text (`old`; `new` when `reverse`) between them. In
/// each hunk, old lines then new lines: apply keeps the unpicked old lines and the picked new
/// ones, reverse the picked old lines and the unpicked new ones.
fn splice_picked(old: &[u8], new: &[u8], hunks: &[Hunk], pick: &Pick, reverse: bool) -> Vec<u8> {
    let (old_l, new_l) = (lines(old), lines(new));
    let base = if reverse { &new_l } else { &old_l };
    let mut out = Vec::with_capacity(old.len().max(new.len()));
    let mut push = |l: &[u8]| {
        if out.last().is_some_and(|&b| b != b'\n') {
            out.push(b'\n');
        }
        out.extend_from_slice(l);
    };
    let mut at = 0;
    for h in hunks {
        let end = if reverse { h.new.end } else { h.old.end };
        let start = if reverse { h.new.start } else { h.old.start };
        base[at.min(base.len())..start.min(base.len())]
            .iter()
            .for_each(|l| push(l));
        at = end;
        for i in h.old.clone().filter(|&i| i < old_l.len()) {
            if pick.has_old(i) == reverse {
                push(old_l[i]);
            }
        }
        for i in h.new.clone().filter(|&i| i < new_l.len()) {
            if pick.has_new(i) != reverse {
                push(new_l[i]);
            }
        }
    }
    base[at.min(base.len())..].iter().for_each(|l| push(l));
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::single_range_in_vec_init)]
    use super::*;

    fn h(old: Range<usize>, new: Range<usize>) -> Hunk {
        Hunk { old, new }
    }

    #[test]
    fn zero_context_hunks_are_the_changed_lines() {
        let old = b"a\nb\nc\nd\n";
        let new = b"a\nB\nc\nd\ne\n";
        let d = diff(old, new, 0).unwrap();
        assert_eq!(d.hunks, vec![h(1..2, 1..2), h(4..4, 4..5)]);
        assert_eq!(d.hunks[0].kind(), HunkKind::Modified);
        assert_eq!(d.hunks[1].kind(), HunkKind::Added);
        let d = diff(b"a\nb\nc\n", b"b\nc\n", 0).unwrap();
        assert_eq!(d.hunks, vec![h(0..1, 0..0)]);
        assert_eq!(d.hunks[0].kind(), HunkKind::Deleted);
    }

    #[test]
    fn context_hunks_include_context_and_merge() {
        let old: Vec<u8> = (0..20).flat_map(|i| format!("line {i}\n").into_bytes()).collect();
        let new = String::from_utf8(old.clone())
            .unwrap()
            .replace("line 5\n", "five\n")
            .replace("line 8\n", "eight\n");
        let d = diff(&old, new.as_bytes(), GIT_CONTEXT).unwrap();
        // Lines 5 and 8 changed: their contexts overlap, so one hunk 2..12.
        assert_eq!(d.hunks, vec![h(2..12, 2..12)]);
    }

    #[test]
    fn insertion_at_the_top_and_into_empty() {
        let d = diff(b"a\n", b"x\na\n", 0).unwrap();
        assert_eq!(d.hunks, vec![h(0..0, 0..1)]);
        let d = diff(b"", b"x\ny", 3).unwrap();
        assert_eq!(d.hunks, vec![h(0..0, 0..2)]);
        assert_eq!(apply(b"", b"x\ny", &d.hunks[0]), b"x\ny");
        let d = diff(b"x\ny\n", b"", 3).unwrap();
        assert_eq!(d.hunks, vec![h(0..2, 0..0)]);
        assert_eq!(apply(b"x\ny\n", b"", &d.hunks[0]), b"");
    }

    #[test]
    fn apply_and_revert_one_hunk() {
        let old = b"a\nb\nc\nd\n";
        let new = b"a\nB\nc\nd\ne\n";
        let d = diff(old, new, 0).unwrap();
        assert_eq!(apply(old, new, &d.hunks[1]), b"a\nb\nc\nd\ne\n");
        assert_eq!(apply(old, new, &d.hunks[0]), b"a\nB\nc\nd\n");
        assert_eq!(revert(old, new, &d.hunks[0]), b"a\nb\nc\nd\ne\n");
        assert_eq!(revert(old, new, &d.hunks[1]), b"a\nB\nc\nd\n");
    }

    #[test]
    fn missing_final_newline_and_crlf() {
        // The last line without a line break differs from the same line with one.
        let d = diff(b"a\nb", b"a\nb\nc\n", 0).unwrap();
        assert_eq!(d.hunks, vec![h(1..2, 1..3)]);
        assert_eq!(apply(b"a\nb", b"a\nb\nc\n", &d.hunks[0]), b"a\nb\nc\n");
        let d = diff(b"a\nb\n", b"a\nb", 0).unwrap();
        assert_eq!(apply(b"a\nb\n", b"a\nb", &d.hunks[0]), b"a\nb");
        let old = b"a\r\nb\r\nc\r\n";
        let new = b"a\r\nB\r\nc\r\n";
        let d = diff(old, new, 3).unwrap();
        assert_eq!(apply(old, new, &d.hunks[0]), new);
    }

    /// A hunk as a `git diff` header's `-a,b +c,d` (1-based start, or the line after which for
    /// an empty side; `,1` omitted as git does).
    fn header(h: &Hunk) -> String {
        let side = |r: &Range<usize>| match r.len() {
            0 => format!("{},0", r.start),
            1 => format!("{}", r.start + 1),
            n => format!("{},{n}", r.start + 1),
        };
        format!("-{} +{}", side(&h.old), side(&h.new))
    }

    /// Each file's hunk headers in `git diff` output.
    fn git_headers(out: &str) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut files = std::collections::BTreeMap::new();
        let mut current = String::new();
        for l in out.lines() {
            if let Some(name) = l.strip_prefix("+++ b/") {
                current = name.to_string();
                files.insert(current.clone(), Vec::new());
            } else if let Some(rest) = l.strip_prefix("@@ ") {
                let h = rest.split(" @@").next().unwrap().to_string();
                files.get_mut(&current).unwrap().push(h);
            }
        }
        files
    }

    /// Random edits to texts made of a few repeated lines (where the alignment is ambiguous),
    /// plus the indent heuristic's classic case: the hunks are the ones `git diff` prints,
    /// with and without context.
    #[test]
    fn hunks_agree_with_git_diff() {
        use crate::git::test_repo::{commit_all, git, init};
        const LINES: &[&str] = &["a\n", "b\n", "\n", "}\n", "    x\n", "fn f() {\n", "c"];
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut rand = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let text = |lines: &[&str]| -> Vec<u8> {
            // A line without a terminator only last.
            let mut out = Vec::new();
            for (i, l) in lines.iter().enumerate() {
                out.extend_from_slice(l.as_bytes());
                if !l.ends_with('\n') && i + 1 < lines.len() {
                    out.push(b'\n');
                }
            }
            out
        };
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = vec![
            (
                b"fn a() {\n    1\n}\n\nfn c() {\n    3\n}\n".to_vec(),
                b"fn a() {\n    1\n}\n\nfn b() {\n    2\n}\n\nfn c() {\n    3\n}\n".to_vec(),
            ),
            // Only the indent heuristic puts this insertion at line 4 (else 5).
            (
                b"    y\n    y\n\nfn f() {\n".to_vec(),
                b"    y\n    y\n\nfn f() {\n    y\nfn f() {\n".to_vec(),
            ),
        ];
        for _ in 0..40 {
            let old: Vec<&str> = (0..rand(25)).map(|_| LINES[rand(LINES.len())]).collect();
            let mut new = old.clone();
            for _ in 0..1 + rand(4) {
                let at = rand(new.len() + 1);
                match rand(3) {
                    0 => new.insert(at, LINES[rand(LINES.len())]),
                    1 if at < new.len() => {
                        new.remove(at);
                    }
                    _ if at < new.len() => new[at] = LINES[rand(LINES.len())],
                    _ => {}
                }
            }
            pairs.push((text(&old), text(&new)));
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init(root);
        for (i, (old, _)) in pairs.iter().enumerate() {
            std::fs::write(root.join(format!("{i:02}")), old).unwrap();
        }
        commit_all(root, "old");
        for (i, (_, new)) in pairs.iter().enumerate() {
            std::fs::write(root.join(format!("{i:02}")), new).unwrap();
        }
        for context in [0, GIT_CONTEXT] {
            let out = git(root, &["diff", "--no-color", &format!("-U{context}")]);
            let expected = git_headers(&out);
            for (i, (old, new)) in pairs.iter().enumerate() {
                let ours: Vec<String> = diff(old, new, context).unwrap().hunks.iter().map(header).collect();
                let name = format!("{i:02}");
                let theirs = expected.get(&name).cloned().unwrap_or_default();
                assert_eq!(ours, theirs, "case {i}, context {context}: {old:?} -> {new:?}");
            }
        }
    }

    #[test]
    fn hunk_at_line_and_old_lines() {
        // Line 1 modified, 2 lines added after line 4, line 7 deleted (the old text's line 7).
        let old = b"0\n1\n2\n3\n4\n5\n6\n7\n8\n";
        let new = b"0\nONE\n2\n3\n4\nx\ny\n5\n6\n8\n";
        let d = diff(old, new, 0).unwrap();
        assert_eq!(d.hunks, vec![h(1..2, 1..2), h(5..5, 5..7), h(7..8, 9..9)]);
        assert_eq!(hunk_at_line(&d.hunks, 1), Some(0));
        assert_eq!(hunk_at_line(&d.hunks, 0), None);
        assert_eq!(hunk_at_line(&d.hunks, 6), Some(1));
        // The deletion before new line 9 is drawn below line 8.
        assert_eq!(hunk_at_line(&d.hunks, 8), Some(2));
        assert_eq!(hunk_at_line(&d.hunks, 9), None);
        // A deletion at the top is drawn on line 0, under a change there if any.
        let top = diff(b"a\nb\n", b"b\n", 0).unwrap();
        assert_eq!(hunk_at_line(&top.hunks, 0), Some(0));
        assert_eq!(old_lines(&d.hunks, 0), 0..1);
        assert_eq!(old_lines(&d.hunks, 1), 1..2);
        assert_eq!(old_lines(&d.hunks, 4), 4..5);
        assert_eq!(old_lines(&d.hunks, 5), 5..5, "an added line: the point");
        assert_eq!(old_lines(&d.hunks, 7), 5..6);
        assert_eq!(old_lines(&d.hunks, 9), 8..9, "after the deletion");
        assert_eq!(new_lines(&d.hunks, 0), 0..1);
        assert_eq!(new_lines(&d.hunks, 1), 1..2);
        assert_eq!(new_lines(&d.hunks, 4), 4..5);
        assert_eq!(
            new_lines(&d.hunks, 5),
            7..8,
            "an inserted line's point shifts to the line after"
        );
        assert_eq!(new_lines(&d.hunks, 6), 8..9);
        assert_eq!(new_lines(&d.hunks, 7), 9..9, "a deleted line: the point");
        assert_eq!(new_lines(&d.hunks, 8), 9..10, "after the deletion");
        assert!(touches(&(2..4), &(3..4)) && !touches(&(2..4), &(4..5)));
        assert!(touches(&(4..4), &(3..4)) && touches(&(4..4), &(4..5)) && !touches(&(6..6), &(4..5)));
        assert!(touches(&(2..4), &(4..4)) && !touches(&(2..4), &(5..5)));
    }

    #[test]
    fn binary_has_no_hunks() {
        let d = diff(b"a\0b", b"a\0c", 3).unwrap();
        assert!(d.binary);
        assert!(d.hunks.is_empty());
    }

    fn hunks_of(old: &[u8], new: &[u8]) -> Vec<Hunk> {
        diff(old, new, 0).unwrap().hunks
    }

    fn pick(old: &[Range<usize>], new: &[Range<usize>]) -> Pick {
        Pick::new(old.to_vec(), new.to_vec())
    }

    fn both(old: &[u8], new: &[u8], p: &Pick) -> (Vec<u8>, Vec<u8>) {
        let hunks = hunks_of(old, new);
        (apply_lines(old, new, &hunks, p), revert_lines(old, new, &hunks, p))
    }

    #[test]
    fn pick_normalises_and_looks_up() {
        let p = Pick::new([5..7, 1..3, 3..4, 2..2, 9..9], [0..1]);
        assert_eq!(p.old, vec![1..4, 5..7]);
        assert!(p.has_old(1) && p.has_old(3) && !p.has_old(4) && p.has_old(6) && !p.has_old(7) && !p.has_old(0));
        assert!(p.has_new(0) && !p.has_new(1));
        let hunks = [h(1..3, 1..2), h(5..5, 4..6)];
        assert_eq!(p.count(&hunks), 2, "old 1..3 in the first hunk; new 0..1 misses both");
        let q = Pick::new([1..2], [5..6]);
        assert_eq!(q.count(&hunks), 2);
        assert_eq!(q.complement(&hunks), Pick::new([2..3], [1..2, 4..5]));
    }

    #[test]
    fn pick_from_new_lines_takes_touched_hunks_old_lines() {
        // Modified at 1, a pure deletion before new line 3, an addition at 6..8.
        let hunks = [h(1..2, 1..2), h(3..5, 3..3), h(8..8, 6..8)];
        let p = Pick::from_new_lines(&hunks, [6..7, 1..2]);
        assert_eq!(p, Pick::new([1..2, 8..8], [1..2, 6..7]));
        // The deletion is touched from either side, not from further away.
        assert_eq!(Pick::from_new_lines(&hunks, [2..3]).old, vec![3..5]);
        assert_eq!(Pick::from_new_lines(&hunks, [3..4]).old, vec![3..5]);
        assert!(Pick::from_new_lines(&hunks, [4..5]).old.is_empty());
        assert!(Pick::from_new_lines(&hunks, [0..1]).old.is_empty());
        assert!(Pick::from_new_lines(&hunks, []).old.is_empty());
    }

    #[test]
    fn lines_of_a_modified_hunk() {
        let (old, new) = (b"x\na\nb\ny\n", b"x\nA\nB\ny\n");
        // `-a -b +A +B` picking old b and new A: `a A` (b gone, a kept, A added, B left out).
        let p = pick(&[2..3], &[1..2]);
        let (staged, reverted) = both(old, new, &p);
        assert_eq!(staged, b"x\na\nA\ny\n");
        // Reverting: b back, A away, B stays.
        assert_eq!(reverted, b"x\nb\nB\ny\n");
    }

    #[test]
    fn lines_of_pure_additions_and_deletions() {
        let (old, new) = (b"a\nd\n", b"a\nb\nc\nd\n");
        let p = pick(&[], &[2..3]);
        assert_eq!(both(old, new, &p), (b"a\nc\nd\n".to_vec(), b"a\nb\nd\n".to_vec()));
        let (old, new) = (b"a\nb\nc\nd\n", b"a\nd\n");
        let p = pick(&[1..2], &[]);
        assert_eq!(both(old, new, &p), (b"a\nc\nd\n".to_vec(), b"a\nb\nd\n".to_vec()));
    }

    #[test]
    fn lines_across_hunks_nothing_and_everything() {
        let old = b"1\n2\n3\n4\n5\n6\n";
        let new = b"1\nTWO\n3\n4\n6\nsix\n";
        let hunks = hunks_of(old, new);
        assert_eq!(hunks.len(), 3, "{hunks:?}");
        assert_eq!(both(old, new, &Pick::default()), (old.to_vec(), new.to_vec()));
        let all = Pick::new([0..6], [0..6]);
        assert_eq!(both(old, new, &all), (new.to_vec(), old.to_vec()));
        assert_eq!(all.count(&hunks), 4);
        // Only the second hunk's removal of 5 (not the added `six`).
        let p = pick(&[4..5], &[]);
        assert_eq!(both(old, new, &p).0, b"1\n2\n3\n4\n6\n".to_vec());
        // The first hunk whole, the second untouched.
        let p = pick(&[1..2], &[1..2]);
        assert_eq!(both(old, new, &p).0, b"1\nTWO\n3\n4\n5\n6\n".to_vec());
    }

    #[test]
    fn lines_keep_crlf_and_a_missing_final_newline() {
        let (old, new) = (b"a\r\nb\r\nc\r\n", b"a\r\nB\r\nC\r\n");
        let p = pick(&[2..3], &[1..2]);
        assert_eq!(both(old, new, &p).0, b"a\r\nb\r\nB\r\n".to_vec());
        // `c` without a newline becomes `c\n` plus a line: an unpicked old `c` gets its newline
        // when something follows it.
        let (old, new) = (b"a\nc", b"a\nc\nd");
        let hunks = hunks_of(old, new);
        assert_eq!(hunks, vec![h(1..2, 1..3)]);
        assert_eq!(apply_lines(old, new, &hunks, &pick(&[], &[2..3])), b"a\nc\nd");
        assert_eq!(apply_lines(old, new, &hunks, &pick(&[], &[1..2])), b"a\nc\nc\n");
        assert_eq!(apply_lines(old, new, &hunks, &pick(&[1..2], &[1..2])), b"a\nc\n");
        assert_eq!(apply_lines(old, new, &hunks, &pick(&[1..2], &[2..3])), b"a\nd");
        assert_eq!(revert_lines(old, new, &hunks, &pick(&[1..2], &[2..3])), b"a\nc\nc\n");
        // Dropping the final newline: the picked new last line has none.
        let (old, new) = (b"a\nb\n", b"a\nb");
        let hunks = hunks_of(old, new);
        assert_eq!(apply_lines(old, new, &hunks, &pick(&[1..2], &[1..2])), b"a\nb");
        assert_eq!(apply_lines(old, new, &hunks, &pick(&[], &[1..2])), b"a\nb\nb");
    }

    /// Random pairs of texts (a few repeated lines, last line maybe unterminated, CRLF maybe) and
    /// random picks: the result matches a slower reference (hunks applied last to first on a
    /// list of lines), and a pick's revert is its complement's apply.
    #[test]
    fn lines_agree_with_a_slow_reference() {
        const LINES: &[&str] = &["a\n", "b\n", "\n", "x\r\n", "c"];
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut rand = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let text = |rand: &mut dyn FnMut(usize) -> usize| -> Vec<u8> {
            let n = rand(9);
            let mut out = Vec::new();
            for i in 0..n {
                let l = LINES[rand(LINES.len())];
                out.extend_from_slice(l.as_bytes());
                if !l.ends_with('\n') && i + 1 < n {
                    out.push(b'\n');
                }
            }
            out
        };
        let join = |parts: Vec<&[u8]>| -> Vec<u8> {
            let mut out: Vec<u8> = Vec::new();
            for l in parts {
                if out.last().is_some_and(|&b| b != b'\n') {
                    out.push(b'\n');
                }
                out.extend_from_slice(l);
            }
            out
        };
        for _ in 0..200 {
            let (old, new) = (text(&mut rand), text(&mut rand));
            let hunks = hunks_of(&old, &new);
            let (ol, nl) = (lines(&old), lines(&new));
            let p = Pick::new(
                (0..ol.len())
                    .filter(|_| rand(2) == 0)
                    .map(|i| i..i + 1)
                    .collect::<Vec<_>>(),
                (0..nl.len())
                    .filter(|_| rand(2) == 0)
                    .map(|i| i..i + 1)
                    .collect::<Vec<_>>(),
            );
            let mut staged = ol.clone();
            let mut reverted = nl.clone();
            for hk in hunks.iter().rev() {
                let mut keep: Vec<&[u8]> = hk.old.clone().filter(|&i| !p.has_old(i)).map(|i| ol[i]).collect();
                keep.extend(hk.new.clone().filter(|&i| p.has_new(i)).map(|i| nl[i]));
                staged.splice(hk.old.clone(), keep);
                let mut back: Vec<&[u8]> = hk.old.clone().filter(|&i| p.has_old(i)).map(|i| ol[i]).collect();
                back.extend(hk.new.clone().filter(|&i| !p.has_new(i)).map(|i| nl[i]));
                reverted.splice(hk.new.clone(), back);
            }
            let ctx = format!(
                "{:?} -> {:?}, {p:?}",
                String::from_utf8_lossy(&old),
                String::from_utf8_lossy(&new)
            );
            let (got, back) = both(&old, &new, &p);
            assert_eq!(got, join(staged), "{ctx}");
            assert_eq!(back, join(reverted), "{ctx}");
            let rest = p.complement(&hunks);
            assert_eq!(back, apply_lines(&old, &new, &hunks, &rest), "{ctx}");
            assert_eq!(got, revert_lines(&old, &new, &hunks, &rest), "{ctx}");
        }
    }

    #[test]
    fn revert_line_edits_applied_equal_revert_lines() {
        let cases: [(&[u8], &[u8]); 4] = [
            (b"x\na\nb\ny\nz\nq\n", b"x\nA\nB\ny\nz\nQ\nr\n"),
            (b"a\nb\nc", b"a\nB\nc\nd"),
            (b"a\nb\nc\n", b"a\nc"),
            (b"a\nb", b"a\nB"),
        ];
        let mut seed = 7u32;
        let mut rand = |n: u32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 16) % n
        };
        for (old, new) in cases {
            let hunks = hunks_of(old, new);
            let (ol, nl) = (lines(old).len(), lines(new).len());
            for _ in 0..40 {
                let p = Pick::new(
                    (0..ol).filter(|_| rand(2) == 0).map(|i| i..i + 1).collect::<Vec<_>>(),
                    (0..nl).filter(|_| rand(2) == 0).map(|i| i..i + 1).collect::<Vec<_>>(),
                );
                let mut out = new.to_vec();
                let edits = revert_line_edits(old, new, &hunks, &p);
                for (range, text) in edits.iter().rev() {
                    out.splice(range.clone(), text.iter().copied());
                }
                assert_eq!(out, revert_lines(old, new, &hunks, &p), "{old:?} -> {new:?}, {p:?}");
                assert!(edits.len() <= hunks.len());
            }
        }
    }
}
