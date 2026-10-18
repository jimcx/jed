//! Git history (git_history-b): the Explorer view's "Git History" section — the active file's
//! commits newest first, the list following the active file, paging as it scrolls, "No history"
//! when there is none, the collapse and the draggable divider, the mouse and the section's
//! keys, and a commit made outside jed appearing at the top. (Commit tabs, stat rows and
//! sessions are git_history-c.)

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::{Command, ExplorerCommand, GitHistoryCommand as G};
use crate::diff_view::Pair;
use crate::git::log::{Commit, Page};
use crate::git::test_repo::{commit_all, git, init};
use crate::history;
use crate::layout::SidePanel;
use crate::milestone10_tests::{dirs, open_file_app};

const SIDE_WIDTH: u16 = 44;

fn write(root: &Path, name: &str, text: &str) {
    fs::write(root.join(name), text).unwrap();
}

/// The path's commits as `git log` lists them.
fn cli_log(root: &Path, path: &str) -> Vec<String> {
    git(root, &["log", "--format=%H", "--", path])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Waits until `path`'s history is loaded with `len` commits (and git isn't still reading).
async fn settle_history(app: &mut App, path: &Path, len: usize) -> bool {
    let path = path.to_path_buf();
    app.settle(move |a| {
        !a.git.running()
            && a.git_history.path.as_deref() == Some(path.as_path())
            && !a.git_history.loading
            && a.git_history.commits.len() == len
    })
    .await
}

fn draw(app: &mut App) -> Terminal<TestBackend> {
    app.ui.side_width = SIDE_WIDTH;
    draw_sized(app, 120, 24)
}

