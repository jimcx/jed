//! Where a review thread sits now: the text it was written against (the blob of its anchor commit)
//! diffed against the text it is shown on, with its lines mapped through the hunks.
//!
//! A thread on the right side is compared with the work tree's file, one on the left (a deleted
//! line) with the merge base's, the left side of the review diff. When every anchored line is
//! unchanged the thread is [`Placement::Placed`] at the mapped lines; a line inside a changed
//! hunk (or lines inserted between the anchored ones, or a vanished file) makes it
//! [`Placement::Outdated`]; a commit that isn't in the clone makes it [`Placement::Missing`].
//!
//! [`place_all`] runs at lookup time (off-thread) and so judges the file on disk, not unsaved
//! edits; the next refresh sees an edit once it is saved.

use std::collections::HashMap;
use std::ops::Range;

use git2::{Oid, Repository};

use crate::forge::{Anchor, Side, Thread};
use crate::git::Repo;
use crate::git::diff::{self, new_lines, touches};
use crate::git::range::FileChange;

/// Where a thread is on the review diff now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Unchanged: these 0-based half-open lines of `side` (the left text is the merge base's,
    /// the right one the work tree's).
    Placed { side: Side, lines: Range<usize> },
    /// The anchored code changed or went: listed under CONVERSATIONS, not in the diff.
    Outdated,
    /// The anchor commit isn't in the local clone: listed, read-only.
    Missing,
}

/// An anchored text diffed against the text it is shown on: what every thread on the same
/// commit, file and side shares.
struct Compared {
    /// Lines of the anchored text.
    lines: usize,
    diff: diff::Diff,
}

impl Compared {
    /// `None` when the texts can't be compared (binary, a diff error): the threads there are outdated.
    fn new(anchored: &[u8], target: &[u8]) -> Option<Compared> {
        let diff = diff::diff(anchored, target, 0).ok().filter(|d| !d.binary)?;
        Some(Compared {
            lines: diff::lines(anchored).len(),
            diff,
        })
    }

    /// Where lines `start..=end` (1-based) of the anchored text are in the target.
    fn place(&self, side: Side, start: u32, end: u32) -> Placement {
        let first = (start.max(1) - 1) as usize;
        let last = (end.max(start).max(1)) as usize;
        let range = first..last;
        if range.end > self.lines {
            return Placement::Outdated;
        }
        for h in &self.diff.hunks {
            // Changed lines inside the range, or lines inserted between its lines.
            if touches(&h.old, &range)
                && !(h.old.is_empty() && (h.old.start == range.start || h.old.start == range.end))
            {
                return Placement::Outdated;
            }
        }
        let from = new_lines(&self.diff.hunks, range.start).start;
        let to = new_lines(&self.diff.hunks, range.end - 1).start + 1;
        Placement::Placed { side, lines: from..to }
    }
}

/// Where lines `start..=end` (1-based) of `anchored` are in `target`.
#[cfg(test)]
fn place(side: Side, start: u32, end: u32, anchored: &[u8], target: &[u8]) -> Placement {
    Compared::new(anchored, target).map_or(Placement::Outdated, |c| c.place(side, start, end))
}

/// Blocking (git, files): every thread's placement, in order. `merge_base` is the commit the left
/// side shows. Threads on the same commit, file and side share one read and one diff.
pub fn place_all(repo: &Repo, threads: &[Thread], merge_base: Option<&str>, files: &[FileChange]) -> Vec<Placement> {
    let Ok(git) = repo.open() else {
        return vec![Placement::Missing; threads.len()];
    };
    let mut cache: HashMap<(&str, &str, Side), Result<Option<Compared>, Placement>> = HashMap::new();
    threads
        .iter()
        .map(|t| {
            let a = &t.anchor;
            let compared = cache
                .entry((&a.commit, &a.path, a.side))
                .or_insert_with(|| compare(repo, &git, a, merge_base, files));
            match compared {
                Ok(Some(c)) => c.place(a.side, a.start_line, a.end_line),
                Ok(None) => Placement::Outdated,
                Err(p) => p.clone(),
            }
        })
        .collect()
}

/// The anchored text against its target; `Err` when the thread is placed without a comparison.
fn compare(
    repo: &Repo,
    git: &Repository,
    a: &Anchor,
    merge_base: Option<&str>,
    files: &[FileChange],
) -> Result<Option<Compared>, Placement> {
    let Some(commit) = Oid::from_str(&a.commit).ok().and_then(|o| git.find_commit(o).ok()) else {
        return Err(Placement::Missing);
    };
    let Some(anchored) = blob_at(git, &commit, &a.path) else {
        return Err(Placement::Outdated);
    };
    let target = match a.side {
        Side::Right => {
            if files.iter().any(|f| f.path == a.path && f.status == 'D') {
                return Err(Placement::Outdated);
            }
            match std::fs::read(repo.workdir.join(&a.path)) {
                Ok(t) => t,
                Err(_) => return Err(Placement::Outdated),
            }
        }
        Side::Left => {
            let Some(base) = merge_base
                .and_then(|b| Oid::from_str(b).ok())
                .and_then(|o| git.find_commit(o).ok())
            else {
                return Err(Placement::Missing);
            };
            let old = files
                .iter()
                .find(|f| f.path == a.path)
                .and_then(|f| f.old_path.as_deref())
                .unwrap_or(&a.path);
            match blob_at(git, &base, old) {
                Some(t) => t,
                None => return Err(Placement::Outdated),
            }
        }
    };
    Ok(Compared::new(&anchored, &target))
}

