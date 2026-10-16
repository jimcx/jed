//! The Problems panel's model (no drawing; `ui::panel` draws it): every file with diagnostics,
//! sorted by path, each followed by its problems in position order, as a `grouped` list.
//!
//! - The files are the store's (`lsp::diagnostics::Store`: closed files keep theirs). A file
//!   open in a buffer lists the buffer's marks (byte ranges in its current text, so the
//!   positions follow edits); any other its published LSP positions (several servers' sets
//!   merged by position).
//! - The list is rebuilt from the store when asked (`files`: one entry per file, no per-problem
//!   work); a problem's row is only looked at when it's on screen (`FileProblems::problems`).
//! - Scroll and selection are kept by `RowKey` (path, row in file), so a publish that adds or
//!   drops a file above doesn't move them.
//! - Next / previous problem (F8 / Shift+F8, VS Code's "in files"): `step` finds the problem
//!   after / before a position in a file, else the first / last of the next / previous file with
//!   problems, wrapping.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ropey::Rope;

use crate::buffer::pos::{self, ByteIdx, LspPos, PosEncoding};
use crate::buffer::{Buffer, Buffers};
use crate::diagnostics::Diagnostic;
use crate::grouped::{self, Group, ListState, RowKey};
use crate::lsp::diagnostics::{FileSet, Store};

/// Where a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// In an open buffer's current text.
    Text { start: ByteIdx, end: ByteIdx },
    /// As published, for a file not open: read in its text once loaded.
    Lsp {
        start: LspPos,
        end: LspPos,
        encoding: PosEncoding,
    },
}

impl Place {
    /// The 1-based line and column shown as `[Ln x, Col y]`: an open buffer's display column (as
    /// the status bar's `Col`), else the published character + 1.
    pub fn line_col(&self, rope: Option<&Rope>, tab_width: usize) -> (usize, usize) {
        match (self, rope) {
            (Place::Text { start, .. }, Some(rope)) => {
                let at = ByteIdx(start.0.min(rope.len()));
                (pos::line_of(rope, at) + 1, pos::display_col(rope, at, tab_width).0 + 1)
            }
            (Place::Lsp { start, .. }, _) => (start.line as usize + 1, start.character as usize + 1),
            (Place::Text { .. }, None) => (1, 1),
        }
    }
}

/// One problem: what it says and where.
#[derive(Debug, Clone)]
pub struct Problem<'a> {
    pub diag: &'a Arc<Diagnostic>,
    pub place: Place,
}

/// A problem to open: its file and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub place: Place,
}

enum Source<'a> {
    Buffer(&'a Buffer),
    Store(&'a [FileSet]),
}

/// One file of the list.
pub struct FileProblems<'a> {
    pub path: &'a Path,
    source: Source<'a>,
    count: usize,
}

impl Group for FileProblems<'_> {
    fn path(&self) -> &Path {
        self.path
    }
    fn items(&self) -> usize {
        self.count
    }
}

impl<'a> FileProblems<'a> {
    pub fn count(&self) -> usize {
        self.count
    }

    /// The open buffer's text, if the file is open.
    pub fn rope(&self) -> Option<&'a Rope> {
        match self.source {
            Source::Buffer(b) => Some(&b.rope),
            Source::Store(_) => None,
        }
    }

    /// Problems `from..from + n` (clamped) in position order.
    pub fn problems(&self, from: usize, n: usize) -> Vec<Problem<'a>> {
        let to = from.saturating_add(n).min(self.count);
        if from >= to {
            return Vec::new();
        }
        match self.source {
            Source::Buffer(b) => b.diagnostics.all()[from..to]
                .iter()
                .map(|m| Problem {
                    diag: &m.diag,
                    place: Place::Text {
                        start: m.start,
                        end: m.end,
                    },
                })
                .collect(),
            Source::Store(sets) => {
                let lsp = |set: &'a FileSet, i: &'a crate::lsp::diagnostics::Item| Problem {
                    diag: &i.diag,
                    place: Place::Lsp {
                        start: i.start,
                        end: i.end,
                        encoding: set.encoding,
                    },
                };
                if let [set] = sets {
                    return set.items[from..to].iter().map(|i| lsp(set, i)).collect();
                }
                // Several servers: merged by position (stable, so each server's order stays).
                let mut all: Vec<Problem<'a>> = sets
                    .iter()
                    .flat_map(|s| s.items.iter().map(move |i| lsp(s, i)))
                    .collect();
                all.sort_by_key(|p| match p.place {
                    Place::Lsp { start, end, .. } => (start, end),
                    Place::Text { .. } => unreachable!("store items are LSP positions"),
                });
                all.drain(from..to).collect()
            }
        }
    }

    /// The `k`-th problem.
    pub fn problem(&self, k: usize) -> Option<Problem<'a>> {
        self.problems(k, 1).pop()
    }
}

