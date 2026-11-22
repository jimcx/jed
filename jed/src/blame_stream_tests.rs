//! Streamed inline blame, "Done when": a repository built with `git2` whose last commit changes
//! one line of a file (50 commits by default; the Done when's 5,000 in the `#[ignore]`d
//! `blame_stream_first_chunk_with_5000_commits`) (`blame_stream_first_chunk_*`: the walk held back after its
//! first chunk by `App::blame_probe`, so the changed line's label shows while the others have
//! none until the rest arrives); edits during the walk (`blame_stream_edits_*`: "Uncommitted
//! changes" within `blame::DELAY`, one walk, chunks landing on the lines mapped through the
//! edits); the cancel flag when HEAD moves, the blame is turned off or the buffer closes
//! (`blame_stream_*_cancels_*`); a finished `Base` reused (`blame_stream_reopening_*`); and the
//! release budget test `milestone_blame_stream_budget` (20,000 commits, a file of a few thousand
//! lines touched by about 1,000 of them), reporting the time to the first chunk and the whole file,
//! before (libgit2's `blame_file`, timed here) and after (`git::blame::Walk`). Asserted in release
//! only: the first chunk in under 100 ms, the whole file at least 5 times faster than before. The
//! walk's line-for-line comparison with `git blame` is in `git::blame`'s tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use git2::{Oid, Repository, Signature, Time};

use crate::app::App;
use crate::blame::{Line, Want};
use crate::command::Command;
use crate::git::Repo;
use crate::git::blame::Walk;
use crate::git::test_repo::commit_all;
use crate::milestone10_tests::{dirs, open_file_app, settle_git};

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
        let sig = Signature::new(
            "Alice",
            "alice@example.com",
            &Time::new(1_600_000_000 + 60 * i as i64, 0),
        )
        .unwrap();
        let parents: Vec<git2::Commit> = parent.iter().map(|p| repo.find_commit(*p).unwrap()).collect();
        let parents: Vec<&git2::Commit> = parents.iter().collect();
        parent = Some(
            repo.commit(None, &sig, &sig, &format!("c{i}"), &tree, &parents)
                .unwrap(),
        );
    }
    repo.reference("refs/heads/main", parent.unwrap(), true, "budget")
        .unwrap();
    repo.set_head("refs/heads/main").unwrap();
    println!("built {commits} commits, {touched} touching target.txt");
    text.len()
}

#[test]
fn milestone_blame_stream_budget() {
    let (commits, every, lines) = if cfg!(debug_assertions) {
        (200, 20, 400)
    } else {
        (20_000, 20, 3_000)
    };
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let total = build(&root, commits, every, lines);
    let repo = Repo::discover(&root).unwrap();
    let path = Path::new("target.txt");

    // Before: libgit2's blame of the whole file (what inline blame called until the streamed
    // walk replaced it): nothing is known until it ends.
    let t = Instant::now();
    let r = repo.open().unwrap();
    let mut opts = git2::BlameOptions::new();
    opts.newest_commit(r.head().unwrap().peel_to_commit().unwrap().id());
    r.blame_file(path, Some(&mut opts)).unwrap();
    let before = t.elapsed();

    // After: the streaming driver the app's job runs (default slice, chunks every `FLUSH`).
    let t = Instant::now();
    let walk = Walk::new(&repo, path).unwrap().unwrap();
    let (mut first, mut settled, mut chunks) = (None, 0, 0);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    crate::git::blame::stream(walk, &cancel, |chunk| {
        let chunk = chunk.unwrap();
        settled += chunk.settled.iter().map(|s| s.count).sum::<usize>();
        chunks += 1;
        first.get_or_insert_with(|| t.elapsed());
        true
    });
    let whole = t.elapsed();
    let first = first.unwrap();
    assert_eq!(settled, total, "every line settled once");

    println!("before (libgit2 blame_file): whole file in {before:?}");
    println!("after (stream): first chunk in {first:?}, whole file in {whole:?}, {chunks} chunks");
    if !cfg!(debug_assertions) {
        assert!(first < Duration::from_millis(100), "first chunk too slow: {first:?}");
        assert!(whole * 5 <= before, "whole file not 5x faster: {whole:?} vs {before:?}");
    }
}

