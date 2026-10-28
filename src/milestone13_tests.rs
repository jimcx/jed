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
//!
//! 13c, recent workspaces: every start records its root in `recent.toml`, most recent first, no
//! duplicates, a corrupt list replaced (`milestone_13_each_start_records_the_workspace_most_recent_first`);
//! Ctrl+R lists the others in the palette — folder name, then the path with `~` for home — the
//! current workspace and folders gone since left out (and pruned from the file), filtered by
//! typing; picking one leaves jed on its workspace and says how to open it, `jed <path>`
//! shell-quoted when needed (`milestone_13_ctrl_r_picks_a_recent_workspace`); the command is in
//! the palette with its key (`milestone_13_open_recent_workspace_is_in_the_palette`); with no
//! other workspace, or no state dir, a message instead
//! (`milestone_13_no_other_recent_workspace_says_so`).
//!
//! 13d, the done when (minus the kitty clause, deferred to the TODO list), on one git project
//! driven through keys: quit and reopen gives back the same frame cell for cell — three groups
//! with a diff tab and both dividers dragged, several cursors and selections (one primary not
//! the first), scrolled views, the Source Control view and the bottom panel, every tab of the
//! first group too, and again after a second round trip
//! (`milestone_13_done_when_quit_and_reopen_gives_back_the_same_screen`); killing the terminal
//! (the app dropped without Quit or shutdown) after a backup interval and more edits gives back
//! the session's tabs and, on Restore, exactly the text of the last backup — only the edits
//! after it are lost — and one undo per file brings back the disk's text
//! (`milestone_13_done_when_a_kill_loses_at_most_one_backup_interval`). The panic hook's path
//! is 13b's `milestone_13_a_panic_backs_up_what_the_next_start_offers`.

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
use crate::layout::{GroupId, PanelTab, SidePanel, SplitDir, SplitTree};
use crate::palette::Mode;
use crate::recent;
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
    app.test_plain_whitespace();
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
                    Tab::Hex(h) => (format!("hex:{}", h.name()), None, None),
                    Tab::Shell(s) => (format!("shell:{}", s.title), None, None),
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
    app.test_plain_whitespace();
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
    app.test_plain_whitespace();
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
        untitled: None,
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
        untitled: None,
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
        untitled: None,
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
    // An untitled buffer with text is backed up too (by its id).
    app.test_execute(Command::NewUntitledFile);
    app.type_text("scratch");
    let uid = app.active_buffer().unwrap().untitled_id().unwrap().to_string();
    // No periodic backup has run: the hook writes them itself, clean buffers left out.
    assert!(!backup::dir(&state, &root).exists());
    let mut written = app.test_panic_backup();
    written.sort();
    let mut expected = vec![
        backup_file(&state, &root, "a.txt"),
        backup_file(&state, &root, "b.txt"),
        backup::key_file(&state, &root, &backup::Key::Untitled(uid.clone())),
    ];
    expected.sort();
    assert_eq!(written, expected);
    drop(app);

    let mut app = reopen(&root, &state, None).await;
    assert_eq!(
        offered(&mut app).await,
        [PathBuf::new(), root.join("a.txt"), root.join("b.txt")]
    );
    app.press(KeyCode::Char('r'), KeyModifiers::NONE);
    let restored = app
        .settle(|a| a.backups.restoring() == 0 && buf_of(a, &root, "b.txt").is_some_and(|b| b.is_dirty()))
        .await;
    assert!(restored);
    assert_eq!(text_of(&app, &root, "a.txt"), "Pa\n");
    assert_eq!(text_of(&app, &root, "b.txt"), "Qb\n");
    assert!(
        app.buffers
            .iter()
            .any(|(_, b)| b.untitled_id() == Some(&uid) && b.rope == "scratch"),
        "the untitled text came back as an untitled buffer"
    );
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

// 13c: recent workspaces.

/// Starts jed on `root` with `state` as its state dir and waits for the root to be recorded.
async fn started(root: &Path, state: &Path) -> App {
    let mut app = start(root, state, None);
    assert!(app.settle(|a| a.recent.recorded == 1).await, "recorded");
    app
}

