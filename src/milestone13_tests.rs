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
//!
//! 13b, crash recovery: dirty buffers are backed up each interval (only those changed since
//! their backup), a killed jed's next start offers them, Restore puts each back into its buffer
//! (opening the file if needed) as one undo step, dirty, nothing written; reverting to clean and
//! saving remove the backup (`milestone_13_a_killed_jed_offers_its_unsaved_changes_back`); a
//! clean quit leaves none, the terminal going away writes them, Later keeps them for the next
//! start and Discard removes them (`milestone_13_quitting_cleanly_leaves_no_backups`); corrupt
//! backups and another workspace's aren't offered
//! (`milestone_13_corrupt_and_other_workspaces_backups_are_not_offered`); a backup of a file
//! changed on disk since restores with the banner and a save that won't overwrite it unasked
//! (`milestone_13_a_backup_of_a_file_changed_since_shows_the_banner`); the offer waits for the
//! trust dialog (`milestone_13_the_backup_offer_waits_for_the_trust_dialog`).
//! The deadline is set by the first edit and not put off by more typing, so killing the
//! terminal loses at most one interval (`milestone_13_typing_does_not_put_the_backup_off`); the
//! panic hook writes the same files the next start offers
//! (`milestone_13_a_panic_backs_up_what_the_next_start_offers`); closing a dirty tab with Don't
//! Save removes its backup and an explorer rename moves it to the new name
//! (`milestone_13_closing_or_renaming_a_dirty_buffer_moves_its_backup`); a backup found but not
//! yet offered is neither replaced nor removed at shutdown
//! (`milestone_13_a_backup_not_offered_yet_is_never_replaced`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus, InitialFile, Prompt};
use crate::backup;
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

// 13b: crash recovery.

fn buf_of<'a>(app: &'a App, root: &Path, name: &str) -> Option<&'a Buffer> {
    app.buffers.get(app.buffers.find_path(&root.join(name))?)
}

fn text_of(app: &App, root: &Path, name: &str) -> String {
    buf_of(app, root, name).unwrap().rope.to_string()
}

/// Fires the backup timer as if `backup::INTERVAL` had passed and waits for its job.
async fn back_up(app: &mut App) {
    app.test_backup_tick(backup::INTERVAL);
    assert!(app.settle(|a| !a.backups.busy()).await, "the backup job ends");
}

/// Waits for the start's offer to restore backups.
async fn offered(app: &mut App) -> Vec<PathBuf> {
    let shown = app.settle(|a| matches!(a.prompt, Some(Prompt::RestoreBackups))).await;
    assert!(shown, "the restore prompt");
    app.backups.offered.iter().map(|b| b.path.clone()).collect()
}

