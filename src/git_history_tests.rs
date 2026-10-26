//! Git history (git_history-b): the Explorer view's "Git History" section — the active file's
//! commits newest first, the list following the active file, paging as it scrolls, "No history"
//! when there is none, the collapse and the draggable divider, the mouse and the section's
//! keys, and a commit made outside jed appearing at the top. (git_history-c: Enter / a click
//! opens a commit tab, its `git show` text and colours, a stat row's diff in the group to the
//! right, selecting and copying, and sessions restoring the tabs and the section's layout.
//! git_history-d: `git_history_done_when` walks the milestone's Done-when against the `git`
//! CLI — the list, the commit tab's text and cells at the drawn width, the side-by-side diff,
//! a commit made outside jed, and the restored tabs and section.)

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crossterm::event::{Event, KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, Focus, InitialFile};
use crate::buffer::Buffer;
use crate::command::{Command, ExplorerCommand, GitHistoryCommand as G};
use crate::diff_view::{Pair, Text};
use crate::git::log::{Commit, Page};
use crate::git::test_repo::{commit_all, git, init};
use crate::group::Tab;
use crate::history;
use crate::layout::SidePanel;
use crate::milestone10_tests::{dirs, open_file_app};
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

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

/// Gives the section the keys without opening a commit (a click on a row opens one).
fn focus_history(app: &mut App) {
    app.ui.focus = Focus::SidePanel;
    app.ui.side_panel = SidePanel::Explorer;
    app.git_history.set_focused(true);
}

/// The app on `root` with sessions in `state` (as milestone 13's tests build one).
fn open_state_app(root: &Path, state: &Path, file: Option<&str>) -> App {
    open_state_app_inner(root, state, file, false)
}

/// `open_state_app`, with the file watcher on (for tests that change files on disk after start).
fn open_state_app_watched(root: &Path, state: &Path, file: Option<&str>) -> App {
    open_state_app_inner(root, state, file, true)
}

fn open_state_app_inner(root: &Path, state: &Path, file: Option<&str>, watch: bool) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let initial = file.map(|file| InitialFile {
        buffer: Buffer::open(&root.join(file)).unwrap(),
        goto: None,
    });
    let mut app = App::new(ws, trust, initial, true);
    app.state_dir = Some(state.to_path_buf());
    if watch {
        app.enable_file_watching();
    }
    app.test_start();
    app.test_plain_whitespace();
    app
}

/// The active commit tab's text, by line (rendered at the width the last frame gave it).
fn commit_lines(app: &App) -> Vec<String> {
    assert!(app.active_commit().is_some(), "a commit tab");
    let text = app.active_buffer().unwrap().rope.to_string();
    text.split('\n').map(str::to_string).collect()
}

/// The text width the last frame gave `group`'s editor (its gutter excluded).
fn text_width(app: &App, group: crate::layout::GroupId) -> usize {
    app.hits.group(group).expect("the group was drawn").text.width as usize
}

/// The active view's primary cursor line.
fn cursor_line(app: &App) -> usize {
    let tab = app.group().active_tab().unwrap();
    let rope = &app.buffers.get(tab.buffer).unwrap().rope;
    crate::buffer::pos::line_of(rope, tab.view.selections.primary().head)
}

fn doc_start(app: &mut App) {
    app.test_execute(Command::Move {
        motion: crate::command::Motion::DocStart,
        select: false,
    });
}

/// The commit a tab shows, if it's a commit tab.
fn tab_commit<'a>(app: &'a App, tab: &Tab) -> Option<&'a crate::commit_view::CommitDoc> {
    app.buffers.get(tab.buffer()?)?.commit.as_ref()
}

/// What `git show --stat --format=medium --stat-width=<width>` prints, without its last line break.
fn cli_show(root: &Path, id: &str, width: usize) -> String {
    let width = format!("--stat-width={width}");
    git(root, &["show", "--stat", "--format=medium", &width, id])
        .trim_end_matches('\n')
        .to_string()
}

/// Opens the selected history commit with the keyboard and waits for its `git show`.
async fn enter_commit(app: &mut App) -> String {
    focus_history(app);
    if app.git_history.selected().is_none() {
        app.press(KeyCode::Down, M::NONE);
    }
    let id = app.git_history.selected().unwrap_or_default().to_string();
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(app.last_command, Some(Command::GitHistory(G::Open)));
    assert!(
        app.settle(|a| { a.active_commit().is_some_and(|c| c.show.is_some() || c.error.is_some()) })
            .await
    );
    id
}

