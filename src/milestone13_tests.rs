//! Milestone 13 end to end (sessions and recent workspaces). 13a, sessions: quitting writes the
//! workspace's session under the state dir and the next start gives back the same groups, split
//! tree (a dragged ratio too), tabs and active tab per group, active group, every selection with
//! the primary, the scroll, and the side / bottom panels' layout
//! (`milestone_13_quit_and_reopen_restores_the_layout`); git and history diff tabs come back by
//! path and pair, a history diff whose entry is gone doesn't
//! (`milestone_13_diff_tabs_are_restored_by_path_and_pair`); task tabs never come back
//! (`milestone_13_task_tabs_are_not_restored`); a file gone since is skipped (its group closed),
//! selections in a file that shrank are clamped, a corrupt session file restores nothing and
//! is replaced at the next quit (`milestone_13_missing_files_and_bad_sessions_are_skipped`);
//! a file named on the command line opens on top of the restored session, its line winning
//! over the session's cursor (`milestone_13_a_command_line_file_opens_on_top_of_the_session`);
//! quitting before the restore finished keeps the old session
//! (`milestone_13_quitting_while_restoring_keeps_the_session`).

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::Command;
use crate::diff_view::Pair;
use crate::git::test_repo::{commit_all, init};
use crate::group::Tab;
use crate::layout::{GroupId, PanelTab, SidePanel, SplitTree};
use crate::selection::{Selection, Selections};
use crate::session;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

/// `proj/` (the workspace root) and `state/` (sessions, trust store) in a tempdir.
fn dirs(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let base = workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    (root, state)
}

/// The app on `root` with sessions in `state`, started as `main` would with `file` (and its
/// line, column) on the command line; the restore not yet waited for.
fn start(root: &Path, state: &Path, file: Option<(&str, Option<(usize, usize)>)>) -> App {
    let initial = file.map(|(name, goto)| InitialFile {
        buffer: Buffer::open(&root.join(name)).unwrap(),
        goto,
    });
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, initial, true);
    app.state_dir = Some(state.to_path_buf());
    app.test_start();
    app
}

/// `start`, then waits for the session's restore.
async fn reopen(root: &Path, state: &Path, file: Option<(&str, Option<(usize, usize)>)>) -> App {
    let mut app = start(root, state, file);
    assert!(app.settle(|a| !a.session.loading()).await, "the session is read");
    app
}

/// Quits as `run` does: the command, then `shutdown` (which writes the session).
async fn quit(mut app: App) {
    app.test_execute(Command::Quit);
    assert!(app.quitting());
    app.shutdown().await;
}

fn open(app: &mut App, root: &Path, name: &str) {
    app.test_open(Buffer::open(&root.join(name)).unwrap());
}

/// Each group (reading order): its tabs and active index. A text tab is its file name with its
/// selections (anchor, head), primary and scroll; a diff tab its title and pair.
type TabShot = (String, Option<(Vec<(usize, usize)>, usize, usize, usize)>, Option<Pair>);

fn layout(app: &App) -> Vec<(GroupId, Vec<TabShot>, usize)> {
    app.groups
        .ids()
        .into_iter()
        .map(|id| {
            let g = app.groups.get(id).unwrap();
            let tabs = g
                .tabs
                .iter()
                .map(|t| match t {
                    Tab::Text(t) => {
                        let v = &t.view;
                        let sels = v.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect();
                        let name = app.buffers.get(t.buffer).unwrap().name();
                        (
                            name,
                            Some((sels, v.selections.primary_index(), v.top_line(), v.left_col)),
                            None,
                        )
                    }
                    Tab::Diff(d) => (d.title(), None, Some(d.pair)),
                })
                .collect();
            (id, tabs, g.active)
        })
        .collect()
}

fn set_view(app: &mut App, sels: &[(usize, usize)], primary: usize, top: usize, left: usize) {
    let (buf, view) = app.active_mut().unwrap();
    let list = sels
        .iter()
        .map(|&(a, h)| Selection::new(ByteIdx(a), ByteIdx(h)))
        .collect();
    view.selections = Selections::new(list, primary);
    view.set_top_line(&buf.rope, top);
    view.left_col = left;
    view.reveal_cursor = false;
}