/// The default tests' history: the probe holds the walk after each chunk, so a short history is
/// still mid-walk when they look. `FULL_COMMITS` is the Done when's size, run by the `#[ignore]`d
/// `blame_stream_first_chunk_with_5000_commits` (about 6 s alone in debug, most of it building
/// the repository).
const COMMITS: usize = 50;
const FULL_COMMITS: usize = 5_000;
const LINES: usize = 12;
const CHANGED: usize = 5;

fn target_text(last: bool) -> String {
    (0..LINES)
        .map(|i| match last && i == CHANGED {
            true => "changed line\n".to_string(),
            false => format!("line {i}\n"),
        })
        .collect()
}

/// The last commit's summary in a history of `commits`.
fn last(commits: usize) -> String {
    format!("c{}", commits - 1)
}

/// `commits` commits (`c0` .. `c{commits - 1}`) built with `git2`, each changing `other.txt`; the
/// first writes `target.txt`, the last changes its line `CHANGED`. The work tree matches HEAD.
fn many_commits(root: &Path, commits: usize) {
    let repo = Repository::init(root).unwrap();
    let (old, new) = (
        repo.blob(target_text(false).as_bytes()).unwrap(),
        repo.blob(target_text(true).as_bytes()).unwrap(),
    );
    let mut parent: Option<Oid> = None;
    for i in 0..commits {
        let mut tree = repo.treebuilder(None).unwrap();
        tree.insert("target.txt", if i == commits - 1 { new } else { old }, 0o100644)
            .unwrap();
        tree.insert("other.txt", repo.blob(format!("{i}\n").as_bytes()).unwrap(), 0o100644)
            .unwrap();
        let tree = repo.find_tree(tree.write().unwrap()).unwrap();
        let sig = Signature::new(
            "Alice",
            "alice@example.com",
            &Time::new(1_600_000_000 + 60 * i as i64, 0),
        )
        .unwrap();
        let parents: Vec<git2::Commit> = parent.iter().map(|p| repo.find_commit(*p).unwrap()).collect();
        let parents: Vec<&git2::Commit> = parents.iter().collect();
        parent = Some(
            repo.commit(None, &sig, &sig, &format!("c{i}"), &tree, &parents)
                .unwrap(),
        );
    }
    repo.reference("refs/heads/main", parent.unwrap(), true, "fixture")
        .unwrap();
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
}

/// Releases the probe when the test unwinds, so a failure isn't a hang (a held job never wakes).
struct Release(std::sync::Arc<crate::app::StreamProbe>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.hold(false);
    }
}

/// The app on a `COMMITS`-commit repository with the walk's slice cut to one commit and the probe
/// holding the walk after each chunk it sends.
async fn walking_app() -> (tempfile::TempDir, PathBuf, App, Release) {
    walking_app_with(COMMITS).await
}

async fn walking_app_with(commits: usize) -> (tempfile::TempDir, PathBuf, App, Release) {
    let (dir, root, state) = dirs();
    many_commits(&root, commits);
    let mut app = open_file_app(&root, &state, "target.txt");
    assert!(settle_git(&mut app, |_| true).await);
    app.blame.slice = Some(Duration::ZERO);
    let probe = app.blame_probe.clone();
    probe.hold(true);
    (dir, root, app, Release(probe))
}

/// Blame on, and the first chunk (the changed line's commit) applied.
async fn first_chunk(app: &mut App) {
    app.test_execute(Command::ToggleBlame);
    assert!(
        app.settle(|a| {
            let b = a.active_buffer().unwrap();
            a.blame_probe.sent.load(Ordering::SeqCst) == 1
                && b.blame.settled() > 0
                && !b.blame.running()
                && b.blame.want(b.revision()) == Want::Nothing
        })
        .await
    );
}

/// Waits until the walk is over and the hunks are computed from all of it.
async fn finished(app: &mut App) {
    assert!(
        app.settle(|a| {
            let b = a.active_buffer().unwrap();
            !b.blame.walking() && !b.blame.running() && b.blame.want(b.revision()) == Want::Nothing
        })
        .await
    );
}

fn at_line(app: &mut App, line: usize) {
    app.test_execute(Command::Move {
        motion: crate::command::Motion::DocStart,
        select: false,
    });
    for _ in 0..line {
        app.press(crossterm::event::KeyCode::Down, crossterm::event::KeyModifiers::NONE);
    }
}