/// Clicks history row `row` in the drawn section (a click selects the commit and opens it).
fn click_history_row(app: &mut App, row: u16) {
    let hit = app.hits.history.expect("the section was drawn");
    let (x, y) = (hit.list.x + 2, hit.list.y + row);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// Clicks line `row` of the commit tab drawn in `group` (on a stat row: opens its diff).
fn click_commit_row(app: &mut App, group: crate::layout::GroupId, row: usize) {
    let hit = app.hits.group(group).expect("the group was drawn");
    let rows = hit.text;
    let top = app.groups.get(group).unwrap().active_tab().unwrap().view.top_line();
    let (x, y) = (rows.x + 2, rows.y + (row - top) as u16);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// A side-by-side diff's change rows: the left (removed) and right (added) line texts, in
/// order — what `git diff` shows as `-` / `+` lines.
fn diff_changes(doc: &crate::diff_view::Doc) -> (Vec<String>, Vec<String>) {
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    for row in &doc.aligned(false).rows {
        if row.hunk.is_some() {
            if let Some(l) = row.left {
                removed.push(doc.left.line(l).to_string());
            }
            if let Some(r) = row.right {
                added.push(doc.right.line(r).to_string());
            }
        }
    }
    (removed, added)
}

/// The `-` / `+` lines of `git diff <from> <to> -- <path>` (`---` / `+++` left out).
fn cli_diff(root: &Path, from: &str, to: &str, path: &str) -> (Vec<String>, Vec<String>) {
    let out = git(root, &["diff", "--no-color", from, to, "--", path]);
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    for line in out.lines() {
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if let Some(text) = line.strip_prefix('-') {
            removed.push(text.to_string());
        } else if let Some(text) = line.strip_prefix('+') {
            added.push(text.to_string());
        }
    }
    (removed, added)
}

/// The commit tab's text of `id` in `group`.
fn commit_of(app: &App, group: crate::layout::GroupId) -> &crate::commit_view::CommitDoc {
    app.commit_in(group).unwrap()
}

/// Every commit tab loaded (a session restore starts their reads).
fn commits_loaded(app: &App) -> bool {
    app.buffers
        .iter()
        .all(|(_, b)| b.commit.as_ref().is_none_or(|c| c.show.is_some() || c.error.is_some()))
}

async fn quit(mut app: App) {
    app.test_execute(Command::Quit);
    assert!(app.quitting());
    app.shutdown().await;
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
    focus_history(&mut app);
    assert_eq!(app.git_history.selected(), None);
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(app.last_command, Some(Command::GitHistory(G::Down)));
    assert_eq!(app.git_history.selected(), Some(ids[0].as_str()));
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(app.git_history.selected(), Some(ids[1].as_str()));
    app.press(KeyCode::Down, M::NONE);
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
    focus_history(&mut app);
    app.press(KeyCode::Esc, M::NONE);
    assert!(!app.git_history.focused());
    assert_eq!(app.last_command, Some(Command::GitHistory(G::FocusTree)));

    // Focusing the Explorer (Ctrl+Shift+E) likewise gives the tree the keys.
    focus_history(&mut app);
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
    focus_history(&mut app);
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
    // A click opens the commit too (as Enter does), reusing a tab if there is one.
    let commit = app.groups.ids().into_iter().find_map(|g| {
        Some((
            g,
            app.groups
                .get(g)?
                .tabs
                .iter()
                .find_map(|t| tab_commit(&app, t))?
                .commit
                .clone(),
        ))
    });
    assert_eq!(commit.map(|(_, c)| c), Some(target), "the clicked commit's tab");
    assert_eq!(app.ui.focus, Focus::Editor, "the commit tab has the keys");
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
    focus_history(&mut app);
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

/// Two commits, the second changing a line of `a.txt` (a `+` and a `-` in its stat) and adding
/// a line to `b.txt`; the first is a root commit.
fn two_commits(root: &Path) -> (String, String) {
    write(root, "a.txt", "one\ntwo\nthree\n");
    write(root, "b.txt", "b\n");
    commit_all(root, "first");
    let first = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    write(root, "a.txt", "one\nTWO\nthree\n");
    write(root, "b.txt", "b\nmore\n");
    commit_all(root, "second");
    let second = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    (first, second)
}

/// A commit whose stat depends on the width: one long-named file, most of its lines changed
/// (git truncates the name and shrinks the graph when the tab is narrow).
fn wide_commit(root: &Path) -> (String, String) {
    let name = "a_very_long_file_name_that_is_truncated_in_narrow_tabs.txt";
    let lines: String = (0..120).map(|i| format!("line {i}\n")).collect();
    write(root, name, &lines);
    commit_all(root, "first");
    let changed: String = (0..120)
        .map(|i| {
            if i % 2 == 0 {
                format!("changed {i}\n")
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    write(root, name, &changed);
    commit_all(root, "second");
    let id = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    (name.to_string(), id)
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_enter_opens_a_commit_tab() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);

    // Enter on the selected commit opens its tab (the file's history's file named).
    let id = enter_commit(&mut app).await;
    assert_eq!(id, second, "the newest commit");
    {
        let c = commit_of(&app, app.groups.active);
        assert_eq!(c.commit, second);
        assert_eq!(c.title(), &second[..7]);
        assert_eq!(c.file.as_deref(), Some("a.txt"));
    }
    assert_eq!(app.group().tabs.len(), 2, "the file's tab and the commit's");
    assert_eq!(app.group().active, 1);

    // Drawn at the text area's width: exactly `git show --stat --format=medium`.
    let _term = draw(&mut app);
    let width = text_width(&app, app.groups.active);
    let lines = commit_lines(&app);
    assert_eq!(lines.join("\n"), cli_show(&root, &second, width));
    assert!(lines[0].starts_with("commit "), "{:?}", lines[0]);

    // Back to the file's tab and Enter the same commit again: the tab is reused.
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    assert_eq!(enter_commit(&mut app).await, second);
    assert_eq!(app.group().tabs.len(), 2, "no second commit tab");
    assert_eq!(app.group().active, 1, "the commit tab activated");
    assert_eq!(app.active_commit().unwrap().commit, second);
}

/// The Done-when's whole walk against the `git` CLI: newest-first commits as `git log` lists
/// them, switching tabs switching the list, a click on a commit giving a tab whose lines and
/// colours are `git show --stat --format=medium`'s at the drawn width, a click on its stat row
/// giving a side-by-side diff to the right whose changed lines are `git diff <commit>^ <commit>
/// -- <file>`'s (no stage / revert buttons), a commit made outside jed at the top, and quit /
/// reopen restoring both tabs and the section's layout.
#[tokio::test(flavor = "multi_thread")]
async fn git_history_done_when() {
    let (_d, root, state) = dirs();
    init(&root);
    // A few commits: both files, then a, then a again (the newest), then b only. a is long
    // enough that `git show`'s stat layout changes with the width it is given (a fixture whose
    // stat is the same at every width would not tell the tab's width from a default).
    let lines: String = (0..120).map(|i| format!("line {i}\n")).collect();
    write(&root, "a.txt", &lines);
    write(&root, "b.txt", "b\n");
    commit_all(&root, "first: both files");
    write(&root, "a.txt", &format!("{lines}two\n"));
    commit_all(&root, "second: adds a line");
    let changed: String = (0..120).map(|i| format!("changed {i}\n")).collect();
    write(&root, "a.txt", &changed);
    commit_all(&root, "third: changes a");
    let third = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let parent = git(&root, &["rev-parse", &format!("{third}^")]).trim().to_string();
    write(&root, "b.txt", "b\nb2\n");
    commit_all(&root, "fourth: changes b");
    let (a, b) = (root.join("a.txt"), root.join("b.txt"));

    let mut app = open_state_app_watched(&root, &state, Some("a.txt"));
    assert!(settle_history(&mut app, &a, 3).await);
    // The list is `git log -- <path>`'s: newest first, a's commits only.
    let expected = cli_log(&root, "a.txt");
    assert_eq!(
        app.git_history.rows().iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        expected,
        "newest first"
    );
    let term = draw(&mut app);
    let hit = app.hits.history.expect("the section is drawn");
    assert!(list_row(&term, hit.list, 0).contains("third: changes a"));
    assert!(list_row(&term, hit.list, 2).contains("first: both files"));

    // Another file's tab: the section follows it; back again: a's list.
    app.test_open(Buffer::open(&b).unwrap());
    assert!(settle_history(&mut app, &b, 2).await);
    assert_eq!(
        app.git_history.rows().iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        cli_log(&root, "b.txt")
    );
    let term = draw(&mut app);
    assert!(list_row(&term, app.hits.history.unwrap().list, 0).contains("fourth: changes b"));
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(settle_history(&mut app, &a, 3).await);
    assert_eq!(app.git_history.rows()[0].id, third);

    // A click on the newest commit opens its tab.
    let _term = draw(&mut app);
    click_history_row(&mut app, 0);
    assert_eq!(app.git_history.selected(), Some(third.as_str()));
    assert!(
        app.settle(|a| a.active_commit().is_some_and(|c| c.show.is_some()))
            .await,
        "the commit loaded"
    );
    let group = app.groups.active;
    {
        let c = commit_of(&app, group);
        assert_eq!(c.commit, third);
        assert_eq!(c.file.as_deref(), Some("a.txt"));
    }

    // The text is `git show --stat --format=medium` at the drawn tab's width, line by line —
    // the model's lines and the cells — and the colours are git's, by line kind.
    let term = draw(&mut app);
    let width = text_width(&app, group);
    let expected_lines: Vec<String> = cli_show(&root, &third, width).lines().map(str::to_string).collect();
    assert_eq!(commit_lines(&app), expected_lines, "the buffer's lines");
    let rows = app.hits.group(group).unwrap().text;
    let buf = term.backend().buffer();
    let drawn: Vec<String> = (0..rows.height)
        .map(|i| {
            (rows.x..rows.x + rows.width)
                .map(|x| buf[(x, rows.y + i)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    for (i, line) in expected_lines.iter().enumerate() {
        assert_eq!(&drawn[i], line, "drawn line {i}");
    }
    assert!(
        drawn[expected_lines.len()..].iter().all(String::is_empty),
        "nothing drawn below the text"
    );
    let at = expected_lines[0]
        .find(&third)
        .unwrap_or_else(|| panic!("no commit id in {:?}", expected_lines[0]));
    assert_eq!(
        buf[(rows.x + at as u16, rows.y)].style().fg,
        app.theme.get("gitShow.commit").fg
    );
    let a_row = {
        let c = commit_of(&app, group);
        let k = c
            .show
            .as_ref()
            .unwrap()
            .files
            .iter()
            .position(|f| f.path == "a.txt")
            .unwrap();
        c.file_row(k).unwrap()
    };
    let line = &expected_lines[a_row];
    let (plus, minus) = (
        line.find('+').unwrap_or_else(|| panic!("no + in {line:?}")),
        line.find('-').unwrap_or_else(|| panic!("no - in {line:?}")),
    );
    let y = rows.y + a_row as u16;
    assert_eq!(
        buf[(rows.x + plus as u16, y)].style().fg,
        app.theme.get("gitShow.add").fg
    );
    assert_eq!(
        buf[(rows.x + minus as u16, y)].style().fg,
        app.theme.get("gitShow.del").fg
    );
    assert_eq!(
        buf[(rows.x + 1, y)].style().bg,
        app.theme.get("editor.lineHighlight").bg,
        "the file whose history opened it is highlighted"
    );

    // A click on the stat row opens a.txt's change in a group to the right, side by side, with
    // the changed lines `git diff <commit>^ <commit> -- a.txt` shows and no buttons.
    click_commit_row(&mut app, group, a_row);
    assert!(app.settle(|a| a.diffs_settled()).await, "the diff loaded");
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 2, "a group split to the right");
    let target = ids[1];
    assert_eq!(app.groups.active, target, "the diff's group has the keys");
    let term = draw_sized(&mut app, 240, 24);
    {
        let d = app.groups.get(target).unwrap().active_diff().unwrap();
        assert!(!d.inline, "side by side at 240 columns");
        assert_eq!(
            d.pair,
            Pair::commit(
                git2::Oid::from_str(&third).unwrap(),
                Some(git2::Oid::from_str(&parent).unwrap())
            )
        );
        assert_eq!(d.title(), format!("a.txt ({})", &third[..7]));
    }
    let actual = diff_changes(
        app.groups
            .get(target)
            .unwrap()
            .active_diff()
            .unwrap()
            .doc
            .as_ref()
            .unwrap(),
    );
    assert_eq!(actual, cli_diff(&root, &format!("{third}^"), &third, "a.txt"));
    // Drawn side by side: the first paired change's old and new lines share one row, the old
    // on the left half and the new on the right.
    let (first, old_text, new_text) = {
        let d = app.groups.get(target).unwrap().active_diff().unwrap();
        let doc = d.doc.as_ref().unwrap();
        let rows = &doc.aligned(false).rows;
        let i = rows
            .iter()
            .position(|r| r.hunk.is_some() && r.left.is_some() && r.right.is_some())
            .expect("a paired change");
        let r = &rows[i];
        (
            i,
            doc.left.line(r.left.unwrap()).to_string(),
            doc.right.line(r.right.unwrap()).to_string(),
        )
    };
    {
        let d = app.groups.get(target).unwrap().active_diff().unwrap();
        let hit = app.hits.group(target).unwrap().diff.as_ref().unwrap();
        let at = first
            .checked_sub(d.top)
            .filter(|r| *r < hit.rows.height as usize)
            .expect("the first change is on screen");
        let y = hit.rows.y + at as u16;
        let row: String = (hit.rows.x..hit.rows.x + hit.rows.width)
            .map(|x| term.backend().buffer()[(x, y)].symbol())
            .collect();
        assert!(row.contains(&old_text), "the old line on the left: {row:?}");
        assert!(row.contains(&new_text), "the new line on the right: {row:?}");
    }
    let _term = draw(&mut app);
    let hit = app.hits.group(target).unwrap().diff.as_ref().unwrap();
    assert!(
        hit.buttons.is_empty() && hit.reverts.is_empty(),
        "nothing to stage or revert"
    );

    // A commit made outside jed (the watcher sees the work tree and `.git`) is at the top.
    write(&root, "a.txt", "one\nTWO\nfour\n");
    commit_all(&root, "fifth: outside jed");
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    assert!(
        app.settle(|a| {
            !a.git_history.loading
                && a.git_history.rows().len() == 4
                && a.git_history.rows().first().is_some_and(|c| c.id == head)
        })
        .await,
        "the new commit is at the top: {:?}",
        app.git_history
    );
    assert_eq!(app.git_history.rows()[0].summary, "fifth: outside jed");
    // The history's first page is `git log`'s, still.
    assert_eq!(
        app.git_history.rows().iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        cli_log(&root, "a.txt")
    );

    // Quit and reopen: both tabs and the section's collapsed state and height come back.
    app.git_history.set_collapsed(true);
    app.git_history.section_height = 7;
    quit(app).await;
    let mut app = open_state_app(&root, &state, None);
    assert!(
        app.settle(|a| !a.session.loading() && a.session.restored == 1).await,
        "the session restored"
    );
    assert!(app.settle(|a| commits_loaded(a) && a.diffs_settled()).await);
    let oid = git2::Oid::from_str(&third).unwrap();
    let tabs: Vec<&Tab> = app
        .groups
        .ids()
        .into_iter()
        .flat_map(|g| app.groups.get(g).unwrap().tabs.iter())
        .collect();
    assert!(
        tabs.iter()
            .any(|t| tab_commit(&app, t).is_some_and(|c| c.commit == third)),
        "the commit tab"
    );
    assert!(
        tabs.iter()
            .any(|t| t.diff().is_some_and(|d| d.pair.right == Text::Commit(oid))),
        "its diff tab"
    );
    assert!(app.git_history.collapsed());
    assert_eq!(app.git_history.section_height, 7);
}

/// The history is read only while the section shows: with the side bar hidden (or the section
/// collapsed) a new active file reads nothing until it shows again.
#[tokio::test(flavor = "multi_thread")]
async fn git_history_reads_only_while_shown() {
    let (_d, root, state) = dirs();
    init(&root);
    two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);

    app.test_execute(Command::ToggleSidebar);
    assert!(!app.ui.side_visible);
    app.test_open(Buffer::open(&root.join("b.txt")).unwrap());
    app.pump();
    assert_eq!(app.git_history.path.as_deref(), Some(root.join("b.txt").as_path()));
    assert!(!app.git_history.loading, "nothing read while hidden");
    assert!(app.git_history.commits.is_empty());

    app.test_execute(Command::ToggleSidebar);
    assert!(
        settle_history(&mut app, &root.join("b.txt"), 2).await,
        "read once shown"
    );

    // Collapsed: the same.
    app.git_history.set_collapsed(true);
    app.test_execute(Command::PrevTab);
    app.pump();
    assert_eq!(app.git_history.path.as_deref(), Some(root.join("a.txt").as_path()));
    assert!(!app.git_history.loading && app.git_history.commits.is_empty());
    app.git_history.set_collapsed(false);
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
}

/// Enter with the section focused but nothing selected opens the first commit (as a click
/// would); a file with no history opens nothing.
#[tokio::test(flavor = "multi_thread")]
async fn git_history_enter_with_nothing_selected_opens_the_first_commit() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    write(&root, "untracked.txt", "u\n");
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);

    // A file with no history: Enter selects and opens nothing.
    app.test_open(Buffer::open(&root.join("untracked.txt")).unwrap());
    assert!(settle_history(&mut app, &root.join("untracked.txt"), 0).await);
    let _term = draw(&mut app);
    focus_history(&mut app);
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(app.last_command, Some(Command::GitHistory(G::Open)));
    assert_eq!(app.git_history.selected(), None);
    assert_eq!(app.group().tabs.len(), 2, "the two files' tabs");
    assert!(
        app.group().tabs.iter().all(|t| tab_commit(&app, t).is_none()),
        "no commit tab"
    );

    // Back to a.txt with nothing selected: Enter opens the newest commit.
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    focus_history(&mut app);
    assert_eq!(app.git_history.selected(), None);
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(app.git_history.selected(), Some(second.as_str()));
    assert!(
        app.settle(|a| a.active_commit().is_some_and(|c| c.show.is_some()))
            .await
    );
    assert_eq!(app.active_commit().unwrap().commit, second);
}

/// A stat row's diff goes in the group drawn to the right, not the next in reading order
/// (which is below when the commit tab's group is split both ways).
#[tokio::test(flavor = "multi_thread")]
async fn git_history_stat_row_opens_the_diff_to_the_right_not_below() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw_sized(&mut app, 240, 30);
    enter_commit(&mut app).await;

    // A group to the right, and one below the commit tab's group (reading order goes down
    // first: ids are [top, below, right]).
    app.test_execute(Command::SplitRight);
    app.test_execute(Command::FocusGroup(1));
    app.test_execute(Command::SplitDown);
    let group = app.groups.ids()[0];
    let (right, below) = {
        let (cells, _) = app.groups.tree.layout(app.hits.regions.editor);
        let here = cells.iter().find(|(g, _)| *g == group).unwrap().1;
        let right = cells
            .iter()
            .find(|(_, r)| r.x >= here.x + here.width && r.y < here.y + here.height)
            .unwrap()
            .0;
        let below = cells.iter().find(|(_, r)| r.y >= here.y + here.height).unwrap().0;
        assert_eq!(app.groups.ids()[1], below, "reading order goes below first");
        (right, below)
    };
    let _term = draw_sized(&mut app, 240, 30);

    // Open a.txt's stat row from the top group's commit tab.
    app.test_execute(Command::FocusGroup(1));
    let a_row = {
        let c = app.commit_in(group).unwrap();
        let k = c
            .show
            .as_ref()
            .unwrap()
            .files
            .iter()
            .position(|f| f.path == "a.txt")
            .unwrap();
        c.file_row(k).unwrap()
    };
    doc_start(&mut app);
    for _ in 0..a_row {
        app.press(KeyCode::Down, M::NONE);
    }
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);

    assert_eq!(app.groups.ids().len(), 3, "no third split");
    assert_eq!(app.groups.active, right, "the drawn group to the right");
    let g = app.groups.get(right).unwrap();
    let d = g.active_diff().expect("the diff is to the right");
    assert_eq!(d.path, root.join("a.txt"));
    assert_eq!(d.pair.right, Text::Commit(git2::Oid::from_str(&second).unwrap()));
    assert_eq!(g.tabs.len(), 2, "the right group's commit copy and the diff");
    let below_tabs = &app.groups.get(below).unwrap().tabs;
    assert_eq!(below_tabs.len(), 1, "the group below is untouched");
    assert!(tab_commit(&app, &below_tabs[0]).is_some());
}

/// With a group below but none to the right (a vertical split), a stat row splits a group to
/// the right rather than opening in the group below.
#[tokio::test(flavor = "multi_thread")]
async fn git_history_stat_row_below_only_splits_right() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw_sized(&mut app, 160, 30);
    enter_commit(&mut app).await;
    let group = app.groups.active;

    // Split down: one group below the commit tab's, nothing to its right.
    app.test_execute(Command::SplitDown);
    let below = app.groups.active;
    let _term = draw_sized(&mut app, 160, 30);
    app.test_execute(Command::FocusGroup(1));
    let a_row = {
        let c = app.commit_in(group).unwrap();
        let k = c
            .show
            .as_ref()
            .unwrap()
            .files
            .iter()
            .position(|f| f.path == "a.txt")
            .unwrap();
        c.file_row(k).unwrap()
    };
    doc_start(&mut app);
    for _ in 0..a_row {
        app.press(KeyCode::Down, M::NONE);
    }
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);

    let ids = app.groups.ids();
    assert_eq!(ids.len(), 3, "a new group to the right, none reused below");
    assert_eq!(
        app.groups.get(below).unwrap().tabs.len(),
        1,
        "the below group is untouched"
    );
    let target = *ids.iter().find(|g| **g != group && **g != below).unwrap();
    let (cells, _) = app.groups.tree.layout(app.hits.regions.editor);
    let here = cells.iter().find(|(g, _)| *g == group).unwrap().1;
    let cell = cells.iter().find(|(g, _)| *g == target).unwrap().1;
    assert!(cell.x >= here.x + here.width, "the new group is to the right");
    let d = app.groups.get(target).unwrap().active_diff().unwrap();
    assert_eq!(d.path, root.join("a.txt"));
    assert_eq!(d.pair.right, Text::Commit(git2::Oid::from_str(&second).unwrap()));
}

/// Before the first frame there's no geometry: a stat row's diff falls back to the next group in
/// reading order, or a split to the right when there is none.
#[tokio::test(flavor = "multi_thread")]
async fn git_history_stat_row_without_a_frame_splits_right() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    // No draw at all: Enter (nothing selected) opens the newest commit.
    focus_history(&mut app);
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| a.active_commit().is_some_and(|c| c.show.is_some()))
            .await
    );
    let k = {
        let c = app.active_commit().unwrap();
        c.show
            .as_ref()
            .unwrap()
            .files
            .iter()
            .position(|f| f.path == "a.txt")
            .unwrap()
    };
    // Not drawn: the text is rendered at a default width, the cursor on a.txt's stat line.
    let c = app.active_commit().unwrap();
    let row = c.file_row(k).unwrap();
    assert_eq!(cursor_line(&app), row);
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 2, "split to the right without a frame");
    let d = app.groups.get(ids[1]).unwrap().active_diff().unwrap();
    assert_eq!(d.path, root.join("a.txt"));
    assert_eq!(d.pair.right, Text::Commit(git2::Oid::from_str(&second).unwrap()));
}