fn recent_on_disk(state: &Path) -> Vec<PathBuf> {
    recent::parse(&fs::read_to_string(recent::file(state)).unwrap()).unwrap()
}

/// Ctrl+R, then waits for the listing: the picker's rows (folder name, path), or none.
async fn ctrl_r(app: &mut App) -> Option<Vec<(String, String)>> {
    app.press(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app.settle(|a| a.recent.listing_running == 0).await, "listed");
    let p = app.palette.as_ref()?;
    assert_eq!(p.mode(), Mode::Recent);
    Some(p.rows.iter().map(|r| (r.label.clone(), r.detail.clone())).collect())
}

fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_each_start_records_the_workspace_most_recent_first() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let other = root.parent().unwrap().join("other");
    fs::create_dir_all(&other).unwrap();
    for r in [&root, &other, &root] {
        quit(started(r, &state).await).await;
    }
    assert_eq!(recent_on_disk(&state), [root.clone(), other.clone()], "no duplicates");
    // A corrupt list is replaced by the next start.
    fs::write(recent::file(&state), "[[workspace]\npath = 1").unwrap();
    let app = started(&other, &state).await;
    assert_eq!(recent_on_disk(&state), [other]);
    quit(app).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_ctrl_r_picks_a_recent_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let base = root.parent().unwrap().to_path_buf();
    let (gone, second, spaced) = (base.join("gone"), base.join("second"), base.join("other one"));
    for r in [&gone, &second, &spaced] {
        fs::create_dir_all(r).unwrap();
        quit(started(r, &state).await).await;
    }
    fs::remove_dir(&gone).unwrap();
    let mut app = started(&root, &state).await;
    let show = |p: &Path| recent::display(p, &crate::paths::home());
    // Most recent first; the current workspace and the folder gone since are left out.
    let rows = ctrl_r(&mut app).await.expect("the picker opens");
    assert_eq!(
        rows,
        [
            ("other one".to_string(), show(&spaced)),
            ("second".to_string(), show(&second))
        ]
    );
    assert_eq!(
        recent_on_disk(&state),
        [root.clone(), spaced.clone(), second.clone()],
        "pruned"
    );
    // Drawn: the folder name, then its path (its start: a long temp path, as macOS's, is cut).
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw(&mut app, &mut term);
    let screen: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    let path_start: String = show(&spaced).chars().take(30).collect();
    assert!(screen.contains(&format!("other one  {path_start}")), "{screen}");
    // Fuzzy: type to filter, Enter picks; jed stays on its workspace and says how to open it.
    app.type_text("scnd");
    // The best match first ("other one" may match too, by its path: macOS's temp paths are long
    // and random).
    assert_eq!(app.palette.as_ref().unwrap().rows[0].label, "second");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.palette.is_none() && app.recent.picker.is_none());
    let message = app.message.as_ref().unwrap().text.clone();
    assert_eq!(
        message,
        format!("To open {}, run: jed {}", show(&second), second.display())
    );
    assert_eq!(app.workspace.root, root);
    // A path the shell would split is quoted.
    ctrl_r(&mut app).await.unwrap();
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    let message = app.message.as_ref().unwrap().text.clone();
    assert!(
        message.ends_with(&format!("run: jed '{}'", spaced.display())),
        "{message}"
    );
    // Esc closes it and drops the entries.
    ctrl_r(&mut app).await.unwrap();
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.palette.is_none() && app.recent.picker.is_none());
    quit(app).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_open_recent_workspace_is_in_the_palette() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let other = root.parent().unwrap().join("other");
    fs::create_dir_all(&other).unwrap();
    quit(started(&other, &state).await).await;
    let mut app = started(&root, &state).await;
    app.press(KeyCode::Char('p'), cmd() | KeyModifiers::SHIFT);
    app.type_text("Open Recent Workspace");
    let p = app.palette.as_ref().unwrap();
    let row = &p.rows[p.selected];
    assert_eq!(
        (row.label.as_str(), row.keys.as_deref()),
        ("Workspaces: Open Recent Workspace", Some("Ctrl+R"))
    );
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| a.recent.listing_running == 0).await);
    let p = app.palette.as_ref().expect("the picker opens");
    assert_eq!(p.mode(), Mode::Recent);
    assert_eq!(p.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["other"]);
    quit(app).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_no_other_recent_workspace_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let gone = root.parent().unwrap().join("gone");
    fs::create_dir_all(&gone).unwrap();
    quit(started(&gone, &state).await).await;
    fs::remove_dir(&gone).unwrap();
    // Only the current workspace and a folder gone since: no picker, a message.
    let mut app = started(&root, &state).await;
    assert_eq!(ctrl_r(&mut app).await, None);
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.starts_with("No other recent workspaces"), "{message}");
    assert_eq!(recent_on_disk(&state), std::slice::from_ref(&root));
    quit(app).await;
    // No state dir (tests' default): nothing recorded, and Ctrl+R says why.
    let ws = Workspace::load(root.clone());
    let mut app = App::new(ws, TrustStore::load(state.join("trust.toml")), None, true);
    app.test_start();
    app.test_plain_whitespace();
    assert!(app.settle(|a| !a.session.loading()).await);
    assert_eq!(app.recent.recorded, 0);
    assert_eq!(ctrl_r(&mut app).await, None);
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("no state directory"), "{message}");
}