/// Buffer line `line` as drawn (the view isn't scrolled), trailing blanks dropped.
fn row(app: &mut App, line: u16) -> String {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let text = app.hits.groups[0].text;
    let buf = term.backend().buffer();
    let s: String = (0..text.width)
        .map(|x| buf[(text.x + x, text.y + line)].symbol())
        .collect();
    s.trim_end().to_string()
}

fn line_of(app: &App, line: usize) -> Option<String> {
    let b = app.active_buffer().unwrap();
    Some(match b.blame.line(&b.rope, line)? {
        Line::Committed(c) => c.summary.clone(),
        Line::Uncommitted => "-".into(),
        Line::Pending => "?".into(),
    })
}

/// The changed line's label shows from the first chunk while the walk runs; the others show none
/// until their chunk arrives, then their commit.
#[tokio::test(flavor = "multi_thread")]
async fn blame_stream_first_chunk_shows_the_changed_line_while_the_walk_runs() {
    first_chunk_while_walking(COMMITS).await;
}

/// The same at the Done when's 5,000 commits: run with `--ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: builds a 5,000-commit repository; the default test runs the same on 50"]
async fn blame_stream_first_chunk_with_5000_commits() {
    first_chunk_while_walking(FULL_COMMITS).await;
}

async fn first_chunk_while_walking(commits: usize) {
    let (_d, _root, mut app, _release) = walking_app_with(commits).await;
    first_chunk(&mut app).await;
    assert!(app.active_buffer().unwrap().blame.walking(), "the walk is held back");
    assert_eq!(app.active_buffer().unwrap().blame.settled(), 1);

    at_line(&mut app, CHANGED);
    let shown = row(&mut app, CHANGED as u16);
    assert!(shown.starts_with("changed line   "), "{shown}");
    let label = format!(" • {}", last(commits));
    assert!(shown.contains(" Alice, ") && shown.ends_with(&label), "{shown}");

    // An old line: its commit isn't known yet.
    at_line(&mut app, 2);
    assert_eq!(line_of(&app, 2).as_deref(), Some("?"));
    assert_eq!(row(&mut app, 2), "line 2");

    // The rest arrives: shown without moving the cursor.
    app.blame_probe.hold(false);
    finished(&mut app).await;
    let shown = row(&mut app, 2);
    assert!(shown.starts_with("line 2   ") && shown.ends_with(" • c0"), "{shown}");
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 1);
}

/// An edit during the walk is "Uncommitted changes" within the delay and doesn't restart the
/// walk; the chunks that come later land on the right lines.
#[tokio::test(flavor = "multi_thread")]
async fn blame_stream_edits_during_the_walk_neither_restart_it_nor_lose_chunks() {
    let (_d, _root, mut app, _release) = walking_app().await;
    first_chunk(&mut app).await;

    // A line added at the top shifts the others down.
    at_line(&mut app, 0);
    app.press(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
    assert!(app.settle(|a| line_of(a, 0).as_deref() == Some("-")).await);
    assert!(app.active_buffer().unwrap().blame.walking());
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 1, "one walk");
    at_line(&mut app, 0);
    assert_eq!(row(&mut app, 0), "   Uncommitted changes");

    app.blame_probe.hold(false);
    finished(&mut app).await;
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 1, "still one walk");
    let lines: Vec<_> = (0..=LINES).map(|l| line_of(&app, l).unwrap()).collect();
    let expected: Vec<_> = std::iter::once("-".to_string())
        .chain((0..LINES).map(|i| if i == CHANGED { last(COMMITS) } else { "c0".to_string() }))
        .collect();
    assert_eq!(lines, expected);
}

/// Releases the held walk and waits for its job to exit; returns what it had sent by then.
async fn exits(app: &mut App) -> usize {
    app.blame_probe.hold(false);
    assert!(
        app.settle(|a| a.blame_probe.exited.load(Ordering::SeqCst) == a.blame_probe.walks.load(Ordering::SeqCst))
            .await
    );
    app.blame_probe.sent.load(Ordering::SeqCst)
}

/// Turning the blame off sets the flag: the job exits without another chunk.
#[tokio::test(flavor = "multi_thread")]
async fn blame_stream_turning_off_cancels_the_walk() {
    let (_d, _root, mut app, _release) = walking_app().await;
    first_chunk(&mut app).await;
    let flag = app.active_buffer().unwrap().blame.cancel_flag().unwrap();
    assert!(!flag.load(Ordering::SeqCst));
    app.test_execute(Command::ToggleBlame);
    assert!(flag.load(Ordering::SeqCst));
    assert_eq!(exits(&mut app).await, 1, "nothing sent after the flag");
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 1);
}