/// Before the first frame with a group already beside the commit tab's, a stat row's diff goes
/// in that group (the next in reading order), not in a third one.
#[tokio::test(flavor = "multi_thread")]
async fn git_history_stat_row_without_a_frame_reuses_the_next_group() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    // No draw: Enter (nothing selected) opens the newest commit, and a split right gives it a
    // neighbour before any frame laid the groups out.
    focus_history(&mut app);
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| a.active_commit().is_some_and(|c| c.show.is_some()))
            .await
    );
    app.test_execute(Command::SplitRight);
    let (first, next) = (app.groups.ids()[0], app.groups.ids()[1]);
    assert_eq!(app.groups.active, next, "the split's group");
    app.test_execute(Command::FocusGroup(1));
    let k = {
        let c = app.commit_in(first).unwrap();
        c.show
            .as_ref()
            .unwrap()
            .files
            .iter()
            .position(|f| f.path == "a.txt")
            .unwrap()
    };
    // The cursor starts on a.txt's stat line.
    assert!(app.commit_in(first).unwrap().file_row(k).is_some());
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(app.groups.ids().len(), 2, "the next group reused, no new split");
    let g = app.groups.get(next).unwrap();
    assert_eq!(g.tabs.len(), 2, "the group's commit copy and the diff");
    let d = g.active_diff().expect("the diff in the next group");
    assert_eq!(d.path, root.join("a.txt"));
    assert_eq!(d.pair.right, Text::Commit(git2::Oid::from_str(&second).unwrap()));
}

