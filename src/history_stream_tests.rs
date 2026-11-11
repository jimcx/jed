//! Streamed git history, "Done when": the Explorer section and the Source Control pane show a
//! walk's first commit before it reaches the root (`history_stream_*_first_chunk_*`, the walk
//! held back after its first chunk by `App::stream_probe`), then the full lists are right; scrolling
//! a branch's history to its end continues one walk (`history_stream_scrolling_*`); switching the
//! file or HEAD moving cancels the running job and starts another
//! (`history_stream_switching_*`, `history_stream_head_moving_*`); and the release budget test
//! `milestone_history_stream_budget_first_chunk` builds a 20,000-commit repository with
//! `git fast-import` and reports the time to the first chunk (from a new `Walker` through
//! `log::stream`) for the branch list and for a file touched once near HEAD (the "before" numbers,
//! measured on the old page calls, are in `tasks/history_stream.md`). Asserted in release only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::Command;
use crate::git::Repo;
use crate::git::log::{self, Walker};
use crate::git::test_repo::{commit_all, fast_import, git, init};
use crate::layout::SidePanel;
use crate::milestone10_tests::{dirs, open_file_app};

const COMMITS: usize = 20_000;

fn report(what: &str, took: Duration, budget: Duration) {
    println!("{what}: first chunk in {took:?}");
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

/// Time from a new walker to the first chunk the stream driver sends.
fn first_result(repo: &Repo, path: Option<&Path>) -> (Duration, Vec<log::Commit>) {
    let t = Instant::now();
    let walker = Walker::new(repo, path).unwrap();
    let (_want, rx) = mpsc::channel();
    let cancel = AtomicBool::new(false);
    let mut first = None;
    log::stream(walker, log::PAGE, &rx, &cancel, |chunk| {
        first = Some((t.elapsed(), chunk.unwrap().commits));
        false
    });
    first.unwrap()
}

/// The driver on its own: chunks add up to the walk, `wanted` bounds what it sends before it
/// waits, a larger request resumes it, and a dropped sender ends it.
#[test]
fn history_stream_driver_sends_in_chunks_and_waits() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    init(&root);
    fast_import(&root, 450, |_| false);
    let repo = Repo::discover(&root).unwrap();
    let expected = cli(&root, &["log", "--format=%H"]);

    let (want, rx) = mpsc::channel();
    let cancel = AtomicBool::new(false);
    let (repo_ref, cancel_ref) = (&repo, &cancel);
    let (out, got) = mpsc::channel();
    std::thread::scope(|scope| {
        let job = scope.spawn(move || {
            let walker = Walker::new(repo_ref, None).unwrap();
            log::stream(walker, 100, &rx, cancel_ref, |chunk| out.send(chunk).is_ok());
        });
        let mut ids = Vec::new();
        let mut done = false;
        while ids.len() < 100 {
            let chunk = got.recv().unwrap().unwrap();
            ids.extend(chunk.commits.into_iter().map(|c| c.id));
            done |= chunk.done;
        }
        assert!(!done);
        assert_eq!(ids, expected[..100], "exactly what was asked for, then it waits");
        want.send(1000).unwrap();
        while !done {
            let chunk = got.recv().unwrap().unwrap();
            ids.extend(chunk.commits.into_iter().map(|c| c.id));
            done = chunk.done;
        }
        assert_eq!(ids, expected);
        job.join().unwrap();
    });

    // Waiting for a request, a dropped sender ends the job.
    let (want, rx) = mpsc::channel();
    let (repo_ref, cancel_ref) = (&repo, &cancel);
    std::thread::scope(|scope| {
        let job = scope.spawn(move || {
            let walker = Walker::new(repo_ref, None).unwrap();
            log::stream(walker, 5, &rx, cancel_ref, |_| true);
        });
        drop(want);
        job.join().unwrap();
    });
}

/// 5,000 commits, `target.txt` touched by the first and the last only (`other.txt` by all).
fn big_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (d, root, state) = dirs();
    init(&root);
    fast_import(&root, 5000, |i| i == 0 || i == 4999);
    (d, root, state)
}

fn draw(app: &mut App) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

fn row_text(term: &Terminal<TestBackend>, y: u16) -> String {
    let buf = term.backend().buffer();
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()
}

