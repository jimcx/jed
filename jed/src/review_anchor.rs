//! Where a review thread sits now: the text it was written against (the blob of its anchor commit)
//! diffed against the text it is shown on, with its lines mapped through the hunks.
//!
//! A thread on the right side (its anchor commit's file) is compared with the work tree's file, one
//! on the left (a deleted line) with the merge base's, the left side of the review diff. Forges give
//! a left thread its head commit too, but its line numbers count the base's lines: that text is read
//! from the merge base of the anchor commit and the review's merge base. When every anchored line is
//! unchanged the thread is [`Placement::Placed`] at the mapped lines; a line inside a changed
//! hunk (or lines inserted between the anchored ones, or a vanished file) makes it
//! [`Placement::Outdated`]; a commit that isn't in the clone makes it [`Placement::Missing`].
//!
//! A thread on a commit the clone lacks (the local branch is behind the merge request) is
//! [`place_behind`]'d by comparing the host's blob id of the file there with the shown one.
//!
//! [`place_all`] runs at lookup time (off-thread) and so judges the file on disk, not unsaved
//! edits; the next refresh sees an edit once it is saved.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use git2::{Oid, Repository};

use crate::forge::{Anchor, Forge, RepoRef, Side, Thread};
use crate::git::Repo;
use crate::git::diff::{self, new_lines, touches};
use crate::git::range::FileChange;
use crate::logs::ToolLog;

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
    let (anchored, target) = match a.side {
        Side::Right => {
            let Some(anchored) = blob_at(git, &commit, &a.path) else {
                return Err(Placement::Outdated);
            };
            if files.iter().any(|f| f.path == a.path && f.status == 'D') {
                return Err(Placement::Outdated);
            }
            match std::fs::read(repo.workdir.join(&a.path)) {
                Ok(t) => (anchored, t),
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
            // A left line number counts lines of the base the thread was written against, not of
            // its (head) commit: the merge base of that commit with today's base (the base itself
            // while the target branch hasn't moved).
            let Some(then) = git
                .merge_base(commit.id(), base.id())
                .ok()
                .and_then(|o| git.find_commit(o).ok())
            else {
                return Err(Placement::Outdated);
            };
            match (blob_at(git, &then, old), blob_at(git, &base, old)) {
                (Some(anchored), Some(target)) => (anchored, target),
                _ => return Err(Placement::Outdated),
            }
        }
    };
    Ok(Compared::new(&anchored, &target))
}

/// Most forge requests [`place_behind`] makes in one lookup.
pub const BEHIND_REQUESTS: usize = 50;

/// The host's blob ids already asked for, by (repository, commit, path); `None` is a file the host
/// doesn't have there. A blob id at a commit never changes, so lookups share it; errors aren't kept.
pub type BlobCache = Arc<Mutex<HashMap<(RepoRef, String, String), Option<String>>>>;