fn backup_file(state: &Path, root: &Path, name: &str) -> PathBuf {
    backup::file(state, root, &root.join(name))
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_a_killed_jed_offers_its_unsaved_changes_back() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(root.join("b.txt"), "bee\n").unwrap();
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    app.type_text("X");
    open(&mut app, &root, "b.txt");
    app.type_text("B");
    back_up(&mut app).await;
    let (a_file, b_file) = (backup_file(&state, &root, "a.txt"), backup_file(&state, &root, "b.txt"));
    let first_b = backup::read(&state, &root, &b_file).unwrap();
    assert_eq!(first_b.text.to_string(), "Bbee\n");

    // Only buffers changed since their backup are written again.
    std::thread::sleep(Duration::from_millis(5));
    app.test_execute(Command::PrevTab);
    app.type_text("Z");
    back_up(&mut app).await;
    let a = backup::read(&state, &root, &a_file).unwrap();
    assert_eq!(a.text.to_string(), "XZone\ntwo\nthree\n");
    assert!(a.time > first_b.time, "a.txt rewritten");
    assert_eq!(
        backup::read(&state, &root, &b_file).unwrap().time,
        first_b.time,
        "b.txt not"
    );

    // Edits after the last backup are what a kill loses (at most one interval).
    app.type_text("Y");
    drop(app);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "one\ntwo\nthree\n");

    // The next start (a.txt open from the command line, b.txt not open) offers both.
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    assert_eq!(offered(&mut app).await, [root.join("a.txt"), root.join("b.txt")]);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut term);
    let screen: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(screen.contains("Unsaved changes to 2 files were backed up"), "{screen}");
    assert!(screen.contains("[R]estore"));
    app.press(KeyCode::Char('r'), KeyModifiers::NONE);
    assert!(app.prompt.is_none());
    let restored = app
        .settle(|a| a.backups.restoring() == 0 && buf_of(a, &root, "b.txt").is_some_and(|b| b.is_dirty()))
        .await;
    assert!(restored, "both restored");
    assert_eq!(text_of(&app, &root, "a.txt"), "XZone\ntwo\nthree\n");
    assert_eq!(text_of(&app, &root, "b.txt"), "Bbee\n");
    assert!(buf_of(&app, &root, "a.txt").unwrap().is_dirty());
    assert_eq!(app.group().tabs.len(), 2, "b.txt opened in a tab");
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .contains("Restored unsaved changes to a.txt, b.txt")
    );
    // Nothing written.
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "bee\n");

    // One undo step brings the file's text back, clean: its backup goes.
    let a = app.buffers.find_path(&root.join("a.txt")).unwrap();
    app.buffers.get_mut(a).unwrap().undo();
    assert_eq!(text_of(&app, &root, "a.txt"), "one\ntwo\nthree\n");
    assert!(!buf_of(&app, &root, "a.txt").unwrap().is_dirty());
    assert!(
        app.settle(|_| !a_file.exists()).await,
        "reverted to clean: backup removed"
    );

    // Saving removes the backup.
    assert_eq!(app.active_buffer().unwrap().name(), "b.txt");
    app.test_execute(Command::Save);
    assert!(app.settle(|_| !b_file.exists()).await, "saved: backup removed");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "Bbee\n");
    quit(app).await;
    let mut app = reopen(&root, &state, None).await;
    assert!(app.settle(|a| !a.backups.busy()).await);
    assert!(app.prompt.is_none(), "nothing left to offer");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_quitting_cleanly_leaves_no_backups() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let a_file = backup_file(&state, &root, "a.txt");

    // Quit with Don't Save: the backups go with the edits.
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    app.type_text("1");
    back_up(&mut app).await;
    assert!(a_file.exists());
    app.test_execute(Command::Quit);
    app.press(KeyCode::Char('d'), KeyModifiers::NONE);
    assert!(app.quitting());
    app.shutdown().await;
    assert!(!backup::dir(&state, &root).exists(), "no backups after a clean quit");

    // The terminal going away (no Quit) brings the backups up to date instead.
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    app.type_text("2");
    app.shutdown().await;
    let b = backup::read(&state, &root, &a_file).unwrap();
    assert_eq!(b.text.to_string(), "a2\n", "typed at the session's cursor");

    // Later (Esc) keeps them, past a clean quit, offered again at the next start…
    let mut app = reopen(&root, &state, None).await;
    assert_eq!(offered(&mut app).await, [root.join("a.txt")]);
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.prompt.is_none());
    assert_eq!(
        text_of(&app, &root, "a.txt"),
        "a\n",
        "nothing restored (the session's tab)"
    );
    quit(app).await;
    assert!(a_file.exists(), "kept");

    // …where Discard removes them.
    let mut app = reopen(&root, &state, None).await;
    offered(&mut app).await;
    app.press(KeyCode::Char('d'), KeyModifiers::NONE);
    assert!(app.settle(|_| !a_file.exists()).await, "discarded");
    assert!(!buf_of(&app, &root, "a.txt").unwrap().is_dirty());
    drop(app);
    let mut app = reopen(&root, &state, None).await;
    assert!(app.settle(|a| !a.backups.busy()).await);
    assert!(app.prompt.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_corrupt_and_other_workspaces_backups_are_not_offered() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let snap = |path: PathBuf, text: &str| backup::Snapshot {
        path,
        rope: ropey::Rope::from_str(text),
        bom: false,
        disk: None,
    };
    let other = root.parent().unwrap().join("other");
    backup::write(&state, &other, &snap(other.join("a.txt"), "theirs"), 1).unwrap();
    backup::write(&state, &root, &snap(root.join("a.txt"), "mine\n"), 1).unwrap();
    // Corrupt: cut short, and garbage.
    let cut = backup::write(&state, &root, &snap(root.join("b.txt"), "bbbb"), 1).unwrap();
    let bytes = fs::read(&cut).unwrap();
    fs::write(&cut, &bytes[..bytes.len() - 2]).unwrap();
    fs::write(backup_file(&state, &root, "c.txt"), "not a backup").unwrap();

    let mut app = reopen(&root, &state, None).await;
    assert_eq!(
        offered(&mut app).await,
        [root.join("a.txt")],
        "only the good one of this workspace"
    );
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        app.settle(|a| buf_of(a, &root, "a.txt").is_some_and(|b| b.is_dirty()))
            .await
    );
    assert_eq!(text_of(&app, &root, "a.txt"), "mine\n");
    // The other workspace's backup is left alone, even by a clean quit.
    app.test_execute(Command::Quit);
    app.press(KeyCode::Char('d'), KeyModifiers::NONE);
    app.shutdown().await;
    assert!(!backup::dir(&state, &root).exists(), "the corrupt ones went too");
    assert!(backup::file(&state, &other, &other.join("a.txt")).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_a_backup_of_a_file_changed_since_shows_the_banner() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "base\n").unwrap();
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    app.type_text("mine ");
    back_up(&mut app).await;
    drop(app);
    // Someone else changes the file before jed starts again.
    fs::write(root.join("a.txt"), "theirs\n").unwrap();

    let mut app = reopen(&root, &state, None).await;
    offered(&mut app).await;
    app.press(KeyCode::Char('r'), KeyModifiers::NONE);
    let banner = app
        .settle(|a| buf_of(a, &root, "a.txt").is_some_and(|b| b.disk_conflict.is_some()))
        .await;
    assert!(banner, "the changed-on-disk banner");
    assert_eq!(text_of(&app, &root, "a.txt"), "mine base\n");
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .contains("changed on disk since: a.txt")
    );
    // A save doesn't overwrite the newer file unasked…
    app.test_execute(Command::Save);
    assert!(app.settle(|a| !a.buffers.any_saving()).await);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "theirs\n");
    assert!(buf_of(&app, &root, "a.txt").unwrap().is_dirty());
    // …Keep Mine does.
    app.test_execute(Command::KeepMine);
    app.test_execute(Command::Save);
    assert!(
        app.settle(|a| !a.buffers.any_saving() && !a.active_buffer().unwrap().is_dirty())
            .await
    );
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "mine base\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_the_backup_offer_waits_for_the_trust_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join(".workspace"), "[tasks.build]\ncommand = \"make\"\n").unwrap();
    let snap = backup::Snapshot {
        path: root.join("a.txt"),
        rope: ropey::Rope::from_str("mine\n"),
        bom: false,
        disk: None,
    };
    backup::write(&state, &root, &snap, 1).unwrap();
    let mut app = reopen(&root, &state, None).await;
    assert!(app.settle(|a| !a.backups.busy()).await);
    assert!(matches!(app.prompt, Some(Prompt::Trust { .. })), "trust first");
    app.press(KeyCode::Char('r'), KeyModifiers::NONE);
    assert!(matches!(app.prompt, Some(Prompt::RestoreBackups)), "then the offer");
}