/// The commit tab's text at a width that makes git's stat layout depend on it, compared with
/// `git show --stat-width`: narrow (truncated name, shrunk graph), then wide (re-rendered).
#[tokio::test(flavor = "multi_thread")]
async fn git_history_commit_tab_matches_git_show_at_each_width() {
    let (_d, root, state) = dirs();
    init(&root);
    let (name, second) = wide_commit(&root);
    let mut app = open_file_app(&root, &state, &name);
    assert!(settle_history(&mut app, &root.join(&name), 2).await);
    let _term = draw(&mut app);
    enter_commit(&mut app).await;

    let _term = draw(&mut app);
    let narrow = text_width(&app, app.groups.active);
    assert!(narrow < 80, "a narrow tab: {narrow}");
    let at_narrow = commit_lines(&app).join("\n");
    assert_eq!(at_narrow, cli_show(&root, &second, narrow), "narrow width {narrow}");

    // A wider tab re-renders its lines at the new width (the commit isn't read again).
    let _term = draw_sized(&mut app, 220, 24);
    let wide = text_width(&app, app.groups.active);
    assert!(wide > narrow, "{wide} > {narrow}");
    let at_wide = commit_lines(&app).join("\n");
    assert_eq!(at_wide, cli_show(&root, &second, wide), "wide width {wide}");
    assert_ne!(at_wide, at_narrow, "the width changed the stat's layout");
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_commit_tab_colours_and_file_row() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    enter_commit(&mut app).await;
    let term = draw(&mut app);
    let group = app.groups.active;
    let rows = app.hits.group(group).unwrap().text;
    let top = app.group().active_tab().unwrap().view.top_line();
    let (a_row, b_row) = {
        let c = app.active_commit().unwrap();
        let show = c.show.as_ref().unwrap();
        let a = show.files.iter().position(|f| f.path == "a.txt").unwrap();
        let b = show.files.iter().position(|f| f.path == "b.txt").unwrap();
        (c.file_row(a).unwrap(), c.file_row(b).unwrap())
    };
    let buf = term.backend().buffer();
    let text_row = |row: usize| -> String {
        (rows.x..rows.x + rows.width)
            .map(|x| buf[(x, rows.y + (row - top) as u16)].symbol())
            .collect()
    };
    // The commit line's id in git's yellow.
    let line = text_row(0);
    let at = line
        .find(&second)
        .unwrap_or_else(|| panic!("no id in {line:?} {rows:?}"));
    assert_eq!(
        buf[(rows.x + at as u16, rows.y)].style().fg,
        app.theme.get("gitShow.commit").fg,
        "{line:?}"
    );
    // a.txt's stat row: its `+` green and its `-` red.
    let line = text_row(a_row);
    let plus = line.find('+').unwrap_or_else(|| panic!("no + in {line:?}"));
    let minus = line.find('-').unwrap_or_else(|| panic!("no - in {line:?}"));
    let fg = |y: u16, x: usize| buf[(rows.x + x as u16, y)].style().fg;
    // The other lines are git's default colour (the editor's): here the Author line.
    let author = text_row(1);
    let at = author
        .find("Author:")
        .unwrap_or_else(|| panic!("no author in {author:?}"));
    assert_eq!(
        fg(rows.y + (1 - top) as u16, at),
        app.theme.get("editor").fg,
        "{author:?}"
    );
    assert_eq!(fg(rows.y + (a_row - top) as u16, plus), app.theme.get("gitShow.add").fg);
    assert_eq!(
        fg(rows.y + (a_row - top) as u16, minus),
        app.theme.get("gitShow.del").fg
    );
    // The tab's own file's stat row is highlighted; another file's isn't.
    let bg = |row: usize| buf[(rows.x + 1, rows.y + (row - top) as u16)].style().bg;
    assert_eq!(bg(a_row), app.theme.get("editor.lineHighlight").bg);
    assert_eq!(bg(b_row), app.theme.get("editor").bg);
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_stat_row_opens_a_diff_to_the_right() {
    let (_d, root, state) = dirs();
    init(&root);
    let (first, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    enter_commit(&mut app).await;
    let _term = draw(&mut app);

    // Enter on the commit line (no stat there): a message, no diff, no split.
    doc_start(&mut app);
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(
        app.last_command,
        Some(Command::Commit(crate::command::CommitCommand::Open))
    );
    assert_eq!(app.message.as_ref().unwrap().text, "No file on this line");
    assert_eq!(app.groups.ids().len(), 1, "no group split");
    assert_eq!(app.group().tabs.len(), 2, "the commit tab only");

    // Enter on a.txt's stat row: a diff in the group to the right.
    let group = app.groups.active;
    let a_row = {
        let c = app.active_commit().unwrap();
        let show = c.show.as_ref().unwrap();
        let a = show.files.iter().position(|f| f.path == "a.txt").unwrap();
        c.file_row(a).unwrap()
    };
    doc_start(&mut app);
    for _ in 0..a_row {
        app.press(KeyCode::Down, M::NONE);
    }
    assert_eq!(cursor_line(&app), a_row);
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(
        app.last_command,
        Some(Command::Commit(crate::command::CommitCommand::Open))
    );

    let ids = app.groups.ids();
    assert_eq!(ids.len(), 2, "a group split to the right");
    assert_eq!(app.groups.active, ids[1]);
    let target = ids[1];
    let (path, pair) = {
        let d = app.groups.get(target).unwrap().active_diff().unwrap();
        (d.path.clone(), d.pair)
    };
    let oid = git2::Oid::from_str(&second).unwrap();
    let parent = git2::Oid::from_str(&first).unwrap();
    assert_eq!(path, root.join("a.txt"));
    assert_eq!(pair, Pair::commit(oid, Some(parent)));
    assert_eq!(
        app.groups.get(target).unwrap().active_diff().unwrap().title(),
        format!("a.txt ({})", &second[..7]),
        "the tab names the commit"
    );
    assert!(app.settle(|a| a.diffs_settled()).await, "the diff loaded");

    // Wide enough (each group 90 columns or more), the diff is drawn side by side.
    let _term = draw_sized(&mut app, 240, 24);
    assert!(
        !app.groups.get(target).unwrap().active_diff().unwrap().inline,
        "side by side at 240 columns"
    );
    let _term = draw(&mut app);

    // Both sides are the commit's texts; the changed lines are git diff's.
    let (left, right, removed, added) = {
        let d = app.groups.get(target).unwrap().active_diff().unwrap();
        let doc = d.doc.as_ref().unwrap();
        let join =
            |lines: &crate::diff_view::Lines| (0..lines.len()).map(|i| lines.line(i)).collect::<Vec<_>>().join("\n");
        let mut removed = Vec::new();
        let mut added = Vec::new();
        for row in &doc.aligned(false).rows {
            if row.hunk.is_some() {
                if let Some(l) = row.left {
                    removed.push(doc.left.line(l).to_string());
                }
                if let Some(r) = row.right {
                    added.push(doc.right.line(r).to_string());
                }
            }
        }
        (join(&doc.left), join(&doc.right), removed, added)
    };
    assert_eq!(
        left,
        git(&root, &["show", &format!("{first}:a.txt")]).trim_end_matches('\n')
    );
    assert_eq!(
        right,
        git(&root, &["show", &format!("{second}:a.txt")]).trim_end_matches('\n')
    );
    let (want_removed, want_added) = {
        let out = git(
            &root,
            &["diff", "--no-color", "--unified=0", &first, &second, "--", "a.txt"],
        );
        let (mut removed, mut added) = (Vec::new(), Vec::new());
        for line in out.lines() {
            if line.starts_with("---") || line.starts_with("+++") {
                continue;
            }
            if let Some(text) = line.strip_prefix('-') {
                removed.push(text.to_string());
            } else if let Some(text) = line.strip_prefix('+') {
                added.push(text.to_string());
            }
        }
        (removed, added)
    };
    assert_eq!((removed, added), (want_removed, want_added));

    // A commit diff stages nothing: no Stage / Revert buttons.
    let _term = draw(&mut app);
    let hit = app.hits.group(target).unwrap().diff.as_ref().unwrap();
    assert!(hit.buttons.is_empty() && hit.reverts.is_empty());

    // A click on b.txt's stat row retargets the same diff tab to b.txt.
    let b_row = {
        let c = app.commit_in(group).unwrap();
        let show = c.show.as_ref().unwrap();
        let b = show.files.iter().position(|f| f.path == "b.txt").unwrap();
        c.file_row(b).unwrap()
    };
    let top = app.groups.get(group).unwrap().active_tab().unwrap().view.top_line();
    let rows = app.hits.group(group).unwrap().text;
    app.mouse(
        MouseEventKind::Down(MouseButton::Left),
        rows.x + 2,
        rows.y + (b_row - top) as u16,
    );
    assert_eq!(app.groups.active, target, "the diff's group");
    let g = app.groups.get(target).unwrap();
    assert_eq!(g.tabs.len(), 1, "the commit diff tab reused");
    let d = g.active_diff().unwrap();
    assert_eq!(d.path, root.join("b.txt"));
    assert_eq!(d.pair.right, Text::Commit(oid));
}

/// The files of a commit whose change is a root commit (`Empty` ↔ the commit), a deletion (the
/// parent's text ↔ nothing) and a binary file (the diff says so), from their stat rows.
#[tokio::test(flavor = "multi_thread")]
async fn git_history_stat_rows_open_root_deleted_and_binary_diffs() {
    let (_d, root, state) = dirs();
    init(&root);
    write(&root, "root.txt", "r\n");
    write(&root, "gone.txt", "gone\n");
    fs::write(root.join("bin.dat"), b"\x00binary").unwrap();
    commit_all(&root, "root");
    let first = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    write(&root, "root.txt", "r\nmore\n");
    fs::remove_file(root.join("gone.txt")).unwrap();
    fs::write(root.join("bin.dat"), b"\x00binary changed").unwrap();
    commit_all(&root, "second");
    let second = git(&root, &["rev-parse", "HEAD"]).trim().to_string();

    let mut app = open_file_app(&root, &state, "root.txt");
    assert!(settle_history(&mut app, &root.join("root.txt"), 2).await);
    let _term = draw(&mut app);
    assert_eq!(enter_commit(&mut app).await, second);
    let _term = draw(&mut app);

    // Move the commit tab's cursor to `path`'s stat row (from the top) and open its diff.
    let goto_row = |app: &mut App, path: &str| {
        doc_start(app);
        let c = app.active_commit().unwrap();
        let show = c.show.as_ref().unwrap();
        let k = show.files.iter().position(|f| f.path == path).unwrap();
        let row = c.file_row(k).unwrap();
        for _ in 0..row {
            app.press(KeyCode::Down, M::NONE);
        }
        app.press(KeyCode::Enter, M::NONE);
    };
    let texts = |doc: &crate::diff_view::Doc| {
        let join =
            |lines: &crate::diff_view::Lines| (0..lines.len()).map(|i| lines.line(i)).collect::<Vec<_>>().join("\n");
        (join(&doc.left), join(&doc.right))
    };

    // gone.txt, deleted: the first parent's text against nothing.
    goto_row(&mut app, "gone.txt");
    assert!(app.settle(|a| a.diffs_settled()).await);
    let ids = app.groups.ids();
    let target = ids[1];
    {
        let d = app.groups.get(target).unwrap().active_diff().unwrap();
        assert_eq!(
            d.pair,
            Pair::commit(
                git2::Oid::from_str(&second).unwrap(),
                Some(git2::Oid::from_str(&first).unwrap())
            )
        );
        let (left, right) = texts(d.doc.as_ref().unwrap());
        assert_eq!(
            left,
            git(&root, &["show", &format!("{first}:gone.txt")]).trim_end_matches('\n')
        );
        assert_eq!(right, "", "a deletion has no new lines");
    }

    // bin.dat is binary: the diff says so.
    app.test_execute(Command::FocusGroup(1));
    goto_row(&mut app, "bin.dat");
    assert!(app.settle(|a| a.diffs_settled()).await, "retargeted to bin.dat");
    let d = app.groups.get(target).unwrap().active_diff().unwrap();
    assert!(d.doc.as_ref().unwrap().binary, "a binary file");

    // Back to the file, the root commit: its change is against the empty tree.
    app.test_execute(Command::FocusGroup(1));
    app.test_execute(Command::NextTab);
    assert!(settle_history(&mut app, &root.join("root.txt"), 2).await);
    focus_history(&mut app);
    // The commit tab's turn cleared the section's selection: Down selects the newest, then the root.
    app.press(KeyCode::Down, M::NONE);
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(app.git_history.selected(), Some(first.as_str()));
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| a.active_commit().is_some_and(|c| c.show.is_some()))
            .await
    );
    let _term = draw(&mut app);
    goto_row(&mut app, "root.txt");
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.groups.get(target).unwrap().active_diff().unwrap();
    assert_eq!(d.pair, Pair::commit(git2::Oid::from_str(&first).unwrap(), None));
    let (left, right) = texts(d.doc.as_ref().unwrap());
    assert_eq!(left, "", "a root commit's left side is the empty tree");
    assert_eq!(
        right,
        git(&root, &["show", &format!("{first}:root.txt")]).trim_end_matches('\n')
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_commit_tab_selects_and_copies() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    enter_commit(&mut app).await;
    let _term = draw(&mut app);
    let width = text_width(&app, app.groups.active);
    // The expected text comes from git itself, not from the tab's rendering.
    let shown = cli_show(&root, &second, width);
    let lines: Vec<&str> = shown.lines().collect();
    let cmd = if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    };

    // From the top, Shift+Down twice: the first two lines selected (the second's start included).
    doc_start(&mut app);
    app.press(KeyCode::Down, M::SHIFT);
    app.press(KeyCode::Down, M::SHIFT);
    app.press(KeyCode::Char('c'), cmd);
    assert_eq!(app.clipboard.read().unwrap(), format!("{}\n{}\n", lines[0], lines[1]));
    // Drawn: the selection covers those two lines, not the third.
    let term = draw(&mut app);
    let group = app.groups.active;
    let rows = app.hits.group(group).unwrap().text;
    let buf = term.backend().buffer();
    let selection = app.theme.get("editor.selection").bg;
    let bg = |row: u16, x: u16| buf[(rows.x + x, rows.y + row)].style().bg;
    assert_eq!(bg(0, 1), selection, "the first line is selected");
    assert_eq!(bg(1, 1), selection, "the second line is selected");
    assert_ne!(bg(2, 1), selection, "the cursor's line is not");
    // An empty cursor copies its whole line.
    app.press(KeyCode::Esc, M::NONE);
    app.press(KeyCode::Char('c'), cmd);
    assert_eq!(app.clipboard.read().unwrap(), format!("{}\n", lines[2]));
    // Cut does nothing (read only): the clipboard keeps what Copy left.
    app.press(KeyCode::Down, M::SHIFT);
    app.press(KeyCode::Char('x'), cmd);
    assert_eq!(app.clipboard.read().unwrap(), format!("{}\n", lines[2]));
}