/// Every file with problems, sorted by path. A file open in a buffer lists the buffer's marks.
pub fn files<'a>(store: &'a Store, buffers: &'a Buffers) -> Vec<FileProblems<'a>> {
    store
        .files()
        .filter_map(|(path, sets)| {
            let (source, count) = match buffers.find_path(path).and_then(|id| buffers.get(id)) {
                Some(b) => (Source::Buffer(b), b.diagnostics.all().len()),
                None => (Source::Store(sets), sets.iter().map(|s| s.items.len()).sum()),
            };
            (count > 0).then_some(FileProblems { path, source, count })
        })
        .collect()
}

/// Who reports a problem, VS Code style: `source(code)`, the source or the code alone, or nothing.
pub fn source_label(d: &Diagnostic) -> Option<String> {
    match (&d.source, &d.code) {
        (Some(s), Some(c)) => Some(format!("{s}({c})")),
        (Some(s), None) => Some(s.clone()),
        (None, Some(c)) => Some(format!("({c})")),
        (None, None) => None,
    }
}

/// Total problems in the list.
pub fn total(files: &[FileProblems]) -> usize {
    files.iter().map(|f| f.count).sum()
}

/// The Problems list's state: scroll and selection (by key), and the height drawn.
#[derive(Debug, Default)]
pub struct ProblemsView {
    pub list: ListState,
}

impl ProblemsView {
    /// The problem Enter opens: the selected one, or a file row's first (which becomes the
    /// selection, as the Search view does).
    pub fn target(&mut self, files: &[FileProblems]) -> Option<Target> {
        let (path, offset) = self.list.selected()?.clone();
        let (_, f) = grouped::group(files, &path)?;
        let k = offset.max(1).min(f.count) - 1;
        self.list.select_key(files, &(path, k + 1));
        target(f, k)
    }

    /// Selects the `k`-th problem of `path`'s file (scrolled into view).
    pub fn select_problem(&mut self, files: &[FileProblems], path: &Path, k: usize) {
        let key: RowKey = (path.to_path_buf(), k + 1);
        self.list.select_key(files, &key);
    }
}

/// The `k`-th problem of `file`, to open.
pub fn target(file: &FileProblems, k: usize) -> Option<Target> {
    Some(Target {
        path: file.path.to_path_buf(),
        place: file.problem(k)?.place,
    })
}

/// Where the cursor is, for `step`: a file and a byte position in its open buffer, and the
/// problem the last step went to if the cursor is still on it (several can start at one place).
#[derive(Debug, Clone)]
pub struct From<'p> {
    pub path: &'p Path,
    pub at: ByteIdx,
    pub last: Option<usize>,
}