fn cli(root: &Path, args: &[&str]) -> Vec<String> {
    git(root, args).lines().map(str::to_string).collect()
}

fn ids(h: &crate::git_history::GitHistory) -> Vec<String> {
    h.rows().iter().map(|c| c.id.clone()).collect()
}

/// The Explorer section shows the newest commit while the walk is still going (held after its
/// first chunk), with the loading row under it; the full list is then right.
#[tokio::test(flavor = "multi_thread")]
async fn history_stream_file_first_chunk_arrives_before_the_walk_ends() {
    let (_d, root, state) = big_repo();
    let mut app = open_file_app(&root, &state, "target.txt");
    let probe = app.stream_probe.clone();
    probe.hold(true);
    assert!(app.settle(|a| !a.git_history.commits.is_empty()).await);
    assert_eq!(app.git_history.commits.len(), 1);
    assert_eq!(app.git_history.commits[0].summary, "c4999");
    assert!(app.git_history.more, "the walk has not reached the root");
    assert!(app.git_history.loading);
    let term = draw(&mut app);
    let hit = app.hits.history.expect("the section shows");
    assert!(row_text(&term, hit.list.y).contains("c4999"));
    assert!(
        row_text(&term, hit.list.y + 1).contains("Reading history…"),
        "the loading row"
    );

    probe.hold(false);
    assert!(
        app.settle(|a| !a.git_history.loading && a.git_history.commits.len() == 2)
            .await
    );
    assert!(!app.git_history.more);
    assert_eq!(
        ids(&app.git_history),
        cli(&root, &["log", "--format=%H", "--", "target.txt"])
    );
    let term = draw(&mut app);
    assert!(!(0..24).any(|y| row_text(&term, y).contains("Reading history")));
    assert!(app.settle(|a| a.stream_probe.exited.load(Ordering::SeqCst) == 1).await);
}

/// The Source Control pane's first commit arrives before the walk ends, and the first page
/// matches the CLI.
#[tokio::test(flavor = "multi_thread")]
async fn history_stream_branch_first_chunk_arrives_before_the_walk_ends() {
    let (_d, root, state) = big_repo();
    let mut app = open_file_app(&root, &state, "target.txt");
    let probe = app.stream_probe.clone();
    probe.hold(true);
    app.test_execute(Command::FocusScm);
    assert_eq!(app.ui.side_panel, SidePanel::SourceControl);
    let _ = draw(&mut app);
    assert!(app.settle(|a| !a.branch_history.commits.is_empty()).await);
    assert_eq!(app.branch_history.commits.len(), 1);
    assert_eq!(app.branch_history.commits[0].summary, "c4999");
    assert!(app.branch_history.more && app.branch_history.loading);

    probe.hold(false);
    assert!(
        app.settle(|a| !a.branch_history.loading && a.branch_history.commits.len() == log::PAGE)
            .await
    );
    assert!(app.branch_history.more);
    assert_eq!(
        ids(&app.branch_history),
        cli(&root, &["log", "--format=%H", "-n", "200"])
    );
}

/// Scrolling a 450-commit branch history to its end continues the one walk.
#[tokio::test(flavor = "multi_thread")]
async fn history_stream_scrolling_continues_the_same_walk() {
    let (_d, root, state) = dirs();
    init(&root);
    fast_import(&root, 450, |_| false);
    let mut app = open_file_app(&root, &state, "other.txt");
    app.test_execute(Command::FocusScm);
    let _ = draw(&mut app);
    assert!(
        app.settle(|a| !a.branch_history.loading && a.branch_history.commits.len() == log::PAGE)
            .await
    );
    for _ in 0..20 {
        if !app.branch_history.more {
            break;
        }
        let _ = draw(&mut app);
        app.branch_history.scroll_by(100_000);
        assert!(app.settle(|a| !a.branch_history.loading).await);
    }
    assert!(!app.branch_history.more);
    assert_eq!(ids(&app.branch_history), cli(&root, &["log", "--format=%H"]));
    assert_eq!(
        app.stream_probe.walks.load(Ordering::SeqCst),
        1,
        "one walk from HEAD, not one per page"
    );
    assert!(app.settle(|a| a.stream_probe.exited.load(Ordering::SeqCst) == 1).await);
}