/// Closing the buffer sets the flag too.
#[tokio::test(flavor = "multi_thread")]
async fn blame_stream_closing_the_buffer_cancels_the_walk() {
    let (_d, _root, mut app, _release) = walking_app().await;
    first_chunk(&mut app).await;
    let flag = app.active_buffer().unwrap().blame.cancel_flag().unwrap();
    app.test_execute(Command::CloseTab);
    assert!(app.active_buffer().is_none());
    assert!(flag.load(Ordering::SeqCst));
    assert_eq!(exits(&mut app).await, 1);
}

/// HEAD moving (a commit made outside jed) sets the old walk's flag and starts a new walk; the
/// old walk's chunks are dropped.
#[tokio::test(flavor = "multi_thread")]
async fn blame_stream_head_moving_cancels_the_walk_and_starts_another() {
    let (_d, root, mut app, _release) = walking_app().await;
    first_chunk(&mut app).await;
    let flag = app.active_buffer().unwrap().blame.cancel_flag().unwrap();
    std::fs::write(root.join("target.txt"), target_text(false).replace("line 1\n", "one\n")).unwrap();
    commit_all(&root, "outside jed");
    assert!(app.settle(|a| a.blame_probe.walks.load(Ordering::SeqCst) == 2).await);
    assert!(flag.load(Ordering::SeqCst), "the old walk is told to stop");
    assert!(app.settle(|a| a.blame_probe.sent.load(Ordering::SeqCst) == 2).await);
    exits(&mut app).await;
    app.blame.slice = None;
    finished(&mut app).await;
    // The old job sent its first chunk and nothing after the flag: every other chunk sent
    // (the probe is shared by both walks) is one the new walk's blame took.
    assert_eq!(
        app.blame_probe.sent.load(Ordering::SeqCst) as u64,
        1 + app.active_buffer().unwrap().blame.chunks_taken(),
        "the stopped walk sent nothing more"
    );
    // The new HEAD's blame, whole: line 1 is the new commit's.
    assert_eq!(line_of(&app, 1).as_deref(), Some("outside jed"));
    assert_eq!(line_of(&app, 2).as_deref(), Some("c0"));
    // The commit put the old text back on the changed line: its line is the new commit's too.
    assert_eq!(line_of(&app, CHANGED).as_deref(), Some("outside jed"));
}

/// After a finished walk, turning the blame off and on again reuses the base: no new walk.
#[tokio::test(flavor = "multi_thread")]
async fn blame_stream_reopening_reuses_the_finished_base() {
    let (_d, _root, mut app, _release) = walking_app().await;
    app.blame.slice = None;
    app.blame_probe.hold(false);
    app.test_execute(Command::ToggleBlame);
    finished(&mut app).await;
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 1);
    app.test_execute(Command::ToggleBlame);
    assert!(app.active_buffer().unwrap().blame.hunks().is_empty());
    app.test_execute(Command::ToggleBlame);
    finished(&mut app).await;
    assert_eq!(line_of(&app, 2).as_deref(), Some("c0"));
    assert_eq!(line_of(&app, CHANGED), Some(last(COMMITS)));
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 1, "no new walk");
}

/// A walk that failed leaves its base unfinished with no walk running: the next computation
/// (here after an edit) walks again, and every line ends settled.
#[tokio::test(flavor = "multi_thread")]
async fn blame_stream_a_failed_walk_is_walked_again() {
    let (_d, _root, mut app, _release) = walking_app().await;
    first_chunk(&mut app).await;
    let (b, _) = app.active_mut().unwrap();
    assert!(b.blame.chunk(1, Err("boom"), true), "the error chunk is the walk's");
    assert!(!b.blame.walking());
    assert!(b.blame.unfinished());
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 1);

    at_line(&mut app, 0);
    app.press(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
    assert!(app.settle(|a| a.blame_probe.walks.load(Ordering::SeqCst) == 2).await);
    app.blame_probe.hold(false);
    finished(&mut app).await;
    assert_eq!(app.blame_probe.walks.load(Ordering::SeqCst), 2, "walked once more");
    assert_eq!(app.active_buffer().unwrap().blame.settled(), LINES);
    assert_eq!(line_of(&app, 3).as_deref(), Some("c0"));
}