fn snap(path: PathBuf, text: &str) -> backup::Snapshot {
    backup::Snapshot {
        path,
        rope: ropey::Rope::from_str(text),
        bom: false,
        disk: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_typing_does_not_put_the_backup_off() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let a_file = backup_file(&state, &root, "a.txt");
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    assert!(app.settle(|a| !a.backups.busy()).await);
    app.pump();
    assert_eq!(app.backups.deadline(), None, "nothing dirty, nothing due");

    let before = Instant::now();
    app.type_text("1");
    app.pump();
    let first = app.backups.deadline().expect("the first edit sets the deadline");
    assert!(first >= before + backup::INTERVAL && first <= Instant::now() + backup::INTERVAL);
    // A constant typist is still backed up: more edits don't move the deadline.
    for c in ["2", "3"] {
        std::thread::sleep(Duration::from_millis(5));
        app.type_text(c);
        app.pump();
        assert_eq!(app.backups.deadline(), Some(first), "not put off by typing");
    }
    // Before the deadline nothing is written…
    app.test_backup_tick(backup::INTERVAL - Duration::from_secs(1));
    assert!(!app.backups.busy() && !a_file.exists());
    // …at it, the buffer as it is then.
    back_up(&mut app).await;
    assert_eq!(backup::read(&state, &root, &a_file).unwrap().text.to_string(), "123a\n");
    assert_eq!(app.backups.deadline(), None, "nothing changed since");
    app.type_text("4");
    app.pump();
    assert!(
        app.backups.deadline().is_some(),
        "the next edit starts the next interval"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_a_panic_backs_up_what_the_next_start_offers() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    fs::write(root.join("c.txt"), "c\n").unwrap();
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    app.type_text("P");
    open(&mut app, &root, "b.txt");
    app.type_text("Q");
    open(&mut app, &root, "c.txt");
    // No periodic backup has run: the hook writes them itself, clean buffers left out.
    assert!(!backup::dir(&state, &root).exists());
    let mut written = app.test_panic_backup();
    written.sort();
    let mut expected = vec![backup_file(&state, &root, "a.txt"), backup_file(&state, &root, "b.txt")];
    expected.sort();
    assert_eq!(written, expected);
    drop(app);

    let mut app = reopen(&root, &state, None).await;
    assert_eq!(offered(&mut app).await, [root.join("a.txt"), root.join("b.txt")]);
    app.press(KeyCode::Char('r'), KeyModifiers::NONE);
    let restored = app
        .settle(|a| a.backups.restoring() == 0 && buf_of(a, &root, "b.txt").is_some_and(|b| b.is_dirty()))
        .await;
    assert!(restored);
    assert_eq!(text_of(&app, &root, "a.txt"), "Pa\n");
    assert_eq!(text_of(&app, &root, "b.txt"), "Qb\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_closing_or_renaming_a_dirty_buffer_moves_its_backup() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    let (a_file, b_file, c_file) = (
        backup_file(&state, &root, "a.txt"),
        backup_file(&state, &root, "b.txt"),
        backup_file(&state, &root, "c.txt"),
    );
    let mut app = reopen(&root, &state, Some(("a.txt", None))).await;
    app.type_text("A");
    open(&mut app, &root, "b.txt");
    app.type_text("B");
    back_up(&mut app).await;
    assert!(a_file.exists() && b_file.exists());

    // Closing b.txt with Don't Save discards its edits and its backup.
    app.test_execute(Command::CloseTab);
    assert!(matches!(app.prompt, Some(Prompt::SaveBeforeClose { .. })));
    app.press(KeyCode::Char('d'), KeyModifiers::NONE);
    assert!(buf_of(&app, &root, "b.txt").is_none());
    assert!(app.settle(|_| !b_file.exists()).await, "discarded: backup removed");
    assert!(a_file.exists());

    // Renaming a.txt in the explorer: the old name's backup goes, the new name's comes with the
    // next backup.
    let cmd = if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    };
    app.press(KeyCode::Char('e'), cmd | KeyModifiers::SHIFT);
    let a = root.join("a.txt");
    assert!(app.settle(|app| app.explorer.find(&a).is_some()).await, "root listed");
    app.explorer.selected = Some(a.clone());
    app.press(KeyCode::F(2), KeyModifiers::NONE);
    for _ in 0.."a.txt".len() {
        app.press(KeyCode::Backspace, KeyModifiers::NONE);
    }
    app.type_text("c.txt");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| buf_of(a, &root, "c.txt").is_some()).await, "renamed");
    assert!(app.settle(|_| !a_file.exists()).await, "the old name's backup removed");
    back_up(&mut app).await;
    let c = backup::read(&state, &root, &c_file).unwrap();
    assert_eq!((c.path, c.text.to_string()), (root.join("c.txt"), "Aa\n".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_a_backup_not_offered_yet_is_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let a_file = backup_file(&state, &root, "a.txt");
    backup::write(&state, &root, &snap(root.join("a.txt"), "mine\n"), 1).unwrap();
    // The start's scan hasn't been handled (no `settle`): its backups aren't offered yet.
    let mut app = start(&root, &state, Some(("a.txt", None)));
    assert!(app.backups.busy(), "scanning");
    app.type_text("Z");
    // The terminal goes away: the edit isn't written over the backup still to be offered.
    app.shutdown().await;
    drop(app);
    assert_eq!(backup::read(&state, &root, &a_file).unwrap().text.to_string(), "mine\n");
    // Nor removed by a clean quit.
    quit(start(&root, &state, Some(("a.txt", None)))).await;
    assert_eq!(backup::read(&state, &root, &a_file).unwrap().text.to_string(), "mine\n");
    let mut app = reopen(&root, &state, None).await;
    assert_eq!(offered(&mut app).await, [root.join("a.txt")]);
}