/// Blocking (git, files, network): places the `Missing` threads whose anchor commit the clone lacks
/// (the local branch is behind the merge request) when the file hasn't changed since: the host's
/// blob id of the file at the anchor commit equals the one shown (the work tree's for the right
/// side, the merge base's for the left). Such a thread becomes `Placed` at its own lines; the
/// returned flags mark them. One request per distinct (commit, path), at most [`BEHIND_REQUESTS`]
/// (logged when hit) and answers kept in `cache`; an error leaves the thread missing. Hashing the
/// work tree's bytes ignores `.gitattributes` filters (autocrlf): a file the host stores differently
/// just stays missing.
#[allow(clippy::too_many_arguments)]
pub fn place_behind(
    repo: &Repo,
    api: &dyn Forge,
    remote: &RepoRef,
    threads: &[Thread],
    places: &mut [Placement],
    merge_base: Option<&str>,
    files: &[FileChange],
    cache: &BlobCache,
    log: &ToolLog,
) -> Vec<bool> {
    let mut behind = vec![false; threads.len()];
    if !places.contains(&Placement::Missing) {
        return behind;
    }
    let Ok(git) = repo.open() else {
        return behind;
    };
    let mut local: HashMap<(&str, Side), Option<String>> = HashMap::new();
    let mut asked: HashMap<(&str, &str), Option<String>> = HashMap::new();
    let mut requests = 0;
    let mut capped = false;
    for (i, t) in threads.iter().enumerate() {
        let a = &t.anchor;
        if places[i] != Placement::Missing || a.commit.is_empty() {
            continue;
        }
        let key = (a.commit.as_str(), a.path.as_str());
        let theirs = match asked.entry(key) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let ckey = (remote.clone(), a.commit.clone(), a.path.clone());
                let cached = cache.lock().ok().and_then(|c| c.get(&ckey).cloned());
                match cached {
                    Some(v) => e.insert(v),
                    None => {
                        if requests >= BEHIND_REQUESTS {
                            capped = true;
                            continue;
                        }
                        requests += 1;
                        match api.blob_id(remote, &a.commit, &a.path) {
                            Ok(v) => {
                                if let Ok(mut c) = cache.lock() {
                                    c.insert(ckey, v.clone());
                                }
                                e.insert(v)
                            }
                            Err(err) => {
                                log.error(&format!("blob of {} at {}: {err}", a.path, a.commit));
                                e.insert(None)
                            }
                        }
                    }
                }
            }
        };
        let Some(theirs) = theirs.as_ref() else {
            continue;
        };
        let ours = local
            .entry((a.path.as_str(), a.side))
            .or_insert_with(|| local_blob_id(repo, &git, a, merge_base, files));
        if ours.as_ref() == Some(theirs) {
            let lines = (a.start_line.max(1) - 1) as usize..a.end_line.max(a.start_line).max(1) as usize;
            places[i] = Placement::Placed { side: a.side, lines };
            behind[i] = true;
        }
    }
    if capped {
        log.info(&format!(
            "Checked {BEHIND_REQUESTS} files at newer commits; threads on other files stay missing"
        ));
    }
    behind
}

/// The git blob id of what the thread's file is shown as: the work tree's file (right side) or the
/// merge base's (left side, at the old path of a rename).
fn local_blob_id(
    repo: &Repo,
    git: &Repository,
    a: &Anchor,
    merge_base: Option<&str>,
    files: &[FileChange],
) -> Option<String> {
    match a.side {
        Side::Right => {
            if files.iter().any(|f| f.path == a.path && f.status == 'D') {
                return None;
            }
            let bytes = std::fs::read(repo.workdir.join(&a.path)).ok()?;
            Some(Oid::hash_object(git2::ObjectType::Blob, &bytes).ok()?.to_string())
        }
        Side::Left => {
            let base = git.find_commit(Oid::from_str(merge_base?).ok()?).ok()?;
            let old = files
                .iter()
                .find(|f| f.path == a.path)
                .and_then(|f| f.old_path.as_deref())
                .unwrap_or(&a.path);
            let entry = base.tree().ok()?.get_path(std::path::Path::new(old)).ok()?;
            Some(entry.id().to_string())
        }
    }
}

/// The code a thread was written on, for its tab: the anchored lines with context, in the
/// anchor commit's numbering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    /// The number (1-based) of the first line.
    pub first: usize,
    /// The lines, each ending in a line break.
    pub text: String,
    /// The index of the last anchored line in `text`'s lines.
    pub last: usize,
}

/// Lines of context before the anchored lines in a [`Snippet`].
const CONTEXT: usize = 3;

/// Blocking (git): the thread's code. A right-side thread reads the blob at its anchor commit
/// (when that commit is in the clone); else the forge's diff hunk; a left-side thread whose hunk
/// is missing reads the merge base's file.
///
/// `blobs` caches the reads per (commit, path): a file's threads share them.
pub fn snippet(
    git: &Repository,
    t: &Thread,
    merge_base: Option<&str>,
    blobs: &mut std::collections::HashMap<(String, String), Option<Vec<u8>>>,
) -> Option<Snippet> {
    let a = &t.anchor;
    let find = |rev: &str| Oid::from_str(rev).ok().and_then(|o| git.find_commit(o).ok());
    let rev = match a.side {
        Side::Right => Some(a.commit.as_str()),
        Side::Left if t.hunk.is_none() => merge_base,
        Side::Left => None,
    };
    let blob = rev.and_then(|rev| {
        blobs
            .entry((rev.to_string(), a.path.clone()))
            .or_insert_with(|| find(rev).and_then(|c| blob_at(git, &c, &a.path)))
            .as_deref()
    });
    if let Some(blob) = blob {
        let text = String::from_utf8_lossy(blob);
        let lines: Vec<&str> = text.lines().collect();
        let end = (a.end_line as usize).min(lines.len());
        if end == 0 {
            return None;
        }
        let from = (a.start_line as usize).clamp(1, end).saturating_sub(CONTEXT).max(1);
        let text: String = lines[from - 1..end].iter().map(|l| format!("{l}\n")).collect();
        return Some(Snippet {
            first: from,
            text,
            last: end - from,
        });
    }
    hunk_snippet(t.hunk.as_deref()?, a)
}

