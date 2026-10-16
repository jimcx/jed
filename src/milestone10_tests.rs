//! Milestone 10 end to end (so far 10a, 10b). 10a: the git status read off-thread at startup and
//! kept current through `notify` (work tree edits, staging and branch switches made outside
//! red), the branch in the status bar (unborn and detached HEAD too), git colouring in the
//! explorer (changed files and the folders holding them), no repository (git off), and a
//! workspace that is a subdirectory of the work tree (its `.git` watched separately).
//! 10b, files changed on disk, through the real watcher: a clean open file reloads (cursors and
//! scroll in every view and undo kept), our own saves aren't changes but a write right after one
//! is, a dirty file gets the banner (a save or quit refused, Keep Mine, Reload), binary and
//! non-UTF-8 files aren't reloaded until asked, a deleted file's tab (also one in a folder
//! renamed away) is marked and saving recreates it. Checks racing a save or an edit are
//! `app::disk`'s unit tests.

use std::fs;
use std::path::{Path, PathBuf};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crossterm::event::{KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::style::Modifier;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::disk::BannerAction;
use crate::git::test_repo::{commit_all, git, init};
use crate::git::{Change, Decoration};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn open_app(root: &Path, state: &Path) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app
}

/// A temp dir with `proj/` (the workspace) and `state/` (the trust store, outside the repo).
fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let base = workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    (dir, root, state)
}

fn draw(app: &mut App) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// Each screen row as text.
fn lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

fn status_bar(term: &Terminal<TestBackend>) -> String {
    lines(term).pop().unwrap()
}

/// The foreground of the explorer row naming `name` (its first letter), and the row's text.
fn explorer_row(term: &Terminal<TestBackend>, name: &str) -> (Option<Color>, String) {
    let buf = term.backend().buffer();
    for (y, line) in lines(term).into_iter().enumerate() {
        // The side panel is left of the editor; its rows start after the activity bar.
        let side: String = line.chars().take(32).collect();
        if let Some(x) = side.find(&format!(" {name}")) {
            let x = side[..x + 1].chars().count() as u16;
            return (buf[(x, y as u16)].fg.into(), side);
        }
    }
    panic!("no explorer row {name}: {:#?}", lines(term));
}

fn colour(app: &App, d: Decoration) -> Option<Color> {
    app.theme.get(d.theme_key()).fg
}