/// Switching the active file mid-walk cancels the job: it exits without another chunk. Back on
/// the file a new walk starts.
// FLAKE: 2026-11-09, hung for over 60 s in a full `scripts/check` run under load (passes alone in
// 0.5 s). Likely an assertion failed while the probe was held: the held `spawn_blocking` job then
// never wakes, so the runtime's shutdown waits on it and the failure shows as a hang. `Release`
// lets the panic surface; the messages dump the state. Seen again the same day and in CI on
// 2026-11-11: "at first chunk sent: walks 1 sent 0 exited 0, commits 1": the job's `send`
// (app/git_history.rs) put the chunk on the channel before bumping `probe.sent`, so the app could
// show it before the counter moved. Fixed: `sent` is now bumped before the send.
#[tokio::test(flavor = "multi_thread")]
async fn history_stream_switching_files_cancels_the_walk() {
    /// FLAKE debugging: releases the probe when the test unwinds, so a failure isn't a hang.
    struct Release<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Release<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let state_of = |a: &App, step: &str| {
        let p = &a.stream_probe;
        format!(
            "FLAKE history_stream_switching_files_cancels_the_walk at {step}: walks {} sent {} exited {}, \
             path {:?}, commits {}, stream {}, loading {}",
            p.walks.load(Ordering::SeqCst),
            p.sent.load(Ordering::SeqCst),
            p.exited.load(Ordering::SeqCst),
            a.git_history.path,
            a.git_history.commits.len(),
            a.git_history.stream.is_some(),
            a.git_history.loading,
        )
    };
    let (d, root, state) = big_repo();
    let outside = d.path().join("outside.txt");
    std::fs::write(&outside, "x\n").unwrap();
    let mut app = open_file_app(&root, &state, "target.txt");
    let probe = app.stream_probe.clone();
    let held = probe.clone();
    let _release = Release(move || held.hold(false));
    probe.hold(true);
    let ok = app.settle(|a| a.git_history.commits.len() == 1).await;
    assert!(ok, "{}", state_of(&app, "first chunk"));
    assert_eq!(
        probe.sent.load(Ordering::SeqCst),
        1,
        "{}",
        state_of(&app, "first chunk sent")
    );

    // A file outside the repository: no new job, the held one is cancelled.
    app.test_open(Buffer::open(&outside).unwrap());
    let ok = app
        .settle(|a| a.git_history.path.as_deref() == Some(outside.as_path()))
        .await;
    assert!(ok, "{}", state_of(&app, "outside file"));
    assert!(
        app.git_history.commits.is_empty() && app.git_history.stream.is_none(),
        "{}",
        state_of(&app, "outside file cleared")
    );
    probe.hold(false);
    let ok = app.settle(|a| a.stream_probe.exited.load(Ordering::SeqCst) == 1).await;
    assert!(ok, "{}", state_of(&app, "held job exits"));
    assert_eq!(
        probe.sent.load(Ordering::SeqCst),
        1,
        "the cancelled job sent nothing more"
    );
    assert!(app.git_history.commits.is_empty(), "and nothing of it shows");

    // Back on the file: a second walk.
    app.test_open(Buffer::open(&root.join("target.txt")).unwrap());
    assert!(
        app.settle(|a| !a.git_history.loading && a.git_history.commits.len() == 2)
            .await
    );
    assert_eq!(probe.walks.load(Ordering::SeqCst), 2);
    assert_eq!(
        ids(&app.git_history),
        cli(&root, &["log", "--format=%H", "--", "target.txt"])
    );
}