fn blob_at(git: &Repository, commit: &git2::Commit, path: &str) -> Option<Vec<u8>> {
    let entry = commit.tree().ok()?.get_path(std::path::Path::new(path)).ok()?;
    Some(git.find_blob(entry.id()).ok()?.content().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(side: Side, lines: Range<usize>) -> Placement {
        Placement::Placed { side, lines }
    }

    const OLD: &[u8] = b"a\nb\nc\nd\ne\n";

    #[test]
    fn an_unchanged_text_keeps_the_lines() {
        assert_eq!(place(Side::Right, 2, 2, OLD, OLD), placed(Side::Right, 1..2));
        assert_eq!(place(Side::Right, 2, 4, OLD, OLD), placed(Side::Right, 1..4));
    }

    #[test]
    fn lines_inserted_above_shift_the_thread() {
        let new = b"x\ny\na\nb\nc\nd\ne\n";
        assert_eq!(place(Side::Right, 3, 3, OLD, new), placed(Side::Right, 4..5));
    }

    #[test]
    fn lines_removed_above_shift_the_thread_up() {
        let new = b"c\nd\ne\n";
        assert_eq!(place(Side::Right, 4, 4, OLD, new), placed(Side::Right, 1..2));
    }

    #[test]
    fn an_edited_line_is_outdated() {
        let new = b"a\nB\nc\nd\ne\n";
        assert_eq!(place(Side::Right, 2, 2, OLD, new), Placement::Outdated);
        // Its neighbours are not.
        assert_eq!(place(Side::Right, 3, 3, OLD, new), placed(Side::Right, 2..3));
    }

    #[test]
    fn a_deleted_line_is_outdated() {
        let new = b"a\nc\nd\ne\n";
        assert_eq!(place(Side::Right, 2, 2, OLD, new), Placement::Outdated);
    }

    #[test]
    fn a_multi_line_thread_is_outdated_by_any_changed_line() {
        let new = b"a\nb\nC\nd\ne\n";
        assert_eq!(place(Side::Right, 2, 4, OLD, new), Placement::Outdated);
        assert_eq!(place(Side::Right, 4, 5, OLD, new), placed(Side::Right, 3..5));
    }

    #[test]
    fn lines_inserted_inside_a_range_outdate_it_but_at_its_edges_do_not() {
        let inside = b"a\nb\nX\nc\nd\ne\n";
        assert_eq!(place(Side::Right, 2, 3, OLD, inside), Placement::Outdated);
        let above = b"a\nX\nb\nc\nd\ne\n";
        assert_eq!(place(Side::Right, 2, 3, OLD, above), placed(Side::Right, 2..4));
        let below = b"a\nb\nc\nX\nd\ne\n";
        assert_eq!(place(Side::Right, 2, 3, OLD, below), placed(Side::Right, 1..3));
    }

    #[test]
    fn an_emptied_or_binary_file_is_outdated() {
        assert_eq!(place(Side::Left, 2, 2, OLD, b""), Placement::Outdated);
        assert_eq!(place(Side::Right, 2, 2, OLD, b"a\0b\n"), Placement::Outdated);
    }

    #[test]
    fn lines_past_the_anchored_text_are_outdated() {
        assert_eq!(place(Side::Right, 9, 9, OLD, OLD), Placement::Outdated);
    }

    #[test]
    fn range_edges_are_half_open_for_deletions_and_appends() {
        // Deleting the line just above / below a range leaves it placed.
        assert_eq!(
            place(Side::Right, 3, 3, OLD, b"a\nc\nd\ne\n"),
            placed(Side::Right, 1..2)
        );
        assert_eq!(
            place(Side::Right, 3, 3, OLD, b"a\nb\nc\ne\n"),
            placed(Side::Right, 2..3)
        );
        // Lines added before the first line and after the last one.
        assert_eq!(
            place(Side::Right, 1, 1, OLD, b"x\na\nb\nc\nd\ne\n"),
            placed(Side::Right, 1..2)
        );
        assert_eq!(
            place(Side::Right, 5, 5, OLD, b"a\nb\nc\nd\ne\nx\n"),
            placed(Side::Right, 4..5)
        );
        // The whole file as the range, with an append and a prepend.
        assert_eq!(
            place(Side::Right, 1, 5, OLD, b"x\na\nb\nc\nd\ne\ny\n"),
            placed(Side::Right, 1..6)
        );
    }

    #[test]
    fn degenerate_ranges_and_files() {
        // End before start reads as the start line; line 0 as line 1.
        assert_eq!(place(Side::Right, 3, 1, OLD, OLD), placed(Side::Right, 2..3));
        assert_eq!(place(Side::Right, 0, 0, OLD, OLD), placed(Side::Right, 0..1));
        // An empty anchored file has no line to be on.
        assert_eq!(place(Side::Right, 1, 1, b"", b"a\n"), Placement::Outdated);
        // The last line without a newline is a line.
        assert_eq!(place(Side::Right, 2, 2, b"a\nb", b"x\na\nb"), placed(Side::Right, 2..3));
        // A changed final unterminated line is changed.
        assert_eq!(place(Side::Right, 2, 2, b"a\nb", b"a\nb\n"), Placement::Outdated);
    }

    #[test]
    fn line_ending_changes_outdate_but_crlf_files_place() {
        let crlf = b"a\r\nb\r\nc\r\n";
        assert_eq!(place(Side::Right, 2, 2, crlf, crlf), placed(Side::Right, 1..2));
        assert_eq!(
            place(Side::Right, 3, 3, crlf, b"x\r\na\r\nb\r\nc\r\n"),
            placed(Side::Right, 3..4)
        );
        // Converted to LF: every line differs.
        assert_eq!(place(Side::Right, 2, 2, crlf, b"a\nb\nc\n"), Placement::Outdated);
    }

    #[test]
    fn the_side_is_carried_through() {
        assert_eq!(place(Side::Left, 1, 1, OLD, OLD), placed(Side::Left, 0..1));
    }

    #[test]
    fn threads_are_placed_against_the_commit_and_the_work_tree() {
        use crate::git::test_repo::{commit_all, git, init};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init(root);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        commit_all(root, "base");
        let head = git(root, &["rev-parse", "HEAD"]).trim().to_string();
        let repo = Repo::discover(root).unwrap();
        let thread = |commit: &str, side, line| Thread {
            id: "1".into(),
            anchor: Anchor {
                path: "a.txt".into(),
                side,
                start_line: line,
                end_line: line,
                commit: commit.into(),
            },
            resolved: false,
            outdated: false,
            comments: Vec::new(),
        };
        let change = |status| FileChange {
            path: "a.txt".into(),
            old_path: None,
            status,
            additions: 0,
            deletions: 0,
            hash: String::new(),
        };
        let threads = [
            thread(&head, Side::Right, 3),
            thread(&"d".repeat(40), Side::Right, 3),
            thread(&head, Side::Left, 2),
            thread("not a commit", Side::Right, 1),
        ];
        std::fs::write(root.join("a.txt"), "zero\none\ntwo\nthree\n").unwrap();
        let p = place_all(&repo, &threads, Some(&head), &[change('M')]);
        assert_eq!(p[0], placed(Side::Right, 3..4));
        assert_eq!(p[1], Placement::Missing);
        assert_eq!(p[2], placed(Side::Left, 1..2));
        assert_eq!(p[3], Placement::Missing);
        // A deleted file: the right side is gone, the left (merge base) side stays.
        std::fs::remove_file(root.join("a.txt")).unwrap();
        let p = place_all(&repo, &threads, Some(&head), &[change('D')]);
        assert_eq!(p[0], Placement::Outdated);
        assert_eq!(p[2], placed(Side::Left, 1..2));
        // No merge base known: the left side cannot be judged.
        assert_eq!(place_all(&repo, &threads, None, &[change('D')])[2], Placement::Missing);
    }

    #[test]
    fn a_renamed_files_left_side_is_read_from_its_old_path() {
        use crate::git::test_repo::{commit_all, git, init};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init(root);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        commit_all(root, "base");
        let base = git(root, &["rev-parse", "HEAD"]).trim().to_string();
        git(root, &["mv", "a.txt", "b.txt"]);
        commit_all(root, "rename");
        let head = git(root, &["rev-parse", "HEAD"]).trim().to_string();
        let repo = Repo::discover(root).unwrap();
        let thread = |side| Thread {
            id: "1".into(),
            anchor: Anchor {
                path: "b.txt".into(),
                side,
                start_line: 2,
                end_line: 2,
                commit: head.clone(),
            },
            resolved: false,
            outdated: false,
            comments: Vec::new(),
        };
        let renamed = FileChange {
            path: "b.txt".into(),
            old_path: Some("a.txt".into()),
            status: 'R',
            additions: 0,
            deletions: 0,
            hash: String::new(),
        };
        let p = place_all(
            &repo,
            &[thread(Side::Left), thread(Side::Right)],
            Some(&base),
            &[renamed],
        );
        assert_eq!(p[0], placed(Side::Left, 1..2));
        assert_eq!(p[1], placed(Side::Right, 1..2));
        // Without the old path the merge base has no b.txt: outdated, not a wrong placement.
        let plain = FileChange {
            path: "b.txt".into(),
            old_path: None,
            status: 'M',
            additions: 0,
            deletions: 0,
            hash: String::new(),
        };
        assert_eq!(
            place_all(&repo, &[thread(Side::Left)], Some(&base), &[plain])[0],
            Placement::Outdated
        );
    }
}