/// Waits for a git status matching `f` (after any refresh in flight).
async fn settle_git(app: &mut App, f: impl Fn(&crate::git::Status) -> bool) -> bool {
    app.settle(|a| !a.git.running() && a.git.status.as_ref().is_some_and(&f))
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_status_bar_and_explorer_follow_the_repository() {
    let (_d, root, state) = dirs();
    init(&root);
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    commit_all(&root, "one");
    fs::write(root.join("src/a.rs"), "fn a() { 1 }\n").unwrap();
    fs::write(root.join("new.txt"), "n\n").unwrap();
    fs::write(root.join("debug.log"), "ignored\n").unwrap();

    let mut app = open_app(&root, &state);
    assert!(
        settle_git(&mut app, |s| s.entries.len() == 2).await,
        "first status read"
    );
    assert!(app.settle(|a| a.explorer.rows().len() >= 4).await, "explorer listed");
    let term = draw(&mut app);
    assert!(status_bar(&term).starts_with(" ⎇ main* "), "{}", status_bar(&term));
    // A folder holding a modified file, an untracked file, an unchanged one (the ignored one
    // isn't listed, nor in the status).
    let (fg, row) = explorer_row(&term, "src");
    assert_eq!(fg, colour(&app, Decoration::Modified), "{row}");
    assert!(row.trim_end().ends_with("src"), "no letter on a folder: {row:?}");
    let (fg, row) = explorer_row(&term, "new.txt");
    assert_eq!(fg, colour(&app, Decoration::Untracked));
    assert!(row.trim_end().ends_with('U'), "the letter at the right: {row:?}");
    let (fg, row) = explorer_row(&term, "b.txt");
    assert_eq!(fg, app.theme.get("sideBar").fg);
    assert!(
        row.trim_end().ends_with("b.txt"),
        "no letter on an unchanged file: {row:?}"
    );
    // Reading the status touches nothing the watcher reports: no refresh follows by itself.
    let until = std::time::Instant::now() + std::time::Duration::from_millis(400);
    app.settle(|a| a.git.running() || std::time::Instant::now() > until)
        .await;
    assert!(!app.git.running(), "a refresh started with nothing changed");

    // Edits on disk, outside red.
    fs::write(root.join("b.txt"), "b2\n").unwrap();
    assert!(
        settle_git(&mut app, |s| s.entry(Path::new("b.txt")).is_some()).await,
        "a file changed on disk"
    );
    let term = draw(&mut app);
    let (fg, row) = explorer_row(&term, "b.txt");
    assert_eq!(fg, colour(&app, Decoration::Modified));
    assert!(row.trim_end().ends_with('M'), "{row:?}");

    // Staging outside red: only the index changes.
    git(&root, &["add", "b.txt", "new.txt"]);
    assert!(
        settle_git(&mut app, |s| s
            .entry(Path::new("new.txt"))
            .is_some_and(|e| e.staged == Some(Change::Added) && e.unstaged.is_none()))
        .await,
        "staged files"
    );
    let term = draw(&mut app);
    assert!(status_bar(&term).starts_with(" ⎇ main*+ "), "{}", status_bar(&term));
    let (fg, row) = explorer_row(&term, "new.txt");
    assert_eq!(fg, colour(&app, Decoration::Added));
    assert!(row.trim_end().ends_with('A'), "{row:?}");

    // A branch switch, then a detached HEAD.
    commit_all(&root, "two");
    git(&root, &["checkout", "-q", "-b", "feature"]);
    assert!(
        settle_git(&mut app, |s| s.head.name() == "feature" && s.entries.is_empty()).await,
        "switched branch"
    );
    let term = draw(&mut app);
    assert!(status_bar(&term).starts_with(" ⎇ feature "), "{}", status_bar(&term));
    assert_eq!(explorer_row(&term, "src").0, app.theme.get("sideBar").fg);
    let id = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "-q", "--detach"]);
    assert!(
        settle_git(&mut app, |s| s.head == crate::git::Head::Detached(id[..8].to_string())).await,
        "detached HEAD"
    );
    assert!(status_bar(&draw(&mut app)).starts_with(&format!(" ⎇ {} ", &id[..8])));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_no_repository_turns_git_off_until_init() {
    let (_d, root, state) = dirs();
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let mut app = open_app(&root, &state);
    assert!(app.settle(|a| !a.git.running() && !a.explorer.rows().is_empty()).await);
    assert!(app.git.status.is_none());
    let term = draw(&mut app);
    assert!(!status_bar(&term).contains('⎇'), "{}", status_bar(&term));

    // `git init` in the workspace: found through the watcher. No commits yet.
    init(&root);
    assert!(
        settle_git(&mut app, |s| s.head == crate::git::Head::Unborn("main".into())).await,
        "repository found"
    );
    let term = draw(&mut app);
    assert!(status_bar(&term).starts_with(" ⎇ main* "), "{}", status_bar(&term));
    assert_eq!(explorer_row(&term, "a.txt").0, colour(&app, Decoration::Untracked));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_workspace_in_a_subdirectory_of_the_work_tree() {
    let (_d, top, state) = dirs();
    init(&top);
    let root = top.join("sub");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a.txt"), "a\n").unwrap();
    fs::write(top.join("outside.txt"), "o\n").unwrap();
    commit_all(&top, "one");

    let mut app = open_app(&root, &state);
    assert!(settle_git(&mut app, |s| s.entries.is_empty()).await, "status read");
    assert_eq!(app.git.status.as_ref().unwrap().repo.workdir, top);
    assert!(app.settle(|a| !a.explorer.rows().is_empty()).await);

    fs::write(root.join("a.txt"), "a2\n").unwrap();
    assert!(
        settle_git(&mut app, |s| s.entry(Path::new("sub/a.txt")).is_some()).await,
        "a change in the workspace"
    );
    let term = draw(&mut app);
    assert_eq!(explorer_row(&term, "a.txt").0, colour(&app, Decoration::Modified));
    // Staged from the top: only `.git/index` (outside the workspace root) changes.
    git(&top, &["add", "sub/a.txt"]);
    assert!(
        settle_git(&mut app, |s| s
            .entry(Path::new("sub/a.txt"))
            .is_some_and(|e| e.staged == Some(Change::Modified) && e.unstaged.is_none()))
        .await,
        "the git directory outside the root is watched"
    );
}

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// The app with `file` (in `root`) open in the editor.
fn open_file_app(root: &Path, state: &Path, file: &str) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let buffer = Buffer::open(&root.join(file)).unwrap();
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.test_start();
    app
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn head(app: &App) -> usize {
    app.group().active_tab().unwrap().view.selections.primary().head.0
}

/// Lets a file system event's debounce and check run: `true` once `checked` grew.
async fn settle_check(app: &mut App, before: u64) -> bool {
    app.settle(|a| a.disk.checked > before).await
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_clean_file_reloads_when_changed_on_disk() {
    let (_d, root, state) = dirs();
    fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    // The cursor on "three", col 2; a second view (split) on "two".
    app.press(KeyCode::Down, M::NONE);
    app.press(KeyCode::Down, M::NONE);
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Right, M::NONE);
    assert_eq!(head(&app), 10);
    app.press(KeyCode::Char('\\'), cmd());
    assert_eq!(app.groups.len(), 2, "split");
    app.press(KeyCode::Up, M::NONE);
    let other = app.groups.active;
    let first = app.groups.ids().into_iter().find(|g| *g != other).unwrap();

    fs::write(root.join("a.txt"), "zero\none\ntwo\nthree\nfour\n").unwrap();
    assert!(
        app.settle(|a| text(a) == "zero\none\ntwo\nthree\nfour\n").await,
        "reloaded: {:?}",
        text(&app)
    );
    let buf = app.active_buffer().unwrap();
    assert!(!buf.is_dirty() && buf.disk_conflict.is_none());
    assert_eq!(app.disk.reloads, 1);
    let view = |app: &App, g| {
        app.groups
            .get(g)
            .unwrap()
            .active_tab()
            .unwrap()
            .view
            .selections
            .primary()
            .head
            .0
    };
    assert_eq!(view(&app, first), 15, "still on \"three\", col 2");
    assert_eq!(view(&app, other), 11, "the other view still on \"two\"");

    // Undo brings the old text back (dirty: it's not the file's); redo is clean again.
    app.press(KeyCode::Char('z'), cmd());
    assert_eq!(text(&app), "one\ntwo\nthree\n");
    assert!(app.active_buffer().unwrap().is_dirty());
    app.press(KeyCode::Char('z'), cmd() | M::SHIFT);
    assert!(!app.active_buffer().unwrap().is_dirty());

    // Not text any more: not reloaded, said so, and the banner offers the choice.
    fs::write(root.join("a.txt"), b"bin\0ary").unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await,
        "binary file"
    );
    assert_eq!(text(&app), "zero\none\ntwo\nthree\nfour\n");
    assert!(
        app.message.as_ref().unwrap().text.contains("binary"),
        "{}",
        app.message.as_ref().unwrap().text
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_own_saves_are_not_changes_on_disk() {
    let (_d, root, state) = dirs();
    let path = root.join("a.txt");
    fs::write(&path, "one\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    app.type_text("x");
    let checked = app.disk.checked;
    app.press(KeyCode::Char('s'), cmd());
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().save_in_flight()).await,
        "saved"
    );
    assert!(!app.active_buffer().unwrap().is_dirty());
    // Typing on before the save's own events are checked: they find just what was written,
    // so the new edits get no banner.
    app.type_text("z");
    assert!(settle_check(&mut app, checked).await, "the save's events were checked");
    assert!(app.settle(|a| !a.disk.busy(0)).await);
    let buf = app.active_buffer().unwrap();
    assert_eq!(app.disk.reloads, 0);
    assert!(buf.disk_conflict.is_none() && buf.is_dirty() && !buf.deleted);
    assert_eq!(text(&app), "xzone\n");
    assert!(!lines(&draw(&mut app)).iter().any(|r| r.contains("has changed on disk")));
    assert_eq!(fs::read_to_string(&path).unwrap(), "xone\n");
    app.press(KeyCode::Char('z'), cmd());
    assert!(!app.active_buffer().unwrap().is_dirty(), "back to what was saved");
    app.press(KeyCode::Char('z'), cmd());
    assert_eq!(text(&app), "one\n", "undo history untouched");
    app.press(KeyCode::Char('z'), cmd() | M::SHIFT);

    // An external write right after a save (its events not handled yet) is still noticed.
    app.type_text("y");
    app.press(KeyCode::Char('s'), cmd());
    assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await, "saved");
    fs::write(&path, "theirs\n").unwrap();
    assert!(
        app.settle(|a| text(a) == "theirs\n").await,
        "reloaded: {:?}",
        text(&app)
    );
    assert_eq!(app.disk.reloads, 1);
    assert!(!app.active_buffer().unwrap().is_dirty());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_dirty_file_changed_on_disk_shows_the_banner() {
    let (_d, root, state) = dirs();
    let path = root.join("a.txt");
    fs::write(&path, "one\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    app.type_text("mine ");
    fs::write(&path, "theirs\n").unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await,
        "the banner"
    );
    assert_eq!(text(&app), "mine one\n", "edits kept");
    let term = draw(&mut app);
    let rows = lines(&term);
    let banner = rows
        .iter()
        .find(|r| r.contains("a.txt has changed on disk"))
        .expect("banner row");
    assert!(
        banner.contains(" Reload ") && banner.contains(" Keep Mine "),
        "{banner}"
    );
    assert!(rows[2].contains("mine one"), "the text below the banner: {rows:#?}");

    // Saving now would lose their version: refused, the file untouched.
    app.press(KeyCode::Char('s'), cmd());
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().save_in_flight()).await,
        "save ended"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\n");
    assert!(app.active_buffer().unwrap().is_dirty());
    assert!(app.message.as_ref().unwrap().text.contains("not saved"));
    // Nor does quitting through "Save" overwrite it, or quit.
    app.press(KeyCode::Char('q'), cmd());
    app.press(KeyCode::Char('s'), M::NONE);
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().save_in_flight()).await,
        "save ended"
    );
    assert!(!app.quitting());
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\n");
    draw(&mut app);

    // Keep Mine (clicked): no banner; the next save overwrites.
    let button = |app: &App, action| {
        let b = app.hits.groups[0].banner.as_ref().expect("banner drawn");
        b.actions.iter().find(|(_, a)| *a == action).unwrap().0
    };
    let r = button(&app, BannerAction::KeepMine);
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    let buf = app.active_buffer().unwrap();
    assert!(buf.disk_conflict.is_none() && buf.is_dirty());
    assert!(!lines(&draw(&mut app)).iter().any(|r| r.contains("has changed on disk")));
    app.press(KeyCode::Char('s'), cmd());
    assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await, "saved");
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine one\n");
    assert!(!app.quitting(), "the refused quit isn't resumed by a later save");

    // A later change shows it again; Reload (clicked) takes theirs, undo brings mine back.
    app.type_text("again ");
    fs::write(&path, "theirs 2\n").unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await,
        "the banner again"
    );
    draw(&mut app);
    let r = button(&app, BannerAction::Reload);
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    assert!(
        app.settle(|a| text(a) == "theirs 2\n").await,
        "reloaded: {:?}",
        text(&app)
    );
    let buf = app.active_buffer().unwrap();
    assert!(!buf.is_dirty() && buf.disk_conflict.is_none());
    app.press(KeyCode::Char('z'), cmd());
    assert_eq!(text(&app), "mine again one\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_deleted_file_marks_the_tab_and_save_recreates_it() {
    let (_d, root, state) = dirs();
    let path = root.join("a.txt");
    fs::write(&path, "one\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    fs::remove_file(&path).unwrap();
    assert!(app.settle(|a| a.active_buffer().unwrap().deleted).await, "deleted");
    assert_eq!(text(&app), "one\n", "the buffer stays");
    let term = draw(&mut app);
    let tab_bar = &lines(&term)[0];
    assert!(tab_bar.contains("a.txt (deleted)"), "{tab_bar}");
    let x = tab_bar.find("a.txt").unwrap();
    let x = tab_bar[..x].chars().count() as u16;
    assert!(term.backend().buffer()[(x, 0)].modifier.contains(Modifier::CROSSED_OUT));

    app.press(KeyCode::Char('s'), cmd());
    // The save itself clears the mark (not a later check of its events).
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().save_in_flight()).await,
        "saved"
    );
    let buf = app.active_buffer().unwrap();
    assert!(!buf.deleted && !buf.is_dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\n", "recreated");
    assert!(!lines(&draw(&mut app))[0].contains("(deleted)"));

    // Deleted, then back with the same bytes: as it was.
    fs::remove_file(&path).unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().deleted).await,
        "deleted again"
    );
    fs::write(&path, "one\n").unwrap();
    assert!(app.settle(|a| !a.active_buffer().unwrap().deleted).await, "back");
    let buf = app.active_buffer().unwrap();
    assert!(!buf.is_dirty() && buf.disk_conflict.is_none());
    assert_eq!(text(&app), "one\n");

    // Renamed away (a delete), then back with other text: clean, so reloaded.
    fs::rename(&path, root.join("b.txt")).unwrap();
    assert!(app.settle(|a| a.active_buffer().unwrap().deleted).await, "renamed away");
    fs::write(&path, "two\n").unwrap();
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().deleted && text(a) == "two\n")
            .await,
        "reappeared: {:?}",
        text(&app)
    );
    assert!(!app.active_buffer().unwrap().is_dirty());

    // With unsaved edits: deleted, then back with other text: not deleted, the banner instead.
    app.type_text("mine ");
    fs::remove_file(&path).unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().deleted).await,
        "deleted with edits"
    );
    fs::write(&path, "three\n").unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await,
        "the banner"
    );
    let buf = app.active_buffer().unwrap();
    assert!(!buf.deleted && buf.is_dirty());
    assert_eq!(text(&app), "mine two\n");
    assert!(!lines(&draw(&mut app))[0].contains("(deleted)"));
}