/// A commit tab is a read-only buffer without a file: typing, pasting, Save and file-keyed
/// commands leave its text and the work tree alone. (`tools_every_command_on_a_log_tab_leaves_it_alone`
/// runs every command on the same kind of buffer.)
#[tokio::test(flavor = "multi_thread")]
async fn git_history_a_commit_tab_is_read_only() {
    let (_d, root, state) = dirs();
    init(&root);
    two_commits(&root);
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    enter_commit(&mut app).await;
    let _term = draw(&mut app);
    let before = commit_lines(&app);
    let on_disk = fs::read_to_string(root.join("a.txt")).unwrap();
    let status = git(&root, &["status", "--porcelain"]);

    app.type_text("hello");
    app.handle_test_event(Event::Paste("pasted\n".into()));
    app.test_execute(Command::Save);
    app.test_execute(Command::OpenChange);
    app.test_execute(Command::ShowFileHistory);
    app.test_execute(Command::RevertHunk);
    app.test_execute(Command::StageHunk);
    app.test_execute(Command::UnstageHunk);
    app.test_execute(Command::TriggerSuggest);
    let _term = draw(&mut app);
    assert_eq!(commit_lines(&app), before, "the text is unchanged");
    let buf = app.active_buffer().unwrap();
    assert!(!buf.is_dirty() && !buf.has_file());
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), on_disk);
    assert_eq!(
        git(&root, &["status", "--porcelain"]),
        status,
        "nothing written to the repository"
    );
    assert!(
        app.active_commit().is_some_and(|c| c.show.is_some()),
        "still the commit tab"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_history_session_restores_commit_tabs() {
    let (_d, root, state) = dirs();
    init(&root);
    let (_, second) = two_commits(&root);
    let mut app = open_state_app(&root, &state, Some("a.txt"));
    assert!(settle_history(&mut app, &root.join("a.txt"), 2).await);
    let _term = draw(&mut app);
    enter_commit(&mut app).await;
    // A stat row's diff, and the section collapsed at a dragged height.
    let _term = draw(&mut app);
    let a_row = app.active_commit().unwrap().own_file_row().unwrap();
    doc_start(&mut app);
    for _ in 0..a_row {
        app.press(KeyCode::Down, M::NONE);
    }
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    app.git_history.set_collapsed(true);
    app.git_history.section_height = 7;
    quit(app).await;

    // Reopened: the commit tab and its diff are back, by commit; the section's state too.
    let mut app = open_state_app(&root, &state, None);
    assert!(
        app.settle(|a| !a.session.loading() && a.session.restored == 1).await,
        "the session restored"
    );
    assert!(app.settle(|a| commits_loaded(a) && a.diffs_settled()).await);
    let oid = git2::Oid::from_str(&second).unwrap();
    let commit = app.groups.ids().into_iter().find_map(|g| {
        Some((
            g,
            app.groups
                .get(g)?
                .tabs
                .iter()
                .find_map(|t| tab_commit(&app, t))?
                .commit
                .clone(),
        ))
    });
    assert_eq!(commit.map(|(_, c)| c), Some(second.clone()));
    let pair = app
        .groups
        .ids()
        .into_iter()
        .find_map(|g| app.groups.get(g)?.tabs.iter().find_map(|t| t.diff()).map(|d| d.pair));
    assert_eq!(pair.map(|p| p.right), Some(Text::Commit(oid)));
    assert!(app.git_history.collapsed());
    assert_eq!(app.git_history.section_height, 7);

    // The commit pruned away (a reset and gc): its tab and its diff don't come back.
    git(&root, &["reset", "--hard", "HEAD~1"]);
    git(&root, &["reflog", "expire", "--expire=now", "--all"]);
    git(&root, &["gc", "--prune=now", "--quiet"]);
    quit(app).await;
    let mut app = open_state_app(&root, &state, None);
    assert!(app.settle(|a| !a.session.loading() && a.session.restored == 1).await);
    let tabs: Vec<&Tab> = app
        .groups
        .ids()
        .into_iter()
        .flat_map(|g| app.groups.get(g).unwrap().tabs.iter())
        .collect();
    assert!(tabs.iter().all(|t| tab_commit(&app, t).is_none()), "no commit tab");
    assert!(tabs.iter().all(|t| t.diff().is_none()), "no commit diff tab");
    assert!(tabs.iter().any(|t| t.buffer().is_some()), "the file's tab is back");
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