/// The anchored side's lines of a diff hunk (`@@ -a,b +c,d @@`, then ` `, `+`, `-` lines), the
/// last ones: the hunk ends at the commented line.
fn hunk_snippet(hunk: &str, a: &Anchor) -> Option<Snippet> {
    let mut lines = hunk.lines();
    let header = lines.next()?.strip_prefix("@@ ")?;
    let mut parts = header.split(' ');
    let (old, new) = (parts.next()?, parts.next()?);
    let from = match a.side {
        Side::Left => old.strip_prefix('-')?,
        Side::Right => new.strip_prefix('+')?,
    };
    let mut number: usize = from.split(',').next()?.parse().ok()?;
    let mut kept: Vec<(usize, &str)> = Vec::new();
    for l in lines {
        let (mark, rest) = l.split_at(l.chars().next().map_or(0, char::len_utf8));
        let on_side = !matches!((mark, a.side), ("+", Side::Left) | ("-", Side::Right) | ("\\", _));
        if on_side {
            kept.push((number, rest));
            number += 1;
        }
    }
    let span = (a.end_line - a.start_line.min(a.end_line)) as usize + 1;
    let kept = &kept[kept.len().saturating_sub(span + CONTEXT)..];
    let first = kept.first()?.0;
    Some(Snippet {
        first,
        text: kept.iter().map(|(_, l)| format!("{l}\n")).collect(),
        last: kept.len() - 1,
    })
}

pub(crate) fn blob_at(git: &Repository, commit: &git2::Commit, path: &str) -> Option<Vec<u8>> {
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

    fn anchor(side: Side, start: u32, end: u32) -> Anchor {
        Anchor {
            path: "a.txt".into(),
            side,
            start_line: start,
            end_line: end,
            commit: String::new(),
        }
    }

    #[test]
    fn a_hunk_gives_the_anchored_sides_last_lines() {
        let hunk = "@@ -10,4 +20,4 @@\n a\n-b\n+B\n c\n+d\n";
        let right = hunk_snippet(hunk, &anchor(Side::Right, 23, 23)).unwrap();
        assert_eq!((right.first, right.text.as_str(), right.last), (20, "a\nB\nc\nd\n", 3));
        let hunk = "@@ -10,4 +20,4 @@\n a\n-b\n+B\n c\n-d\n";
        let left = hunk_snippet(hunk, &anchor(Side::Left, 13, 13)).unwrap();
        assert_eq!((left.first, left.text.as_str(), left.last), (10, "a\nb\nc\nd\n", 3));
        assert!(hunk_snippet("not a hunk", &anchor(Side::Right, 1, 1)).is_none());
    }

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
            hunk: None,
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
    fn a_left_thread_counts_the_base_lines_not_its_head_commits() {
        // A forge anchors a comment on a deleted line to the head commit, but numbers the line in
        // the base: lines added above it on the branch must not shift it.
        use crate::git::test_repo::{commit_all, git, init};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init(root);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        commit_all(root, "base");
        let base = git(root, &["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(root.join("a.txt"), "new1\nnew2\none\ntwo\nfour\n").unwrap();
        commit_all(root, "add two lines, delete three");
        let head = git(root, &["rev-parse", "HEAD"]).trim().to_string();
        let repo = Repo::discover(root).unwrap();
        let thread = Thread {
            id: "1".into(),
            anchor: Anchor {
                path: "a.txt".into(),
                side: Side::Left,
                start_line: 3,
                end_line: 3,
                commit: head,
            },
            resolved: false,
            outdated: false,
            comments: Vec::new(),
            hunk: None,
        };
        let change = FileChange {
            path: "a.txt".into(),
            old_path: None,
            status: 'M',
            additions: 2,
            deletions: 1,
            hash: String::new(),
        };
        let p = place_all(&repo, &[thread], Some(&base), &[change]);
        assert_eq!(
            p[0],
            placed(Side::Left, 2..3),
            "base line 3 (\"three\"), not shifted by the added lines"
        );
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
            hunk: None,
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