/// A commit made outside jed (HEAD moves) mid-walk cancels the job and starts another; the old
/// rows stay up until the new walk covers them, and the first commit is selected (the existing
/// reload rule).
#[tokio::test(flavor = "multi_thread")]
async fn history_stream_head_moving_restarts_the_walk() {
    let (_d, root, state) = big_repo();
    let mut app = open_file_app(&root, &state, "target.txt");
    let probe = app.stream_probe.clone();
    probe.hold(true);
    assert!(app.settle(|a| a.git_history.commits.len() == 1).await);
    let old_generation = app.git_history.generation;

    std::fs::write(root.join("target.txt"), "new\n").unwrap();
    commit_all(&root, "outside jed");
    assert!(app.settle(|a| a.stream_probe.walks.load(Ordering::SeqCst) == 2).await);
    assert!(app.git_history.generation > old_generation);
    // The new walk's first chunk is staged; the old row is still the list.
    assert!(app.settle(|a| !a.git_history.staged.is_empty()).await);
    assert_eq!(app.git_history.commits.len(), 1);
    assert_eq!(app.git_history.commits[0].summary, "c4999");

    probe.hold(false);
    let expected = cli(&root, &["log", "--format=%H", "--", "target.txt"]);
    assert_eq!(expected.len(), 3);
    assert!(
        app.settle(|a| !a.git_history.loading && a.git_history.commits.len() == 3)
            .await
    );
    assert_eq!(ids(&app.git_history), expected);
    assert_eq!(app.git_history.selected(), Some(expected[0].as_str()));
    assert_eq!(app.git_history.scroll(), 0);
    assert!(
        app.settle(|a| a.stream_probe.exited.load(Ordering::SeqCst) == 2).await,
        "both jobs exited"
    );
}

fn click(app: &mut App, x: u16, y: u16) {
    use crossterm::event::{MouseButton, MouseEventKind};
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// A click on the loading row selects nothing and opens nothing (and a commit row still does).
#[tokio::test(flavor = "multi_thread")]
async fn history_stream_clicking_the_loading_row_does_nothing() {
    let (_d, root, state) = big_repo();
    let mut app = open_file_app(&root, &state, "target.txt");
    let probe = app.stream_probe.clone();
    probe.hold(true);
    assert!(app.settle(|a| a.git_history.commits.len() == 1).await);
    let term = draw(&mut app);
    let hit = app.hits.history.expect("the section shows");
    assert!(row_text(&term, hit.list.y + 1).contains("Reading history…"));
    assert!(app.git_history.selected().is_none());
    assert!(app.active_commit().is_none());

    click(&mut app, hit.list.x + 3, hit.list.y + 1);
    assert!(app.git_history.selected().is_none(), "the only commit was not selected");
    assert!(app.active_commit().is_none(), "no commit tab opened");

    // The commit row above it still selects.
    click(&mut app, hit.list.x + 3, hit.list.y);
    assert_eq!(app.git_history.selected_row(), Some(0));
    assert!(app.active_commit().is_some(), "a commit row opens its commit");
    probe.hold(false);
}

/// A reload shows a commit made outside jed as soon as the new rows hold the selected commit,
/// before the walk ends (the walk is held after its first chunk, the next one handed in by
/// hand); until then the old rows stay up.
#[tokio::test(flavor = "multi_thread")]
async fn history_stream_reload_swaps_once_the_old_selection_is_covered() {
    let (_d, root, state) = big_repo();
    let mut app = open_file_app(&root, &state, "target.txt");
    let probe = app.stream_probe.clone();
    assert!(
        app.settle(|a| !a.git_history.loading && a.git_history.commits.len() == 2)
            .await
    );
    let old = app.git_history.commits.clone();
    let path = app.git_history.path.clone().unwrap();
    app.git_history.select_row(1);

    probe.hold(true);
    std::fs::write(root.join("target.txt"), "new\n").unwrap();
    commit_all(&root, "outside jed");
    assert!(app.settle(|a| !a.git_history.staged.is_empty()).await);
    // The first chunk (the new commit) doesn't hold the selected commit: nothing changed.
    assert_eq!(app.git_history.staged.len(), 1);
    assert_eq!(ids(&app.git_history), [old[0].id.clone(), old[1].id.clone()]);
    assert!(app.git_history.fresh && app.git_history.more);

    // The walk reaches the selected (older) commit, not yet the end.
    let generation = app.git_history.generation;
    let chunk = log::Chunk {
        commits: vec![old[0].clone(), old[1].clone()],
        done: false,
    };
    app.test_git_history_chunk(path, generation, Ok(chunk));
    assert_eq!(app.git_history.commits.len(), 3, "swapped in before the walk ended");
    assert_eq!(app.git_history.commits[0].summary, "outside jed");
    assert!(app.git_history.more && !app.git_history.fresh);
    assert!(app.git_history.staged.is_empty());
    assert_eq!(app.git_history.selected(), Some(old[1].id.as_str()), "selection kept");
    assert_eq!(app.git_history.scroll(), 0, "the new top commit shows");
    probe.hold(false);
}
