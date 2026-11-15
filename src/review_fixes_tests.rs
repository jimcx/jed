//! Review fixes, subtask a: the review's FILES list only the merge request's own files (the merge
//! base's tree against the MR head, else local HEAD), shown as the work tree has them: untracked
//! files and locally edited files outside the MR are hidden, an MR file edited locally shows with
//! the edit's counts, and a local HEAD behind the MR still lists the MR's files.

use crate::git::Repo;
use crate::git::range::{FileChange, changes};
use crate::git::test_repo::{commit_all, git, init};

/// A repo: `base` on main with a.txt, b.txt, c.txt; branch `feature` changes a.txt and adds mr.txt.
/// Returns (dir, base sha, feature sha); `main` is checked out afterwards when `behind`.
fn setup(behind: bool) -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    init(p);
    for f in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(p.join(f), "one\ntwo\n").unwrap();
    }
    commit_all(p, "base");
    let base = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    git(p, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(p.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(p.join("mr.txt"), "m\n").unwrap();
    commit_all(p, "work");
    let head = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    if behind {
        git(p, &["checkout", "-q", "main"]);
    }
    (dir, base, head)
}

fn names(files: &[FileChange]) -> Vec<&str> {
    let mut v: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    v.sort();
    v
}

#[test]
fn review_fixes_untracked_and_unrelated_edits_are_hidden() {
    let (dir, base, head) = setup(false);
    let p = dir.path();
    std::fs::write(p.join("untracked.txt"), "u\n").unwrap();
    std::fs::write(p.join("b.txt"), "one\ntwo\nlocal\n").unwrap();
    let repo = Repo::discover(p).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    assert_eq!(names(&files), ["a.txt", "mr.txt"]);
}

#[test]
fn review_fixes_a_local_edit_of_an_mr_file_shows_its_counts() {
    let (dir, base, head) = setup(false);
    let p = dir.path();
    std::fs::write(p.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let repo = Repo::discover(p).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    let a = files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!(a.additions, 3);
    assert_eq!(a.status, 'M');
}

#[test]
fn review_fixes_an_mr_file_deleted_locally_shows_as_deleted() {
    let (dir, base, head) = setup(false);
    let p = dir.path();
    std::fs::remove_file(p.join("a.txt")).unwrap();
    let repo = Repo::discover(p).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    assert_eq!(files.iter().find(|f| f.path == "a.txt").unwrap().status, 'D');
}

#[test]
fn review_fixes_local_head_behind_the_mr_lists_the_mrs_files() {
    let (dir, base, head) = setup(true);
    let repo = Repo::discover(dir.path()).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    assert_eq!(names(&files), ["a.txt", "mr.txt"]);
    // Without the MR head in the clone, local HEAD (main, no changes) lists nothing.
    let none = changes(&repo, &base, Some(&"0".repeat(40))).unwrap();
    assert!(none.is_empty());
}

#[test]
fn review_fixes_an_appended_file_hashes_the_mrs_blob_not_the_work_tree() {
    let (dir, base, head) = setup(true);
    let repo = Repo::discover(dir.path()).unwrap();
    let files = changes(&repo, &base, Some(&head)).unwrap();
    let mr = files.iter().find(|f| f.path == "mr.txt").unwrap();
    let blob = git(dir.path(), &["rev-parse", &format!("{head}:mr.txt")]);
    assert_eq!(mr.hash, blob.trim());
}