fn draw(app: &mut App, term: &mut Terminal<TestBackend>) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i} with some text\n")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_quit_and_reopen_restores_the_layout() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), lines(200)).unwrap();
    fs::write(root.join("b.txt"), lines(50)).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/c.txt"), lines(80)).unwrap();
    let outside = workspace::normalize(dir.path()).join("outside.txt");
    fs::write(&outside, "far away\n").unwrap();

    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    assert_eq!(app.session.restored, 0, "no session yet");
    // Group 1: a.txt (three cursors, the middle one primary, scrolled), b.txt.
    set_view(&mut app, &[(30, 30), (100, 110), (500, 495)], 1, 12, 3);
    open(&mut app, &root, "b.txt");
    set_view(&mut app, &[(40, 44)], 0, 2, 0);
    app.test_execute(Command::PrevTab);
    // Group 2, right: a copy of a.txt's view, then src/c.txt and a file outside the root.
    app.test_execute(Command::SplitRight);
    open(&mut app, &root, "src/c.txt");
    set_view(&mut app, &[(7, 7)], 0, 30, 0);
    app.test_open(Buffer::open(&outside).unwrap());
    app.test_execute(Command::PrevTab);
    // Group 3, below group 2: b.txt.
    app.test_execute(Command::SplitDown);
    app.test_execute(Command::NextTab);

    // A dragged divider, a wider side panel on Search, the bottom panel shown and taller.
    let mut term = Terminal::new(TestBackend::new(140, 45)).unwrap();
    draw(&mut app, &mut term);
    let div = app.hits.dividers[0].rect;
    app.mouse(MouseEventKind::Down(MouseButton::Left), div.x, div.y + 3);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), div.x - 15, div.y + 3);
    app.mouse(MouseEventKind::Up(MouseButton::Left), div.x - 15, div.y + 3);
    app.test_execute(Command::FindInFiles);
    app.ui.side_width = 44;
    app.test_execute(Command::TogglePanel);
    app.ui.panel_height = 15;
    app.test_execute(Command::FocusGroup(2));
    draw(&mut app, &mut term);
    let drawn: Vec<_> = app.hits.dividers.iter().map(|d| d.rect).collect();
    let tree: SplitTree = app.groups.tree.clone();
    let before = layout(&app);
    assert_eq!(before.len(), 3);
    assert_eq!(before[0].1.len(), 2);
    assert_eq!(before[1].1.len(), 3);
    let active = app.groups.active;
    quit(app).await;

    let file = session::file(&state, &root);
    assert!(file.is_file(), "the session is in the state dir: {}", file.display());
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"src/c.txt\""), "paths inside the root are relative");
    assert!(text.contains(&outside.display().to_string()), "others absolute");

    let app = reopen(&root, &state, None).await;
    assert_eq!(app.session.restored, 1);
    assert_eq!(layout(&app), before, "tabs, selections, scroll per group");
    assert_eq!(app.groups.tree, tree, "the split tree and its ratios");
    assert_eq!(app.groups.active, active);
    assert!(app.ui.side_visible);
    assert_eq!((app.ui.side_panel, app.ui.side_width), (SidePanel::Search, 44));
    assert!(app.ui.panel_visible);
    assert_eq!((app.ui.panel_height, app.ui.panel_tab), (15, PanelTab::Problems));
    assert_eq!(app.ui.focus, Focus::Editor);
    // One buffer per file, however many tabs show it.
    assert_eq!(app.buffers.iter().count(), 4);
    // The restored state draws: the side panel's width and both dividers.
    let mut app = app;
    let mut term = Terminal::new(TestBackend::new(140, 45)).unwrap();
    draw(&mut app, &mut term);
    assert_eq!(app.hits.regions.side.unwrap().width, 44);
    assert_eq!(app.hits.dividers.len(), 2);
    let dividers: Vec<_> = app.hits.dividers.iter().map(|d| d.rect).collect();
    assert_eq!(dividers, drawn, "drawn where they were");

    // Hidden side panel and bottom panel come back hidden.
    app.test_execute(Command::ToggleSidebar);
    app.test_execute(Command::TogglePanel);
    quit(app).await;
    let app = reopen(&root, &state, None).await;
    assert!(!app.ui.side_visible && !app.ui.panel_visible);
    assert_eq!(app.ui.focus, Focus::Editor);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_diff_tabs_are_restored_by_path_and_pair() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    init(&root);
    fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    fs::write(root.join("b.txt"), "x\n").unwrap();
    commit_all(&root, "first");
    fs::write(root.join("a.txt"), "one\nTWO\n").unwrap();
    let cfg = crate::workspace::config::HistoryConfig::default();
    let entry = match crate::history::record(&root, Path::new("a.txt"), b"one\n", &cfg).unwrap() {
        crate::history::Recorded::Added(e) => e,
        other => panic!("{other:?}"),
    };
    let kept = Pair::history(entry.snapshot());
    let gone = Pair::history(crate::history::Snapshot { time: 1, hash: 2 });

    let mut app = reopen(&root, &state, Some(("b.txt", None))).await;
    assert!(app.settle(|a| a.git.status.is_some()).await);
    app.open_diff(root.join("a.txt"), Pair::UNSTAGED);
    app.open_diff(root.join("a.txt"), Pair::STAGED);
    app.open_diff(root.join("a.txt"), kept);
    app.open_diff(root.join("a.txt"), gone);
    app.open_diff(root.join("a.txt"), Pair::SAVED);
    app.test_execute(Command::PrevTab);
    app.test_execute(Command::PrevTab);
    let loaded = |a: &App| {
        a.groups
            .ids()
            .iter()
            .flat_map(|&g| &a.groups.get(g).unwrap().tabs)
            .all(|t| {
                t.diff()
                    .is_none_or(|d| !d.loading() && (d.doc.is_some() || d.error.is_some()))
            })
    };
    assert!(app.settle(loaded).await);
    let failed: Vec<_> = app
        .group()
        .tabs
        .iter()
        .filter_map(|t| t.diff()?.error.as_ref())
        .collect();
    assert_eq!(failed.len(), 1, "the missing entry's diff fails: {failed:?}");
    let pairs = |app: &App| -> Vec<Option<Pair>> { layout(app)[0].1.iter().map(|t| t.2).collect() };
    assert_eq!(
        pairs(&app),
        [
            None,
            Some(Pair::UNSTAGED),
            Some(Pair::STAGED),
            Some(kept),
            Some(gone),
            Some(Pair::SAVED)
        ]
    );
    quit(app).await;

    let app = reopen(&root, &state, None).await;
    let shot = layout(&app);
    assert_eq!(
        pairs(&app),
        [
            None,
            Some(Pair::UNSTAGED),
            Some(Pair::STAGED),
            Some(kept),
            Some(Pair::SAVED)
        ],
        "the history diff of a missing entry is dropped"
    );
    assert_eq!(shot[0].2, 3, "the active tab (the kept history diff)");
    assert!(app.group().active_diff().is_some());
    let mut app = app;
    assert!(app.settle(|a| a.diffs_settled()).await, "the restored diffs load");
    for id in app.groups.ids() {
        for t in &app.groups.get(id).unwrap().tabs {
            if let Some(d) = t.diff() {
                assert!(d.error.is_none(), "{}: {:?}", d.title(), d.error);
                assert!(d.doc.is_some());
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_task_tabs_are_not_restored() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let tasks_toml = state.join(crate::tasks::templates::FILE_NAME);
    fs::write(&tasks_toml, "[tasks.hello]\ncommand = \"echo hello\"\n").unwrap();

    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(state.join("trust.toml"));
    let buffer = Buffer::open(&root.join("a.txt")).unwrap();
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.state_dir = Some(state.clone());
    app.tasks.user = crate::tasks::templates::load(&tasks_toml).tasks;
    app.test_start();
    assert!(app.settle(|a| !a.session.loading()).await);
    let id = app.run_task("hello").unwrap();
    assert!(
        app.settle(|a| a.tasks.get(id).is_some_and(|r| !r.status().is_running()))
            .await
    );
    assert_eq!(app.ui.panel_tab, PanelTab::Task(id));
    assert!(app.ui.panel_visible);
    quit(app).await;

    let text = fs::read_to_string(session::file(&state, &root)).unwrap();
    assert!(!text.contains("hello"), "{text}");
    let app = reopen(&root, &state, None).await;
    assert!(app.tasks.runs().is_empty());
    assert!(app.ui.panel_visible, "the panel's layout is kept");
    assert_eq!(app.ui.panel_tab, PanelTab::Problems, "on Problems, not the task");
    assert_eq!(layout(&app)[0].1.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_missing_files_and_bad_sessions_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), lines(100)).unwrap();
    fs::write(root.join("b.txt"), lines(10)).unwrap();
    fs::write(root.join("c.txt"), lines(10)).unwrap();

    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    set_view(&mut app, &[(5, 5), (1500, 1510)], 1, 60, 0);
    open(&mut app, &root, "c.txt");
    app.test_execute(Command::SplitRight);
    open(&mut app, &root, "b.txt");
    // The right group shows c.txt (the split's copy) and b.txt.
    app.test_execute(Command::SplitDown);
    // The bottom right group: only b.txt.
    app.test_execute(Command::FocusGroup(2));
    app.test_execute(Command::CloseTab);
    // Group 2 now has only c.txt... and group 3 only b.txt.
    let before = layout(&app);
    assert_eq!(before.len(), 3);
    assert_eq!(before[2].1.len(), 1);
    quit(app).await;

    // b.txt goes, c.txt is deleted too, a.txt shrinks to 3 lines.
    fs::remove_file(root.join("b.txt")).unwrap();
    fs::remove_file(root.join("c.txt")).unwrap();
    fs::write(root.join("a.txt"), "x\ny\nz\n").unwrap();
    let app = reopen(&root, &state, None).await;
    let after = layout(&app);
    assert_eq!(after.len(), 1, "groups left without tabs close: {after:?}");
    assert_eq!(after[0].1.len(), 1, "a.txt only (c.txt's tab dropped)");
    let (name, view, _) = &after[0].1[0];
    assert_eq!(name, "a.txt");
    let (sels, _, top, _) = view.as_ref().unwrap();
    assert_eq!(sels, &[(5, 5), (6, 6)], "clamped to the shorter text, merged");
    assert_eq!(*top, 3, "the scroll clamped to the last line");
    assert_eq!(app.buffers.iter().count(), 1, "no buffer kept for dropped tabs");
    quit(app).await;

    // A corrupt session file: nothing restored, the start goes on, the next quit replaces it.
    let file = session::file(&state, &root);
    fs::write(&file, "{\"version\": 1, \"root\": oops").unwrap();
    let mut app = reopen(&root, &state, None).await;
    assert_eq!(app.session.restored, 0);
    assert_eq!(layout(&app)[0].1.len(), 0);
    assert_eq!(app.ui.focus, Focus::SidePanel, "as a start without a session");
    open(&mut app, &root, "a.txt");
    quit(app).await;
    assert!(session::load(&state, &root).unwrap().is_some());
    let app = reopen(&root, &state, None).await;
    assert_eq!(layout(&app)[0].1[0].0, "a.txt");

    // A session whose split tree doesn't match its groups: nothing restored.
    let mut s = session::load(&state, &root).unwrap().unwrap();
    s.groups[0].id = 42;
    session::save(&state, &s).unwrap();
    drop(app);
    let app = reopen(&root, &state, None).await;
    assert_eq!(app.session.restored, 0);
    assert!(app.group().tabs.is_empty());
    assert_eq!(app.buffers.iter().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_a_command_line_file_opens_on_top_of_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(root.join(name), lines(20)).unwrap();
    }
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    set_view(&mut app, &[(50, 50)], 0, 0, 0);
    open(&mut app, &root, "b.txt");
    app.test_execute(Command::SplitRight);
    app.test_execute(Command::FocusGroup(1));
    quit(app).await;

    // A file not in the session: a new tab right of the active group's active tab.
    let app = reopen(&root, &state, Some(("c.txt", None))).await;
    let shot = layout(&app);
    assert_eq!(shot.len(), 2);
    let names: Vec<_> = shot[0].1.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
    assert_eq!(shot[0].2, 2, "and active");
    assert_eq!(app.groups.active, shot[0].0);
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(app.active_buffer().unwrap().name(), "c.txt");
    drop(app);

    // A file in the session: its tab, activated, with the session's cursor…
    let app = reopen(&root, &state, Some(("a.txt", None))).await;
    let shot = layout(&app);
    let names: Vec<_> = shot[0].1.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(names, ["a.txt", "b.txt"], "no second tab");
    assert_eq!(shot[0].2, 0);
    assert_eq!(shot[0].1[0].1.as_ref().unwrap().0, [(50, 50)]);
    assert_eq!(app.buffers.iter().count(), 2, "one buffer for a.txt");
    drop(app);

    // …unless the command line names a line.
    let app = reopen(&root, &state, Some(("a.txt", Some((3, 1))))).await;
    let shot = layout(&app);
    assert_eq!(shot[0].1.len(), 2);
    assert_eq!(shot[0].2, 0);
    let line3 = lines(2).len();
    assert_eq!(shot[0].1[0].1.as_ref().unwrap().0, [(line3, line3)]);
    // The other group's copy keeps the session's.
    assert_eq!(shot[1].1[0].0, "b.txt");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_quitting_while_restoring_keeps_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    open(&mut app, &root, "b.txt");
    quit(app).await;
    let file = session::file(&state, &root);
    let saved = fs::read(&file).unwrap();

    // Quit before the restore's read is handled: the session file is left as it was.
    let app = start(&root, &state, None);
    assert!(app.session.loading());
    quit(app).await;
    assert_eq!(fs::read(&file).unwrap(), saved);
    let app = reopen(&root, &state, None).await;
    assert_eq!(layout(&app)[0].1.len(), 2);

    // No state dir (as in other tests): nothing is read or written.
    let ws = Workspace::load(root.clone());
    let mut app = App::new(ws, TrustStore::load(state.join("trust.toml")), None, true);
    app.test_start();
    assert!(!app.session.loading());
    let buffer = Buffer::open(&root.join("b.txt")).unwrap();
    app.test_open(buffer);
    quit(app).await;
    assert_eq!(fs::read(&file).unwrap(), saved);
    let app = reopen(&root, &state, None).await;
    assert_eq!(layout(&app)[0].1.len(), 2, "unchanged");
}
