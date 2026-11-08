//! Streamed git history, "Done when": the release budget test
//! `milestone_history_stream_budget_first_chunk` builds a 20,000-commit repository with
//! `git fast-import` and reports the time to the first result for the branch list and for a file
//! touched once near HEAD: from a new `Walker` to the first `step` that shows a commit (the
//! "before" numbers, measured on the old page calls, are in `tasks/history_stream.md`). Asserted
//! in release only.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::git::Repo;
use crate::git::log::{self, Walker};
use crate::git::test_repo::{fast_import, init};

const COMMITS: usize = 20_000;

fn report(what: &str, took: Duration, budget: Duration) {
    println!("{what}: first result in {took:?}");
    if !cfg!(debug_assertions) {
        assert!(took < budget, "{what} too slow: {took:?} > {budget:?}");
    }
}

#[test]
fn milestone_history_stream_budget_first_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    init(&root);
    // target.txt: the first commit and one near HEAD.
    fast_import(&root, COMMITS, |i| i == 0 || i == COMMITS - 10);
    let repo = Repo::discover(&root).unwrap();
    let budget = Duration::from_millis(100);

    let (took, found) = first_result(&repo, None);
    assert!(!found.is_empty());
    report("branch", took, budget);

    let (took, found) = first_result(&repo, Some(Path::new("target.txt")));
    assert_eq!(found[0].summary, format!("c{}", COMMITS - 10));
    report("file", took, budget);
}

/// Time from a new walker to the first `step` that shows a commit.
fn first_result(repo: &Repo, path: Option<&Path>) -> (Duration, Vec<log::Commit>) {
    let t = Instant::now();
    let mut walker = Walker::new(repo, path).unwrap();
    loop {
        let step = walker.step(log::PAGE).unwrap();
        if !step.found.is_empty() || step.done {
            return (t.elapsed(), step.found);
        }
    }
}
