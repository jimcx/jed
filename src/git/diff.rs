//! Hunks between two texts: the one diff engine for the gutter, staging and the diff view
//! (`git2::Patch::from_buffers`, with git's defaults: Myers, the indent heuristic, no
//! inter-hunk merging beyond what the context does).

#![allow(dead_code)] // the gutter, staging and the diff view (10c–10e)

use std::ops::Range;

use git2::{DiffOptions, Patch};

/// The context `git add -p` and `git diff` use (`diff.context`'s default): staging one of these
/// hunks stages what `git add -p` stages for it. The gutter uses 0 (one hunk per change).
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

#[cfg(test)]
mod tests {
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
    fn binary_has_no_hunks() {
        let d = diff(b"a\0b", b"a\0c", 3).unwrap();
        assert!(d.binary);
        assert!(d.hunks.is_empty());
    }
}