fn draw_sized(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

fn list_row(term: &Terminal<TestBackend>, list: Rect, i: u16) -> String {
    let buf = term.backend().buffer();
    (list.x..list.x + list.width)
        .map(|x| buf[(x, list.y + i)].symbol())
        .collect()
}

fn summaries(app: &App) -> Vec<String> {
    app.git_history.rows().iter().map(|c| c.summary.clone()).collect()
}

/// 205 real commits on `a.txt`, made with `git2` (one CLI call per commit would be slow); each
/// changes the file so the history walk shows it.
fn fast_commits(root: &Path, n: usize) {
    let repo = git2::Repository::open(root).unwrap();
    let signature =
        |i: usize| git2::Signature::new("t", "t@example.com", &git2::Time::new(1_700_000_000 + i as i64, 0)).unwrap();
    // On top of what HEAD is (the commits then belong to the same history), or a root.
    let mut parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).map(|c| c.id());
    for i in 0..n {
        write(root, "a.txt", &format!("{i}\n"));
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let parent_commit = parent.map(|id| repo.find_commit(id).unwrap());
        let parents: Vec<&git2::Commit> = parent_commit.iter().collect();
        let id = repo
            .commit(
                Some("HEAD"),
                &signature(i),
                &signature(i),
                &format!("commit {i}"),
                &tree,
                &parents,
            )
            .unwrap();
        parent = Some(id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_lists_the_active_files_commits_newest_first() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "a.txt", "one\n");
    write(&root, "b.txt", "b\n");
    commit_all(&root, "first commits both");
    write(&root, "a.txt", "one\ntwo\n");
    commit_all(&root, "second changes a");
    write(&root, "b.txt", "b\nb2\n");
    commit_all(&root, "third changes b");
    write(&root, "a.txt", "one\ntwo\nthree\n");
    commit_all(&root, "fourth changes a");

    let mut app = open_file_app(&root, &state, "a.txt");
    let path = root.join("a.txt");
    assert!(settle_history(&mut app, &path, 3).await, "{:?}", app.git_history);

    let expected = cli_log(&root, "a.txt");
    assert_eq!(expected.len(), 3, "a's commits only");
    assert_eq!(
        app.git_history.rows().iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        expected,
        "newest first, as git lists them"
    );
    assert_eq!(
        summaries(&app),
        ["fourth changes a", "second changes a", "first commits both"]
    );
    assert!(!summaries(&app).iter().any(|s| s == "third changes b"));

    // Drawn: summary, author, relative date and short id, newest first.
    let term = draw(&mut app);
    let hit = app.hits.history.expect("the section is drawn below the tree");
    assert_eq!(hit.top, 0);
    assert!(hit.list.height >= 3);
    let first = list_row(&term, hit.list, 0);
    let second = list_row(&term, hit.list, 1);
    let third = list_row(&term, hit.list, 2);
    assert!(first.contains("fourth changes a"), "{first:?}");
    assert!(first.contains("t,"), "the author: {first:?}");
    assert!(
        first.contains(&app.git_history.rows()[0].short),
        "the short id: {first:?}"
    );
    assert!(second.contains("second changes a"), "{second:?}");
    assert!(third.contains("first commits both"), "{third:?}");
    assert!(list_row(&term, hit.list, 3).trim().is_empty(), "no fourth row");
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_follows_the_active_file() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "a.txt", "a1\n");
    write(&root, "b.txt", "b1\n");
    commit_all(&root, "adds both");
    write(&root, "a.txt", "a1\na2\n");
    commit_all(&root, "changes a");
    write(&root, "b.txt", "b1\nb2\n");
    commit_all(&root, "changes b");

    let (a, b) = (root.join("a.txt"), root.join("b.txt"));
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &a, 2).await);
    assert_eq!(summaries(&app), ["changes a", "adds both"]);

    // A second tab: the list follows it.
    app.test_open(Buffer::open(&b).unwrap());
    assert!(settle_history(&mut app, &b, 2).await);
    assert_eq!(summaries(&app), ["changes b", "adds both"]);

    // Back to the first tab (Ctrl+PageUp, the previous tab).
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(settle_history(&mut app, &a, 2).await);
    assert_eq!(summaries(&app), ["changes a", "adds both"]);
    assert_eq!(app.git_history.rows()[0].id, cli_log(&root, "a.txt")[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_says_no_history_when_there_is_none() {
    // A file outside any repository.
    let (_d, root, state) = dirs();
    write(&root, "a.txt", "a\n");
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(
        settle_history(&mut app, &root.join("a.txt"), 0).await,
        "not even a repository: nothing to read"
    );
    assert!(app.git_history.error.is_none());
    let term = draw(&mut app);
    let hit = app.hits.history.expect("the section still shows");
    assert!(
        list_row(&term, hit.list, 0).contains("No history"),
        "{:?}",
        list_row(&term, hit.list, 0)
    );

    // An untracked file in a repository.
    let (_d2, repo, state) = dirs();
    init(&repo);
    write(&repo, "tracked.txt", "t\n");
    commit_all(&repo, "one");
    write(&repo, "untracked.txt", "u\n");
    let mut app = open_file_app(&repo, &state, "untracked.txt");
    assert!(settle_history(&mut app, &repo.join("untracked.txt"), 0).await);
    assert!(app.git_history.error.is_none(), "{:?}", app.git_history.error);
    let term = draw(&mut app);
    assert!(list_row(&term, app.hits.history.unwrap().list, 0).contains("No history"));

    // A repository with no commits yet (unborn HEAD).
    let (_d3, empty, state) = dirs();
    init(&empty);
    write(&empty, "a.txt", "a\n");
    let mut app = open_file_app(&empty, &state, "a.txt");
    assert!(settle_history(&mut app, &empty.join("a.txt"), 0).await);
    assert!(app.git_history.error.is_none(), "{:?}", app.git_history.error);
    let term = draw(&mut app);
    assert!(list_row(&term, app.hits.history.unwrap().list, 0).contains("No history"));
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_collapses_and_drags_its_divider() {
    let (_d, root, state) = dirs();
    init(&root);
    for (i, text) in ["one", "two", "three"].iter().enumerate() {
        write(&root, "a.txt", &format!("{text}\n"));
        commit_all(&root, &format!("commit {i}"));
    }
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 3).await);

    draw(&mut app);
    let tree_before = app.hits.explorer_rows.height;
    let hit = app.hits.history.unwrap();
    assert!(hit.list.height > 0);
    let (x, header_y) = (hit.header.x + 3, hit.header.y);

    // The header collapses the section: its rows go, the tree grows.
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, header_y);
    assert!(app.git_history.collapsed());
    assert!(app.git_history.focused(), "the section got the keys");
    let _term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    assert_eq!(hit.list, Rect::default(), "no rows drawn");
    assert!(app.hits.explorer_rows.height > tree_before, "the tree grew");

    // Clicking it again opens the section.
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, hit.header.y);
    assert!(!app.git_history.collapsed());
    let _term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    assert_eq!(hit.list.height, 8, "the default section height less divider + header");
    assert_eq!(app.hits.explorer_rows.height, tree_before);

    // Dragging the divider up grows the section by the rows dragged.
    let before = app.git_history.section_height;
    let divider = hit.divider;
    app.mouse(MouseEventKind::Down(MouseButton::Left), divider.x + 2, divider.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), divider.x + 2, divider.y - 3);
    app.mouse(MouseEventKind::Up(MouseButton::Left), divider.x + 2, divider.y - 3);
    assert_eq!(app.git_history.section_height, before + 3);

    // Dragged past the bottom: the section keeps its divider and header.
    let side = app.hits.regions.side.unwrap();
    let bottom = side.y + side.height;
    let _term = draw(&mut app);
    let divider = app.hits.history.unwrap().divider;
    app.mouse(MouseEventKind::Down(MouseButton::Left), divider.x + 2, divider.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), divider.x + 2, bottom + 5);
    app.mouse(MouseEventKind::Up(MouseButton::Left), divider.x + 2, bottom + 5);
    assert_eq!(app.git_history.section_height, 2);

    // Dragged past the top: the tree keeps its three rows.
    let _term = draw(&mut app);
    let divider = app.hits.history.unwrap().divider;
    app.mouse(MouseEventKind::Down(MouseButton::Left), divider.x + 2, divider.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), divider.x + 2, side.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), divider.x + 2, side.y);
    let body = bottom - (side.y + 2);
    assert_eq!(app.git_history.section_height, body - 3);
    let _term = draw(&mut app);
    assert!(app.hits.explorer_rows.height >= 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_selects_with_the_keyboard() {
    let (_d, root, state) = dirs();
    init(&root);
    for (i, text) in ["one", "two", "three", "four"].iter().enumerate() {
        write(&root, "a.txt", &format!("{text}\n"));
        commit_all(&root, &format!("commit {i}"));
    }
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 4).await);
    let ids: Vec<String> = app.git_history.rows().iter().map(|c| c.id.clone()).collect();

    draw(&mut app);
    let hit = app.hits.history.unwrap();
    // A click on a commit row selects it and gives the section the keys.
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 2, hit.list.y + 1);
    assert!(app.git_history.focused());
    assert_eq!(app.ui.focus, crate::app::Focus::SidePanel);
    assert_eq!(app.ui.side_panel, SidePanel::Explorer);
    assert_eq!(app.git_history.selected(), Some(ids[1].as_str()));

    app.press(KeyCode::Down, M::NONE);
    assert_eq!(app.last_command, Some(Command::GitHistory(G::Down)));
    assert_eq!(app.git_history.selected(), Some(ids[2].as_str()));
    app.press(KeyCode::Down, M::NONE);
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(
        app.git_history.selected(),
        Some(ids[3].as_str()),
        "Down at the end stays"
    );
    app.press(KeyCode::Up, M::NONE);
    assert_eq!(app.git_history.selected(), Some(ids[2].as_str()));
    app.press(KeyCode::Home, M::NONE);
    assert_eq!(app.git_history.selected(), Some(ids[0].as_str()));
    app.press(KeyCode::Up, M::NONE);
    assert_eq!(app.git_history.selected(), Some(ids[0].as_str()), "Up at the top stays");
    app.press(KeyCode::End, M::NONE);
    assert_eq!(app.git_history.selected(), Some(ids[3].as_str()));

    // Left goes back to the tree: a key now goes to the explorer, the selection stays.
    app.press(KeyCode::Left, M::NONE);
    assert_eq!(app.last_command, Some(Command::GitHistory(G::FocusTree)));
    assert!(!app.git_history.focused());
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(app.last_command, Some(Command::Explorer(ExplorerCommand::Down)));
    assert_eq!(app.git_history.selected(), Some(ids[3].as_str()));
    // Esc does the same from the list.
    app.test_execute(Command::GitHistory(G::FocusTree));
    let hit = app.hits.history.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 2, hit.list.y);
    assert!(app.git_history.focused());
    app.press(KeyCode::Esc, M::NONE);
    assert!(!app.git_history.focused());
    assert_eq!(app.last_command, Some(Command::GitHistory(G::FocusTree)));

    // Focusing the Explorer (Ctrl+Shift+E) likewise gives the tree the keys.
    let hit = app.hits.history.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 2, hit.list.y);
    assert!(app.git_history.focused());
    app.test_execute(Command::FocusExplorer);
    assert!(!app.git_history.focused());
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(app.last_command, Some(Command::Explorer(ExplorerCommand::Down)));
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_pages_load_as_it_scrolls() {
    let (_d, root, state) = dirs();
    init(&root);
    fast_commits(&root, 205);
    let expected = cli_log(&root, "a.txt");
    assert_eq!(expected.len(), 205);

    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(
        app.settle(|a| !a.git_history.loading && a.git_history.commits.len() == 200 && a.git_history.more)
            .await,
        "the first page is 200 commits and promises more: {:?}",
        app.git_history
    );

    // Put the end of the list on screen: the next page follows.
    draw(&mut app);
    let hit = app.hits.history.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 2, hit.list.y);
    app.press(KeyCode::End, M::NONE);
    assert_eq!(app.last_command, Some(Command::GitHistory(G::Last)));
    assert_eq!(app.git_history.scroll(), 192, "End revealed the last of 200");
    assert!(
        app.settle(|a| !a.git_history.loading && a.git_history.commits.len() == 205 && !a.git_history.more)
            .await,
        "the second page: {:?}",
        app.git_history
    );
    let ids: Vec<String> = app.git_history.rows().iter().map(|c| c.id.clone()).collect();
    assert_eq!(ids, expected, "pages are the same list split, in order");
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 205, "no duplicates");
    // The drawn list still shows the tail End went to (the page's append doesn't move it).
    let term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    assert_eq!(hit.top, 200 - hit.list.height as usize, "End went to the tail");
    // Row 192 of the list is `commit 12` (newest first: `commit 204` is row 0).
    assert!(
        list_row(&term, hit.list, 0).contains("commit 12"),
        "{:?}",
        list_row(&term, hit.list, 0)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_new_commit_appears_at_the_top() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "a.txt", "one\n");
    commit_all(&root, "first");
    write(&root, "a.txt", "one\ntwo\n");
    commit_all(&root, "second");
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    assert_eq!(summaries(&app), ["second", "first"]);

    // A commit made outside jed: the watcher sees the file and the repository, the status read
    // sees the new HEAD, and the history reloads page 1.
    write(&root, "a.txt", "one\ntwo\nthree\n");
    commit_all(&root, "third");
    let expected = cli_log(&root, "a.txt");
    assert_eq!(expected.len(), 3);
    assert!(
        app.settle(|a| {
            !a.git_history.loading
                && a.git_history.commits.len() == 3
                && a.git_history.commits.first().is_some_and(|c| c.id == expected[0])
        })
        .await,
        "the new commit is at the top: {:?}",
        app.git_history
    );
    assert_eq!(summaries(&app), ["third", "second", "first"]);
    assert_eq!(app.git_history.selected(), Some(expected[0].as_str()));
    assert_eq!(app.git_history.scroll(), 0, "the list scrolled back to the top");

    let term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    assert!(list_row(&term, hit.list, 0).contains("third"));
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_follows_a_diff_tabs_file() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "a.txt", "a1\n");
    write(&root, "b.txt", "b1\n");
    commit_all(&root, "adds both");
    write(&root, "a.txt", "a1\na2\n");
    commit_all(&root, "changes a");
    write(&root, "b.txt", "b1\nb2\n");
    commit_all(&root, "changes b");

    let (a, b) = (root.join("a.txt"), root.join("b.txt"));
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &a, 2).await);
    app.test_open(Buffer::open(&b).unwrap());
    assert!(settle_history(&mut app, &b, 2).await);
    assert_eq!(summaries(&app), ["changes b", "adds both"]);

    // A diff tab counts as its file: show a.txt's unstaged change while b.txt is the file of
    // the text tab that was active.
    write(&root, "a.txt", "a1\na2\na3\n");
    app.open_diff(a.clone(), Pair::UNSTAGED);
    assert!(app.settle(|a| a.diffs_settled()).await, "the diff loaded");
    assert!(settle_history(&mut app, &a, 2).await, "{:?}", app.git_history);
    assert_eq!(summaries(&app), ["changes a", "adds both"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_wheel_and_scroll_bar_scroll_only_its_list() {
    let (_d, root, state) = dirs();
    init(&root);
    for i in 0..40 {
        write(&root, &format!("f{i}.txt"), "x\n");
    }
    write(&root, "a.txt", "a\n");
    commit_all(&root, "many files");
    fast_commits(&root, 30);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 31).await);
    assert!(app.settle(|a| a.explorer.row_count() >= 41).await, "the tree listed");

    let _term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    assert_eq!((app.explorer.scroll, app.git_history.scroll()), (0, 0));
    // The wheel over a commit row scrolls the history, not the tree.
    app.mouse(MouseEventKind::ScrollDown, hit.list.x + 2, hit.list.y + 1);
    let step = app.git_history.scroll();
    assert!(step > 0, "the history moved");
    assert_eq!(app.explorer.scroll, 0, "the tree didn't move");
    // The wheel over a tree row scrolls the tree, not the history.
    let tree = app.hits.explorer_rows;
    app.mouse(MouseEventKind::ScrollDown, tree.x + 2, tree.y + 1);
    assert_eq!(app.explorer.scroll, step, "the tree moved by a wheel step");
    assert_eq!(app.git_history.scroll(), step, "the history didn't move");
    // The wheel over the history's own scroll bar scrolls the history.
    let bar = app.hits.history_scrollbar.expect("the history is scrollable");
    app.mouse(MouseEventKind::ScrollDown, bar.track.x, bar.track.y);
    assert_eq!(app.git_history.scroll(), step * 2);

    // A click at the bottom of its bar scrolls the history (not the tree) to the end.
    let _term = draw(&mut app);
    let bar = app.hits.history_scrollbar.unwrap();
    assert!(bar.thumb.is_some());
    app.mouse(
        MouseEventKind::Down(MouseButton::Left),
        bar.track.x,
        bar.track.y + bar.track.height - 1,
    );
    app.mouse(
        MouseEventKind::Up(MouseButton::Left),
        bar.track.x,
        bar.track.y + bar.track.height - 1,
    );
    assert_eq!(app.git_history.scroll(), 31 - hit.list.height as usize);
    assert_eq!(app.explorer.scroll, step, "the tree's list didn't move");
    let term = draw(&mut app);
    assert!(
        list_row(&term, hit.list, 0).contains("commit 6"),
        "the first row on screen"
    );
    // A click adds the drawn top: row 1 on screen is list row top + 1.
    let top = app.git_history.scroll();
    let target = app.git_history.rows()[top + 1].id.clone();
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 2, hit.list.y + 1);
    assert_eq!(app.git_history.selected(), Some(target.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_head_change_without_new_commits_keeps_its_place() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "a.txt", "a\n");
    write(&root, "b.txt", "b\n");
    commit_all(&root, "both");
    fast_commits(&root, 20);
    let a = root.join("a.txt");
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &a, 21).await);

    let term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 2, hit.list.y);
    app.press(KeyCode::End, M::NONE);
    app.press(KeyCode::Up, M::NONE);
    let (selected, top, first) = (
        app.git_history.selected().map(str::to_string),
        app.git_history.scroll(),
        app.git_history.rows()[0].id.clone(),
    );
    assert!(top > 0, "scrolled down to pick a middle commit");
    drop(term);

    // A commit touching only another file: HEAD moves, this file's history doesn't.
    write(&root, "b.txt", "b\nb2\n");
    commit_all(&root, "b only");
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    assert!(
        app.settle(|a| {
            !a.git.running()
                && a.git
                    .status
                    .as_ref()
                    .is_some_and(|s| s.head_id.as_deref() == Some(head.as_str()))
                && !a.git_history.loading
                && !a.git_history.stale
        })
        .await,
        "the history reloaded: {:?}",
        app.git_history
    );
    assert_eq!(app.git_history.rows().len(), 21, "the same commits");
    assert_eq!(app.git_history.rows()[0].id, first);
    assert_eq!(
        app.git_history.selected().map(str::to_string),
        selected,
        "the selection stayed"
    );
    assert_eq!(app.git_history.scroll(), top, "the scroll stayed");
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_drops_pages_for_another_file_or_an_older_list() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "a.txt", "a1\n");
    write(&root, "b.txt", "b1\n");
    commit_all(&root, "both");
    write(&root, "a.txt", "a1\na2\n");
    commit_all(&root, "a");
    write(&root, "b.txt", "b1\nb2\n");
    commit_all(&root, "b");
    let (a, b) = (root.join("a.txt"), root.join("b.txt"));
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &a, 2).await);
    app.test_open(Buffer::open(&b).unwrap());
    assert!(settle_history(&mut app, &b, 2).await);
    let generation = app.git_history.generation;
    let before = summaries(&app);
    let late = |summary: &str| Page {
        commits: vec![Commit {
            id: "f".repeat(40),
            short: "fffffff".into(),
            summary: summary.into(),
            author: "x".into(),
            time: 0,
        }],
        more: true,
    };

    // The old file's page, at this list's generation: dropped.
    app.test_git_history_page(a.clone(), 0, generation, Ok(late("other file")));
    assert_eq!(summaries(&app), before);
    // A page started before the file changed (an older generation): dropped.
    app.test_git_history_page(b.clone(), 0, generation - 1, Ok(late("older list")));
    assert_eq!(summaries(&app), before);
    // A later page that doesn't follow the loaded list: dropped (no duplicates).
    app.test_git_history_page(b.clone(), 1, generation, Ok(late("not the next page")));
    assert_eq!(summaries(&app), before);
    // The page after the loaded ones: appended.
    app.test_git_history_page(
        b.clone(),
        2,
        generation,
        Ok(Page {
            more: false,
            ..late("the next page")
        }),
    );
    assert_eq!(summaries(&app), ["b", "both", "the next page"]);
    assert!(!app.git_history.more, "the empty page ends the list");
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_enter_does_nothing_yet() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "a.txt", "one\n");
    commit_all(&root, "one");
    write(&root, "a.txt", "one\ntwo\n");
    commit_all(&root, "two");
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 2, hit.list.y);
    let (tabs, active, text) = (
        app.group().tabs.len(),
        app.group().active,
        app.active_buffer().unwrap().rope.to_string(),
    );

    // No commit tab yet (git_history-c): Enter is unbound, a plain char does nothing either.
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Char('x'), M::NONE);
    assert_eq!(app.group().tabs.len(), tabs, "no tab opened");
    assert_eq!(app.group().active, active);
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), text, "no text typed");
    assert!(app.git_history.focused(), "the section kept the keys");
    assert_eq!(
        app.git_history.selected(),
        app.git_history.rows().first().map(|c| c.id.as_str())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_uses_theme_colours_and_fits_small_panels() {
    let (_d, root, state) = dirs();
    init(&root);
    fast_commits(&root, 3);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 3).await);
    let term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    let commit = app.git_history.rows()[0].clone();
    let row = list_row(&term, hit.list, 0);
    let buf = term.backend().buffer();
    let fg = |text: &str| {
        let at = row.find(text).unwrap_or_else(|| panic!("{text:?} not in {row:?}"));
        buf[(hit.list.x + at as u16, hit.list.y)].style().fg
    };
    // The commit's own fields as the section's theme keys.
    assert_eq!(fg("t,"), app.theme.get("gitHistory.author").fg);
    let ago = history::ago((commit.time.max(0) as u64) * 1000, history::now_ms());
    assert_eq!(fg(&ago), app.theme.get("gitHistory.date").fg);
    assert_eq!(fg(&commit.short), app.theme.get("gitHistory.hash").fg);

    // A narrow panel: the summary gives way to the short id; the tree keeps three rows.
    app.ui.side_width = 20;
    let term = draw_sized(&mut app, 70, 12);
    let hit = app.hits.history.unwrap();
    assert_eq!(hit.list.width, 18, "the body less the border and the bar");
    assert_eq!(hit.list.height, 4);
    assert_eq!(app.hits.explorer_rows.height, 3);
    assert!(list_row(&term, hit.header, 0).contains("GIT HISTORY"));
    assert!(
        list_row(&term, hit.list, 0).contains("commit 2"),
        "{:?}",
        list_row(&term, hit.list, 0)
    );

    // The smallest side panel: the summary only (no room for the suffix), still drawn.
    app.ui.side_width = 12;
    let term = draw_sized(&mut app, 60, 24);
    let hit = app.hits.history.unwrap();
    assert_eq!(hit.list.width, 10, "the body less the border and the bar");
    assert!(!list_row(&term, hit.list, 0).trim().is_empty(), "a commit row");

    // Too short for both: the tree takes the body.
    let _term = draw_sized(&mut app, 70, 6);
    assert!(app.hits.history.is_none(), "no room for the section");
    assert!(app.hits.explorer_rows.height >= 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_shows_loading_and_errors() {
    let (_d, root, state) = dirs();
    write(&root, "a.txt", "a\n");
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 0).await);

    app.git_history.loading = true;
    let term = draw(&mut app);
    let hit = app.hits.history.unwrap();
    assert!(list_row(&term, hit.list, 0).contains("Reading history…"));

    app.git_history.loading = false;
    app.git_history.error = Some("boom".into());
    let term = draw(&mut app);
    assert!(list_row(&term, hit.list, 0).contains("boom"));
}