// 13d: the done-when tests, end to end on one workspace, through keys and real files.

/// A small Rust project in a git repository: `src/main.rs` (300 lines), `src/lib.rs` (changed
/// on disk since the commit, so it has a diff), `README.md`, `Cargo.toml`.
fn project(root: &Path) {
    init(root);
    fs::create_dir_all(root.join("src")).unwrap();
    let main: String = (0..300)
        .map(|i| format!("    let value_{i} = compute({i}, \"item {i}\"); // step {i}\n"))
        .collect();
    fs::write(root.join("src/main.rs"), format!("fn main() {{\n{main}}}\n")).unwrap();
    let lib: String = (0..120)
        .map(|i| format!("pub fn f{i}(x: u32) -> u32 {{\n    x + {i}\n}}\n"))
        .collect();
    fs::write(root.join("src/lib.rs"), &lib).unwrap();
    let readme: String = (0..60).map(|i| format!("Paragraph {i} of the readme.\n\n")).collect();
    fs::write(root.join("README.md"), format!("# demo\n\n{readme}")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    commit_all(root, "first");
    fs::write(root.join("src/lib.rs"), lib.replace("x + 7\n", "x * 7 // changed\n")).unwrap();
}

/// Quick open (Cmd+P): types `name`, waits for it to be the first row, Enter, waits for the open.
async fn quick_open(app: &mut App, name: &str) {
    app.press(KeyCode::Char('p'), cmd());
    app.type_text(name);
    let found = app
        .settle(|a| {
            a.palette
                .as_ref()
                .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == name))
        })
        .await;
    assert!(found, "quick open lists {name}");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.palette.is_none());
    let opened = app
        .settle(|a| a.active_buffer().is_some_and(|b| b.name() == name))
        .await;
    assert!(opened, "{name} opened");
}