/// F8 (`forward`) / Shift+F8: the problem after / before `from` in its file (by start; the one
/// after / before `last` when given), else the first / last problem of the next / previous file
/// with problems, wrapping around the list (back to the same file when it's the only one). With
/// no position (no file open), the first / last problem. Returns the file's index and the
/// problem's.
pub fn step(files: &[FileProblems], from: Option<From>, forward: bool) -> Option<(usize, usize)> {
    let n = files.len();
    if n == 0 {
        return None;
    }
    let edge = |i: usize| {
        let i = i % n;
        (i, if forward { 0 } else { files[i].count - 1 })
    };
    let Some(from) = from else {
        return Some(edge(if forward { 0 } else { n - 1 }));
    };
    let i = files.partition_point(|f| f.path < from.path);
    let Some(file) = files.get(i).filter(|f| f.path == from.path) else {
        // Not a file with problems: the next / previous one by path.
        return Some(edge(if forward { i } else { i + n - 1 }));
    };
    let within = match from.last.filter(|&k| k < file.count) {
        Some(k) if forward => (k + 1 < file.count).then_some(k + 1),
        Some(k) => k.checked_sub(1),
        None => {
            // Starts are sorted: the first after / last before `at` by binary search on a
            // buffer's marks; a closed file has no cursor in it, so this is the edge anyway.
            match file.source {
                Source::Buffer(b) => {
                    let marks = b.diagnostics.all();
                    if forward {
                        let k = marks.partition_point(|m| m.start <= from.at);
                        (k < marks.len()).then_some(k)
                    } else {
                        marks.partition_point(|m| m.start < from.at).checked_sub(1)
                    }
                }
                Source::Store(_) => None,
            }
        }
    };
    Some(match within {
        Some(k) => (i, k),
        None => edge(if forward { i + 1 } else { i + n - 1 }),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::lsp::ServerId;
    use crate::lsp::diagnostics::parse;

    fn diag(line: u32, from: u32, to: u32, message: &str) -> serde_json::Value {
        json!({
            "range": { "start": { "line": line, "character": from }, "end": { "line": line, "character": to } },
            "severity": 1,
            "message": message,
        })
    }

    fn store(files: &[(&str, ServerId, serde_json::Value)]) -> Store {
        let mut store = Store::default();
        for (path, server, diags) in files {
            let p = parse(json!({ "uri": format!("file://{path}"), "diagnostics": diags })).unwrap();
            store.publish(*server, PosEncoding::Utf8, p);
        }
        store
    }

    fn messages(f: &FileProblems) -> Vec<String> {
        f.problems(0, usize::MAX)
            .iter()
            .map(|p| p.diag.message.clone())
            .collect()
    }

    /// Files sorted by path; a closed file's problems by position, several servers merged; an
    /// open one lists its buffer's marks (current byte ranges).
    #[test]
    fn files_and_their_problems() {
        let (s1, s2) = (ServerId::test(1), ServerId::test(2));
        let store = store(&[
            ("/w/b.cpp", s1, json!([diag(3, 0, 1, "b3"), diag(0, 4, 5, "b0")])),
            ("/w/b.cpp", s2, json!([diag(1, 0, 1, "b1"), diag(0, 0, 1, "b0'")])),
            ("/w/a.cpp", s1, json!([diag(0, 2, 3, "a")])),
            ("/w/c.cpp", s1, json!([diag(1, 0, 3, "c1"), diag(0, 0, 3, "c0")])),
        ]);
        let mut buffers = Buffers::default();
        let mut c = Buffer::from_str("abc\ndef\n");
        c.path = PathBuf::from("/w/c.cpp");
        for set in store.get(&c.path) {
            let marks = crate::lsp::diagnostics::resolve(set.server, &set.items, &c.rope, set.encoding, &[]);
            c.diagnostics.replace(set.server, marks);
        }
        buffers.insert(c);
        let files = files(&store, &buffers);
        let paths: Vec<&Path> = files.iter().map(|f| f.path).collect();
        assert_eq!(
            paths,
            [Path::new("/w/a.cpp"), Path::new("/w/b.cpp"), Path::new("/w/c.cpp")]
        );
        assert_eq!(total(&files), 7);
        assert_eq!(messages(&files[1]), ["b0'", "b0", "b1", "b3"]);
        assert_eq!(
            files[1]
                .problems(1, 2)
                .iter()
                .map(|p| p.diag.message.as_str())
                .collect::<Vec<_>>(),
            ["b0", "b1"]
        );
        assert_eq!(files[1].problems(9, 2).len(), 0);
        assert!(matches!(files[1].problem(0).unwrap().place, Place::Lsp { .. }));
        assert_eq!(files[1].problem(3).unwrap().place.line_col(None, 4), (4, 1));
        // Open: the buffer's marks.
        assert_eq!(messages(&files[2]), ["c0", "c1"]);
        let p = files[2].problem(1).unwrap();
        assert_eq!(
            p.place,
            Place::Text {
                start: ByteIdx(4),
                end: ByteIdx(7)
            }
        );
        assert_eq!(p.place.line_col(files[2].rope(), 4), (2, 1));

        // An open buffer without marks (none placed in it) lists nothing, not an empty file
        // (which F8 couldn't step into).
        let mut a = Buffer::from_str("x\n");
        a.path = PathBuf::from("/w/a.cpp");
        buffers.insert(a);
        let files = super::files(&store, &buffers);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, Path::new("/w/b.cpp"));
        let from = From {
            path: Path::new("/w/a.cpp"),
            at: ByteIdx(0),
            last: None,
        };
        assert_eq!(step(&files, Some(from.clone()), true), Some((0, 0)));
        assert_eq!(step(&files, Some(from), false), Some((1, 1)));
    }

    /// F8 / Shift+F8: after / before the cursor in its file, then on to the next / previous
    /// file, wrapping; problems starting at one place are visited one by one through `last`.
    #[test]
    fn step_goes_through_files_and_wraps() {
        let s = ServerId::test(1);
        let store = store(&[
            (
                "/w/a.cpp",
                s,
                json!([diag(0, 0, 1, "a0"), diag(0, 2, 3, "a1"), diag(0, 2, 4, "a2")]),
            ),
            ("/w/c.cpp", s, json!([diag(0, 0, 1, "c0")])),
        ]);
        let mut buffers = Buffers::default();
        let mut a = Buffer::from_str("abcdef\n");
        a.path = PathBuf::from("/w/a.cpp");
        for set in store.get(&a.path) {
            let marks = crate::lsp::diagnostics::resolve(set.server, &set.items, &a.rope, set.encoding, &[]);
            a.diagnostics.replace(set.server, marks);
        }
        buffers.insert(a);
        let files = files(&store, &buffers);
        let a = Path::new("/w/a.cpp");
        let at = |at: usize, last: Option<usize>| {
            Some(From {
                path: a,
                at: ByteIdx(at),
                last,
            })
        };
        assert_eq!(step(&files, at(0, None), true), Some((0, 1)), "after the cursor");
        assert_eq!(step(&files, at(1, None), true), Some((0, 1)));
        assert_eq!(step(&files, at(2, Some(1)), true), Some((0, 2)), "same start: the next");
        assert_eq!(step(&files, at(2, Some(2)), true), Some((1, 0)), "next file");
        assert_eq!(step(&files, at(5, None), true), Some((1, 0)));
        assert_eq!(step(&files, at(2, None), false), Some((0, 0)));
        assert_eq!(step(&files, at(2, Some(2)), false), Some((0, 1)));
        assert_eq!(step(&files, at(0, None), false), Some((1, 0)), "wraps to the last file");
        // From the closed file c (no cursor there): on to the next / previous file.
        let c = Some(From {
            path: Path::new("/w/c.cpp"),
            at: ByteIdx(0),
            last: Some(0),
        });
        assert_eq!(step(&files, c.clone(), true), Some((0, 0)), "wraps to the first");
        assert_eq!(step(&files, c, false), Some((0, 2)));
        // A file without problems: the next / previous by path.
        let b = || {
            Some(From {
                path: Path::new("/w/b.cpp"),
                at: ByteIdx(0),
                last: None,
            })
        };
        assert_eq!(step(&files, b(), true), Some((1, 0)));
        assert_eq!(step(&files, b(), false), Some((0, 2)));
        assert_eq!(step(&files, None, true), Some((0, 0)));
        assert_eq!(step(&files, None, false), Some((1, 0)));
        assert_eq!(step(&[], None, true), None);
    }

    /// Enter on a file row opens its first problem and selects that row; selection by key.
    #[test]
    fn target_of_a_file_row_is_its_first_problem() {
        let s = ServerId::test(1);
        let store = store(&[("/w/a.cpp", s, json!([diag(2, 1, 2, "x"), diag(5, 0, 1, "y")]))]);
        let buffers = Buffers::default();
        let files = files(&store, &buffers);
        let mut v = ProblemsView::default();
        v.list.height = 10;
        assert!(v.target(&files).is_none());
        v.list.select(&files, 0);
        let t = v.target(&files).unwrap();
        assert_eq!(t.path, Path::new("/w/a.cpp"));
        assert_eq!(t.place, files[0].problem(0).unwrap().place);
        assert_eq!(v.list.selected_row(&files), Some(1));
        v.select_problem(&files, Path::new("/w/a.cpp"), 1);
        assert_eq!(v.list.selected_row(&files), Some(2));
        assert!(matches!(v.target(&files).unwrap().place, Place::Lsp { start, .. } if start.line == 5));
    }
}
