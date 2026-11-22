//! Streamed inline blame, "Done when" (subtask a): the release budget test
//! `milestone_blame_stream_budget` builds a repository with `git2` (20,000 commits, a file of a few
//! thousand lines touched by about 1,000 of them) and reports the time to the first chunk and to
//! the whole file, before (libgit2's `blame_file`, as `blame::blame_head` calls it) and after (the
//! `git::blame::Walk`). Asserted in release only: the first chunk in under 100 ms, the whole file at
//! least 5 times faster than before. The walk's line-for-line comparison with `git blame` is in
//! `git::blame`'s tests.

use std::path::Path;
use std::time::{Duration, Instant};

use git2::{Oid, Repository, Signature, Time};

use crate::git::Repo;
use crate::git::blame::Walk;

/// A repository of `commits` commits (each changes `other.txt`), every `every`th one also editing
/// a few lines of `target.txt` (`lines` lines long at first); returns the target's line count at
/// HEAD.
fn build(dir: &Path, commits: usize, every: usize, lines: usize) -> usize {
    let repo = Repository::init(dir).unwrap();
    let mut text: Vec<String> = (0..lines).map(|i| format!("original line {i}")).collect();
    let mut seed = 12345u64;
    let mut next = move |n: usize| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize % n
    };
    let blob_of = |repo: &Repository, text: &[String]| {
        let joined: String = text.iter().map(|l| format!("{l}\n")).collect();
        repo.blob(joined.as_bytes()).unwrap()
    };
    let mut parent: Option<Oid> = None;
    let mut target = blob_of(&repo, &text);
    let mut touched = 0;
    for i in 0..commits {
        if i % every == 0 {
            touched += 1;
            for _ in 0..3 {
                let at = next(text.len());
                text[at] = format!("line edited in commit {i} ({})", next(1000));
            }
            // An insertion and a deletion shift the lines below them.
            let at = next(text.len());
            text.insert(at, format!("line added in commit {i}"));
            let at = next(text.len());
            text.remove(at);
            target = blob_of(&repo, &text);
        }
        let mut tree = repo.treebuilder(None).unwrap();
        tree.insert("target.txt", target, 0o100644).unwrap();
        let other = repo.blob(format!("{i}\n").as_bytes()).unwrap();
        tree.insert("other.txt", other, 0o100644).unwrap();
        let tree = repo.find_tree(tree.write().unwrap()).unwrap();
        let sig = Signature::new("Alice", "alice@example.com", &Time::new(1_600_000_000 + 60 * i as i64, 0)).unwrap();
        let parents: Vec<git2::Commit> = parent.iter().map(|p| repo.find_commit(*p).unwrap()).collect();
        let parents: Vec<&git2::Commit> = parents.iter().collect();
        parent = Some(repo.commit(None, &sig, &sig, &format!("c{i}"), &tree, &parents).unwrap());
    }
    repo.reference("refs/heads/main", parent.unwrap(), true, "budget").unwrap();
    repo.set_head("refs/heads/main").unwrap();
    println!("built {commits} commits, {touched} touching target.txt");
    text.len()
}

#[test]
fn milestone_blame_stream_budget() {
    let (commits, every, lines) = if cfg!(debug_assertions) {
        (1_000, 20, 400)
    } else {
        (20_000, 20, 3_000)
    };
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let total = build(&root, commits, every, lines);
    let repo = Repo::discover(&root).unwrap();
    let path = Path::new("target.txt");

    // Before: libgit2's blame of the whole file.
    let r = repo.open().unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap().id();
    let t = Instant::now();
    crate::blame::blame_head(&r, head, path).unwrap().unwrap();
    let before = t.elapsed();

    // After: the walk, stepping with the default slice.
    let t = Instant::now();
    let mut walk = Walk::new(&repo, path).unwrap().unwrap();
    let (mut first, mut settled) = (None, 0);
    loop {
        let step = walk.step().unwrap();
        settled += step.settled.iter().map(|s| s.count).sum::<usize>();
        if first.is_none() && !step.settled.is_empty() {
            first = Some(t.elapsed());
        }
        if step.done {
            break;
        }
    }
    let whole = t.elapsed();
    let first = first.unwrap();
    assert_eq!(settled, total, "every line settled once");

    println!("before (libgit2 blame_file): whole file in {before:?}");
    println!(
        "after (walk): first chunk in {first:?}, whole file in {whole:?}, {} commits taken",
        walk.visited()
    );
    if !cfg!(debug_assertions) {
        assert!(first < Duration::from_millis(100), "first chunk too slow: {first:?}");
        assert!(whole * 5 <= before, "whole file not 5x faster: {whole:?} vs {before:?}");
    }
}