/// Runs the palette command titled `title` (Cmd+Shift+P).
fn palette_command(app: &mut App, title: &str) {
    app.press(KeyCode::Char('p'), cmd() | KeyModifiers::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
}

/// Everything a frame shows has arrived: the session, the git status, every diff tab, the gutter
/// markers and each buffer's parse tree.
fn screen_settled(a: &App) -> bool {
    !a.session.loading()
        && a.git.status.is_some()
        && !a.git.running()
        && a.diffs_settled()
        && a.buffers.iter().all(|(_, b)| {
            b.syntax.tree().is_some()
                && !b.quick_diff.running()
                && b.quick_diff.want(b.revision()) == crate::quick_diff::Want::Nothing
        })
}

/// A drawn frame: every row's text and every cell's style outside the side panel, the side
/// panel's rows without the Source Control view's `+` / `−` buttons (drawn on the selected or
/// hovered row: the list's selection isn't part of the session), and the terminal cursor.
#[derive(Debug, PartialEq)]
struct Frame {
    rows: Vec<String>,
    styles: Vec<ratatui::style::Style>,
    side: Vec<String>,
    cursor: ratatui::layout::Position,
}

fn frame(app: &mut App, term: &mut Terminal<TestBackend>) -> Frame {
    use ratatui::backend::Backend;
    draw(app, term);
    let side = app.hits.regions.side.unwrap_or_default();
    let buf = term.backend().buffer().clone();
    let in_side = |x, y| side.contains(ratatui::layout::Position { x, y });
    let (mut rows, mut styles, mut side_rows) = (Vec::new(), Vec::new(), Vec::new());
    for y in 0..buf.area.height {
        let (mut row, mut side_row) = (String::new(), String::new());
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if in_side(x, y) {
                row.push(' ');
                styles.push(ratatui::style::Style::default());
                side_row.push_str(match cell.symbol() {
                    "+" | "−" | "↶" => " ",
                    s => s,
                });
            } else {
                row.push_str(cell.symbol());
                styles.push(cell.style());
            }
        }
        rows.push(row);
        if !side_row.is_empty() {
            side_rows.push(side_row);
        }
    }
    Frame {
        rows,
        styles,
        side: side_rows,
        cursor: term.backend_mut().get_cursor_position().unwrap(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_done_when_quit_and_reopen_gives_back_the_same_screen() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    project(&root);
    let mut term = Terminal::new(TestBackend::new(150, 44)).unwrap();

    // `jed proj`, then everything through keys.
    let mut app = reopen(&root, &state, None).await;
    draw(&mut app, &mut term);
    // Group 1: main.rs scrolled to line 120 with three cursors selecting a word each, the
    // bottom one primary (the status bar's Ln / Col and the terminal cursor are its)…
    quick_open(&mut app, "main.rs").await;
    app.press(KeyCode::Char('g'), KeyModifiers::CONTROL);
    app.type_text("122:5");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    for _ in 0..2 {
        app.press(KeyCode::Up, cmd() | KeyModifiers::ALT);
    }
    for _ in 0..3 {
        app.press(KeyCode::Right, KeyModifiers::SHIFT);
    }
    // …lib.rs paged down, and its git changes in a diff tab (active).
    quick_open(&mut app, "lib.rs").await;
    app.press(KeyCode::PageDown, KeyModifiers::NONE);
    app.press(KeyCode::PageDown, KeyModifiers::NONE);
    assert!(app.settle(|a| a.git.status.is_some()).await);
    palette_command(&mut app, "Git: Open Changes");
    assert!(app.group().active_diff().is_some(), "the diff tab");
    // Group 2, right: README.md near its end, "Paragraph" and its next match selected with Cmd+D.
    app.press(KeyCode::Char('\\'), cmd());
    quick_open(&mut app, "README.md").await;
    app.press_file_end();
    app.press(KeyCode::Char('g'), KeyModifiers::CONTROL);
    app.type_text("101:3");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    app.press(KeyCode::Char('d'), cmd());
    app.press(KeyCode::Char('d'), cmd());
    assert_eq!(
        app.group().active_tab().unwrap().view.selections.len(),
        2,
        "Cmd+D added a match"
    );
    // Group 3, below group 2: Cargo.toml.
    app.press(KeyCode::Char('\\'), cmd() | KeyModifiers::ALT);
    quick_open(&mut app, "Cargo.toml").await;
    assert_eq!(app.groups.ids().len(), 3);
    // Side panel on Source Control, the bottom panel shown; then back to group 2's editor.
    app.press(KeyCode::Char('g'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    app.press(KeyCode::Char('j'), cmd());
    assert!(app.ui.panel_visible);
    // Both dividers dragged off the middle (the ratios are the session's).
    for dir in [SplitDir::Right, SplitDir::Down] {
        draw(&mut app, &mut term);
        let r = app.hits.dividers.iter().find(|d| d.dir == dir).unwrap().rect;
        let ((x, y), to) = match dir {
            SplitDir::Right => ((r.x, r.y + 3), (r.x + 9, r.y + 3)),
            SplitDir::Down => ((r.x + r.width / 2, r.y), (r.x + r.width / 2, r.y + 6)),
        };
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
        app.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1);
    }
    app.press(KeyCode::Char('2'), cmd());
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(
        app.group().active_tab().unwrap().view.selections.len(),
        2,
        "group 2 kept its selections"
    );
    let group1 = app.groups.get(app.groups.ids()[0]).unwrap();
    let main_sels = &group1.tabs[0].text().unwrap().view.selections;
    assert_eq!(main_sels.primary_index(), 2, "main.rs's primary is its last");
    assert!(app.settle(screen_settled).await, "everything drawn has arrived");
    let before = frame(&mut app, &mut term);
    let ratios = format!("{:?}", app.groups.tree);
    assert!(!ratios.contains("ratio: 0.5,"), "both dividers moved: {ratios}");
    // Group 1's other tabs, drawn one by one (Ctrl+PageUp), then back to the diff.
    app.press(KeyCode::Char('1'), cmd());
    let mut tabs_before = Vec::new();
    for _ in 0..2 {
        app.press(KeyCode::PageUp, KeyModifiers::CONTROL);
        tabs_before.push(frame(&mut app, &mut term));
    }
    app.press(KeyCode::PageDown, KeyModifiers::CONTROL);
    app.press(KeyCode::PageDown, KeyModifiers::CONTROL);
    app.press(KeyCode::Char('2'), cmd());
    assert!(frame(&mut app, &mut term) == before, "looking changed nothing");
    let screen = format!("{}\n{}", before.rows.join("\n"), before.side.join("\n"));
    for text in [
        "main.rs",
        "lib.rs (Working Tree)",
        "README.md",
        "Cargo.toml",
        "SOURCE CONTROL",
    ] {
        assert!(
            screen.to_uppercase().contains(&text.to_uppercase()),
            "{text} drawn:\n{screen}"
        );
    }
    assert!(screen.contains("Paragraph 49"), "README scrolled:\n{screen}");

    // Quit (Cmd+Q: nothing unsaved, no prompt), as `run` does.
    app.press(KeyCode::Char('q'), cmd());
    assert!(app.quitting());
    app.shutdown().await;

    // `jed proj` again: the same frame, cell for cell — tab bars, the visible lines, every
    // selection and the cursor, the gutter, both panels, the dividers.
    let mut app = reopen(&root, &state, None).await;
    assert_eq!(app.session.restored, 1);
    assert!(app.settle(screen_settled).await, "everything drawn has arrived");
    let after = frame(&mut app, &mut term);
    for (y, (a, b)) in before.rows.iter().zip(&after.rows).enumerate() {
        assert_eq!(a, b, "row {y}");
    }
    assert!(before.styles == after.styles, "every cell's colours and modifiers");
    assert_eq!(before.side, after.side, "the Source Control view's rows");
    assert_eq!(before.cursor, after.cursor, "the terminal cursor");
    // Group 1's other tabs: main.rs at line 120 with its three selections, lib.rs paged down.
    app.press(KeyCode::Char('1'), cmd());
    for (i, shot) in tabs_before.iter().enumerate() {
        app.press(KeyCode::PageUp, KeyModifiers::CONTROL);
        let tab = frame(&mut app, &mut term);
        for (y, (a, b)) in shot.rows.iter().zip(&tab.rows).enumerate() {
            assert_eq!(a, b, "tab {i}, row {y}");
        }
        assert!(tab == *shot, "tab {i}: colours, the side panel, the cursor");
    }
    app.press(KeyCode::PageDown, KeyModifiers::CONTROL);
    app.press(KeyCode::PageDown, KeyModifiers::CONTROL);
    app.press(KeyCode::Char('2'), cmd());

    // And again after a second quit with nothing touched.
    quit(app).await;
    let mut app = reopen(&root, &state, None).await;
    assert!(app.settle(screen_settled).await);
    let again = frame(&mut app, &mut term);
    assert!(again == before, "a second round trip changes nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_13_done_when_a_kill_loses_at_most_one_backup_interval() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    project(&root);
    let disk = |name: &str| fs::read_to_string(root.join(name)).unwrap();
    let (main0, readme0) = (disk("src/main.rs"), disk("README.md"));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();

    // A session to come back to: main.rs, and README.md in a split.
    let mut app = reopen(&root, &state, None).await;
    quick_open(&mut app, "main.rs").await;
    app.press(KeyCode::Char('\\'), cmd());
    quick_open(&mut app, "README.md").await;
    quit(app).await;

    // Edits: three cursors in main.rs, a line in README.md.
    let mut app = reopen(&root, &state, None).await;
    app.press(KeyCode::Char('1'), cmd());
    app.press(KeyCode::Char('g'), KeyModifiers::CONTROL);
    app.type_text("10");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    app.press(KeyCode::Down, cmd() | KeyModifiers::ALT);
    app.press(KeyCode::Down, cmd() | KeyModifiers::ALT);
    app.type_text("kept ");
    app.press(KeyCode::Char('2'), cmd());
    app.type_text("Saved by the backup.");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    // One backup interval passes: everything so far is backed up.
    back_up(&mut app).await;
    let main_at_tick = text_of(&app, &root, "src/main.rs");
    let readme_at_tick = text_of(&app, &root, "README.md");
    assert_eq!(main_at_tick.matches("kept ").count(), 3);
    // More edits, less than an interval before the kill: the next backup isn't due yet.
    app.type_text("lost");
    app.press(KeyCode::Char('1'), cmd());
    app.type_text("lost ");
    app.pump();
    let due = app.backups.deadline().expect("the next backup is pending");
    assert!(due > Instant::now() && due <= Instant::now() + backup::INTERVAL);
    let main_killed = text_of(&app, &root, "src/main.rs");
    assert_eq!(main_killed.matches("kept lost ").count(), 3);
    // The terminal is killed: no Quit, no shutdown.
    drop(app);
    assert_eq!(
        (disk("src/main.rs"), disk("README.md")),
        (main0.clone(), readme0.clone())
    );

    // The next start gives back the session's tabs and offers the backups; Restore brings back
    // every edit up to the last backup, only the later ones are lost.
    let mut app = reopen(&root, &state, None).await;
    assert_eq!(app.groups.ids().len(), 2, "the session's split");
    assert_eq!(
        offered(&mut app).await,
        [root.join("README.md"), root.join("src/main.rs")]
    );
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    let restored = app
        .settle(|a| a.backups.restoring() == 0 && a.buffers.iter().all(|(_, b)| b.is_dirty()))
        .await;
    assert!(restored, "both restored");
    assert_eq!(text_of(&app, &root, "src/main.rs"), main_at_tick);
    assert_eq!(text_of(&app, &root, "README.md"), readme_at_tick);
    assert_eq!(
        main_killed.replace("kept lost ", "kept "),
        main_at_tick,
        "only `lost` is lost"
    );
    assert_eq!(app.groups.ids().len(), 2, "into the session's tabs, none added");
    let screen = frame(&mut app, &mut term).rows.join("\n");
    assert!(
        screen.contains("Saved by the backup.") && screen.contains("kept "),
        "{screen}"
    );
    assert!(!screen.contains("lost"), "{screen}");
    // Nothing written; one undo per file (Cmd+Z) gives back the disk's text, clean, and the
    // backups go.
    assert_eq!(
        (disk("src/main.rs"), disk("README.md")),
        (main0.clone(), readme0.clone())
    );
    for g in ['1', '2'] {
        app.press(KeyCode::Char(g), cmd());
        app.press(KeyCode::Char('z'), cmd());
        assert!(!app.active_buffer().unwrap().is_dirty());
    }
    assert_eq!(text_of(&app, &root, "src/main.rs"), main0);
    assert_eq!(text_of(&app, &root, "README.md"), readme0);
    assert!(
        app.settle(|_| !backup::dir(&state, &root).exists()
            || fs::read_dir(backup::dir(&state, &root)).unwrap().next().is_none())
            .await,
        "backups removed"
    );
}