/// A folder renamed away takes its open files with it (one event for the folder, none for
/// the files in it), and back again they're as they were.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_folder_renamed_away_marks_its_files_deleted() {
    let (_d, root, state) = dirs();
    fs::create_dir_all(root.join("sub/deep")).unwrap();
    fs::write(root.join("sub/deep/a.txt"), "one\n").unwrap();
    let mut app = open_file_app(&root, &state, "sub/deep/a.txt");
    fs::rename(root.join("sub"), root.join("moved")).unwrap();
    assert!(app.settle(|a| a.active_buffer().unwrap().deleted).await, "deleted");
    assert!(lines(&draw(&mut app))[0].contains("a.txt (deleted)"));
    fs::rename(root.join("moved"), root.join("sub")).unwrap();
    assert!(app.settle(|a| !a.active_buffer().unwrap().deleted).await, "back");
    assert!(!app.active_buffer().unwrap().is_dirty());
}

/// Reloading keeps every view's scroll on the same text, and a file that's no longer valid
/// UTF-8 isn't reloaded until asked ("Reload from Disk"), then read-only.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_reload_keeps_the_scroll_and_refuses_non_utf8() {
    let (_d, root, state) = dirs();
    let path = root.join("a.txt");
    let numbered = |from: usize, to: usize| (from..to).map(|i| format!("line {i}\n")).collect::<String>();
    fs::write(&path, numbered(0, 100)).unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    draw(&mut app);
    let g = app.groups.active;
    {
        let rope = app.active_buffer().unwrap().rope.clone();
        let view = &mut app.groups.get_mut(g).unwrap().tabs[0].view;
        view.set_top_line(&rope, 40);
    }
    let top = |app: &App| app.group().active_tab().unwrap().view.top_line();
    // Three lines above the viewport: it moves down with the text.
    fs::write(&path, format!("a\nb\nc\n{}", numbered(0, 100))).unwrap();
    assert!(app.settle(|a| text(a).starts_with("a\n")).await, "reloaded");
    assert_eq!(top(&app), 43);
    let screen = lines(&draw(&mut app));
    assert!(screen[1].contains("line 40"), "the same text on top: {screen:#?}");

    // Not UTF-8: not reloaded, said so, the banner offers the choice.
    fs::write(&path, b"caf\xE9\n").unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await,
        "not reloaded"
    );
    assert!(text(&app).starts_with("a\n"));
    assert!(app.active_buffer().unwrap().read_only.is_none());
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("not reloaded"), "{message}");
    let screen = lines(&draw(&mut app));
    assert!(
        screen.iter().any(|r| r.contains("a.txt has changed on disk and wasn")),
        "{screen:#?}"
    );
    app.press(KeyCode::Char('p'), cmd() | M::SHIFT);
    app.type_text("Reload from Disk");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| text(a) == "caf\u{FFFD}\n").await, "{:?}", text(&app));
    let buf = app.active_buffer().unwrap();
    assert!(buf.read_only.is_some() && buf.disk_conflict.is_none() && !buf.is_dirty());
}
