//! Milestone 10 end to end (so far 10a–10d). 10a: the git status read off-thread at startup and
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
//! 10c, the gutter: markers for added / modified / deleted lines drawn in their colours, moved
//! with edits at once and recomputed after them, after `git add` outside red and for an
//! untracked file (none); a click on a marker opens the peek, whose Stage (button or Enter)
//! gives the same index as `git add -p` for that hunk; a dirty buffer is saved first (asked;
//! Cancel stages nothing), a save in flight is waited for; unstaging the change at the cursor is
//! `git reset -p`'s; Alt+F5 / Shift+Alt+F5 step between changes; typing at 60k cursors with
//! markers keeps M6's keystroke budget (release).
//! 10d, the diff view: Git: Open Changes opens a diff tab (unstaged, else staged; asked again,
//! the same tab) drawn side by side with aligned rows (changed pairs, filler against added /
//! deleted lines, each side numbered, Dark+'s line and text backgrounds) or in one column when
//! narrow; F7 / Shift+F7 and Alt+F5 step through the changes, wrapping; the keys, the wheel and
//! clicks move and scroll it; staging the change at the cursor (command or its Stage button)
//! gives `git add -p`'s index — the file open (from disk, a dirty buffer saved first) or not;
//! refused when the file changed since it was shown — and unstaging in the staged view `git
//! reset -p`'s; Stage / Unstage File (a dirty open file saved first); no changes / untracked; the view follows
//! the file and the index changing outside red and the open buffer's unsaved edits; splitting
//! copies it, closing asks nothing; every other command does nothing to the buffer from it;
//! the banner's Show Diff (on disk ↔ yours, the banner's buttons working over it); a 1 MB
//! diff loads off the UI thread and scrolls within a frame (release).

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
use crate::git::test_repo::{commit_all, git, git_with_input, init};
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
        let view = &mut app.groups.get_mut(g).unwrap().tabs[0].text_mut().unwrap().view;
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
    let screen = lines(&draw_sized(&mut app, 130, 20));
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

// 10c: the gutter.

/// A repository with `files` committed, then `changes` written over them on disk.
fn repo_with(root: &Path, files: &[(&str, &str)], changes: &[(&str, &str)]) {
    init(root);
    for (name, text) in files {
        fs::write(root.join(name), text).unwrap();
    }
    commit_all(root, "one");
    for (name, text) in changes {
        fs::write(root.join(name), text).unwrap();
    }
}

fn numbered(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

/// Waits until the active buffer's markers are current (nothing running or due) and the git
/// status read.
async fn settle_markers(app: &mut App) -> bool {
    app.settle(|a| {
        let buf = a.active_buffer().unwrap();
        !a.git.running()
            && a.git.status.is_some()
            && !buf.quick_diff.running()
            && buf.quick_diff.want(buf.revision()) == crate::quick_diff::Want::Nothing
            && a.quick_diff.deadline().is_none()
            && a.quick_diff.staging == 0
    })
    .await
}

/// The gutter's markers on screen: (buffer line, glyph, colour) for each marked row.
fn markers(app: &mut App) -> Vec<(usize, String, Option<Color>)> {
    let term = draw(app);
    let text = app.hits.group(app.groups.active).unwrap().text;
    let top = app.group().active_tab().unwrap().view.top_line();
    let buf = term.backend().buffer();
    (0..text.height)
        .filter_map(|r| {
            let cell = &buf[(text.x - 1, text.y + r)];
            (cell.symbol() != " ").then(|| (top + r as usize, cell.symbol().to_string(), cell.fg.into()))
        })
        .collect()
}

fn marker_colour(app: &App, key: &str) -> Option<Color> {
    app.theme.get(key).fg
}

/// Runs the palette command titled `title`.
fn run_command(app: &mut App, title: &str) {
    app.press(KeyCode::Char('p'), cmd() | M::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(KeyCode::Enter, M::NONE);
}

/// What the index holds: `git ls-files -s` and the cached diff.
fn index_state(root: &Path) -> (String, String) {
    (
        git(root, &["ls-files", "-s"]),
        git(root, &["diff", "--cached", "--no-color"]),
    )
}

/// A copy of the repository at `root` (beside it, as `name`), for `git add -p` to stage in.
fn copy_repo(root: &Path, name: &str) -> PathBuf {
    let to = root.with_file_name(name);
    let ok = std::process::Command::new("cp")
        .arg("-a")
        .arg(root)
        .arg(&to)
        .status()
        .unwrap();
    assert!(ok.success());
    to
}

fn place_cursor(app: &mut App, line: usize) {
    app.press(KeyCode::Home, cmd());
    for _ in 0..line {
        app.press(KeyCode::Down, M::NONE);
    }
}

fn cursor_line(app: &App) -> usize {
    let buf = app.active_buffer().unwrap();
    crate::buffer::pos::line_of(&buf.rope, crate::buffer::pos::ByteIdx(head(app)))
}

async fn settle_staged(app: &mut App, what: &str) -> bool {
    app.settle(|a| a.quick_diff.staging == 0 && a.message.as_ref().is_some_and(|m| m.text.contains(what)))
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_gutter_markers_follow_edits_and_the_index() {
    let (_d, root, state) = dirs();
    let text = numbered(10);
    let changed = text
        .replace("line 1\n", "one\n")
        .replace("line 4\n", "")
        .replace("line 9\n", "line 9\nten\neleven\n");
    repo_with(&root, &[("a.txt", &text), ("new.txt", "n\n")], &[("a.txt", &changed)]);
    fs::write(root.join("untracked.txt"), "u\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    assert!(settle_markers(&mut app).await);
    let (added, modified, deleted) = (
        marker_colour(&app, "editorGutter.addedBackground"),
        marker_colour(&app, "editorGutter.modifiedBackground"),
        marker_colour(&app, "editorGutter.deletedBackground"),
    );
    // Line 1 modified; "line 4" deleted (drawn below line 3); lines 9 and 10 added.
    let expected = vec![
        (1, "▎".to_string(), modified),
        (3, "▁".to_string(), deleted),
        (9, "▎".to_string(), added),
        (10, "▎".to_string(), added),
    ];
    assert_eq!(markers(&mut app), expected);

    // A line typed at the top: the markers move with the text at once, before any recompute.
    app.press(KeyCode::Enter, M::NONE);
    let moved: Vec<_> = expected.iter().map(|(l, g, c)| (l + 1, g.clone(), *c)).collect();
    let mut now = moved.clone();
    now.insert(0, (0, "▎".to_string(), added));
    assert_eq!(markers(&mut app), moved, "mapped through the edit");
    // Not recomputed at each key: after `quick_diff::DELAY`, not pushed back by more keys.
    app.pump();
    let deadline = app.quick_diff.deadline().expect("a recompute is due later");
    assert!(
        !app.active_buffer().unwrap().quick_diff.running(),
        "nothing started at the key"
    );
    app.press(KeyCode::Char('q'), M::NONE);
    app.press(KeyCode::Backspace, M::NONE);
    app.pump();
    assert_eq!(app.quick_diff.deadline(), Some(deadline));
    assert!(!app.active_buffer().unwrap().quick_diff.running());
    // Then recomputed: the new line is an addition.
    assert!(settle_markers(&mut app).await);
    assert_eq!(markers(&mut app), now);
    // Undone (the Backspace, then the Enter and the q): back as they were.
    app.press(KeyCode::Char('z'), cmd());
    app.press(KeyCode::Char('z'), cmd());
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), changed);
    assert!(settle_markers(&mut app).await);
    assert_eq!(markers(&mut app), expected);

    // `git add` outside red: the index now holds the file, nothing is marked.
    git(&root, &["add", "a.txt"]);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().quick_diff.marks().is_empty())
            .await
    );
    assert!(settle_markers(&mut app).await);
    assert_eq!(markers(&mut app), vec![]);
    // Typing changes a line against the new index.
    place_cursor(&mut app, 5);
    app.type_text("x");
    assert!(settle_markers(&mut app).await);
    assert_eq!(markers(&mut app), vec![(5, "▎".to_string(), modified)]);
    // Taken out of the index (untracked now): no markers.
    git(&root, &["rm", "-q", "--cached", "a.txt"]);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().quick_diff.marks().is_empty())
            .await
    );
    assert!(settle_markers(&mut app).await);
    assert_eq!(markers(&mut app), vec![]);

    // An untracked file has no markers.
    app.test_open(Buffer::open(&root.join("untracked.txt")).unwrap());
    app.type_text("more ");
    assert!(settle_markers(&mut app).await);
    assert_eq!(markers(&mut app), vec![]);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_stage_from_the_gutter_like_add_p() {
    let (_d, root, state) = dirs();
    let original = numbered(40);
    let changed = original
        .replace("line 2\n", "two\n")
        .replace("line 20\n", "twenty\nmore\n")
        .replace("line 35\n", "");
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let reference = copy_repo(&root, "reference");

    // Scroll so line 21 is on screen, then click its marker: the peek opens below the change.
    place_cursor(&mut app, 25);
    let _ = markers(&mut app);
    let text_rect = app.hits.group(app.groups.active).unwrap().text;
    let top = app.group().active_tab().unwrap().view.top_line();
    assert!(top <= 20 && 21 < top + text_rect.height as usize, "line 21 on screen");
    let y = text_rect.y + (21 - top) as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), text_rect.x - 1, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), text_rect.x - 1, y);
    assert_eq!(app.quick_diff.peek.as_ref().map(|p| p.lines.clone()), Some(20..22));
    assert_eq!(cursor_line(&app), 25, "the click doesn't move the cursor");
    let screen = lines(&draw(&mut app));
    let peek = app.hits.quick_diff.expect("the peek is drawn");
    let rows: Vec<&String> = screen[peek.rect.y as usize..(peek.rect.y + peek.rect.height) as usize]
        .iter()
        .collect();
    assert!(
        rows.iter()
            .any(|r| r.contains("1 line changed to 2") && r.contains("Stage Change")),
        "{rows:#?}"
    );
    assert!(rows.iter().any(|r| r.contains("- line 20")), "{rows:#?}");
    assert!(rows.iter().any(|r| r.contains("+ twenty")) && rows.iter().any(|r| r.contains("+ more")));
    // A click on the marker again closes it; another opens it.
    app.mouse(MouseEventKind::Down(MouseButton::Left), text_rect.x - 1, y);
    assert!(app.quick_diff.peek.is_none());
    app.mouse(MouseEventKind::Down(MouseButton::Left), text_rect.x - 1, y);
    assert!(app.quick_diff.peek.is_some());

    // Stage: the same index as `git add -p` answering yes to that hunk only.
    let _ = draw(&mut app);
    let stage = app.hits.quick_diff.unwrap().stage;
    app.mouse(MouseEventKind::Down(MouseButton::Left), stage.x + 1, stage.y);
    assert!(app.quick_diff.peek.is_none());
    assert!(settle_staged(&mut app, "Staged the change at line 21").await);
    git_with_input(&reference, &["add", "-p", "--", "f.txt"], Some("n\ny\nn\n"));
    assert_eq!(index_state(&root), index_state(&reference));
    // The staged change isn't marked any more.
    assert!(settle_markers(&mut app).await);
    let modified = marker_colour(&app, "editorGutter.modifiedBackground");
    let shown = markers(&mut app);
    assert!(!shown.iter().any(|(l, _, _)| *l == 20 || *l == 21), "{shown:?}");
    assert_eq!(
        app.active_buffer().unwrap().quick_diff.marks().len(),
        2,
        "lines 2 and 35 left"
    );

    // Enter in the peek stages too: the deletion, drawn below line 35.
    place_cursor(&mut app, 39);
    let _ = draw(&mut app);
    let top = app.group().active_tab().unwrap().view.top_line();
    assert!(top <= 35, "line 35 on screen");
    app.mouse(
        MouseEventKind::Down(MouseButton::Left),
        text_rect.x - 1,
        text_rect.y + (35 - top) as u16,
    );
    assert!(app.quick_diff.peek.is_some());
    // Esc closes it, Enter stages from it.
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.quick_diff.peek.is_none());
    assert_eq!(app.group().active_tab().unwrap().view.selections.len(), 1);
    app.mouse(
        MouseEventKind::Down(MouseButton::Left),
        text_rect.x - 1,
        text_rect.y + (35 - top) as u16,
    );
    app.press(KeyCode::Enter, M::NONE);
    assert!(settle_staged(&mut app, "Staged the change at line 36").await);
    assert_eq!(text(&app), changed, "Enter didn't type");
    git(&reference, &["reset", "-q"]);
    git_with_input(&reference, &["add", "-p", "--", "f.txt"], Some("n\ny\ny\n"));
    assert_eq!(index_state(&root), index_state(&reference));
    assert!(settle_markers(&mut app).await);
    assert_eq!(
        markers(&mut app).iter().map(|(l, _, _)| *l).collect::<Vec<_>>(),
        Vec::<usize>::new()
    );
    // Line 2 is above the screen: scroll there to see it's still marked.
    place_cursor(&mut app, 0);
    assert_eq!(markers(&mut app), vec![(2, "▎".to_string(), modified)]);

    // With the peek open, a click in the text closes it and places the cursor there.
    let top = app.group().active_tab().unwrap().view.top_line();
    let row = |line: usize| text_rect.y + (line - top) as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), text_rect.x - 1, row(2));
    assert!(app.quick_diff.peek.is_some());
    let _ = draw(&mut app);
    let peek = app.hits.quick_diff.unwrap().rect;
    let below = peek.y + peek.height + 1;
    app.mouse(MouseEventKind::Down(MouseButton::Left), text_rect.x + 2, below);
    app.mouse(MouseEventKind::Up(MouseButton::Left), text_rect.x + 2, below);
    assert!(app.quick_diff.peek.is_none());
    assert_eq!(cursor_line(&app), top + (below - text_rect.y) as usize);
    // The file changed outside red (a clean buffer reloads): the peek closes.
    app.mouse(MouseEventKind::Down(MouseButton::Left), text_rect.x - 1, row(2));
    assert!(app.quick_diff.peek.is_some());
    fs::write(root.join("f.txt"), changed.replace("line 10\n", "ten\n")).unwrap();
    assert!(app.settle(|a| text(a).contains("ten\n")).await);
    assert!(app.quick_diff.peek.is_none(), "closed by the reload");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_staging_a_dirty_buffer_saves_it_first() {
    let (_d, root, state) = dirs();
    let text = numbered(30);
    repo_with(&root, &[("f.txt", &text)], &[]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    place_cursor(&mut app, 5);
    app.type_text("x");
    assert!(settle_markers(&mut app).await);
    // No change on line 10: nothing to ask about.
    place_cursor(&mut app, 10);
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(app.prompt.is_none());
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .contains("No change to stage at line 11")
    );
    // On line 5: asked to save first; Cancel stages nothing and leaves the buffer dirty.
    place_cursor(&mut app, 5);
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(matches!(
        app.prompt,
        Some(crate::app::Prompt::SaveBeforeStage { line: 5, .. })
    ));
    let screen = lines(&draw(&mut app));
    assert!(
        screen
            .iter()
            .any(|r| r.contains("Save f.txt before staging the change?")),
        "{screen:#?}"
    );
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.prompt.is_none() && app.active_buffer().unwrap().is_dirty());
    assert_eq!(git(&root, &["diff", "--cached"]), "");
    // Save: written, then staged from the file on disk.
    run_command(&mut app, "Git: Stage Change at Cursor");
    app.press(KeyCode::Char('s'), M::NONE);
    assert!(settle_staged(&mut app, "Staged the change at line 6").await);
    assert!(!app.active_buffer().unwrap().is_dirty());
    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        text.replace("line 5\n", "xline 5\n")
    );
    assert_eq!(git(&root, &["diff"]), "", "everything on disk staged");
    assert!(git(&root, &["diff", "--cached"]).contains("+xline 5"));
    assert!(settle_markers(&mut app).await);
    assert_eq!(markers(&mut app), vec![]);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_staging_waits_for_a_save_in_flight() {
    let (_d, root, state) = dirs();
    let text = numbered(30);
    repo_with(&root, &[("f.txt", &text)], &[]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    place_cursor(&mut app, 3);
    app.type_text("y");
    assert!(settle_markers(&mut app).await);
    // Save, and stage before the save's result is handled: it waits, then stages from disk.
    app.press(KeyCode::Char('s'), cmd());
    assert!(app.active_buffer().unwrap().save_in_flight());
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(app.prompt.is_none(), "a save in flight isn't asked about");
    assert!(app.message.as_ref().unwrap().text.contains("Waiting for the save"));
    assert_eq!(app.quick_diff.staging, 0, "nothing staged yet");
    assert!(settle_staged(&mut app, "Staged the change at line 4").await);
    assert!(git(&root, &["diff", "--cached"]).contains("+yline 3"));

    // Lines typed above the change while the stage waits: it keeps to the change's line.
    git(&root, &["reset", "-q"]);
    assert!(settle_markers(&mut app).await);
    place_cursor(&mut app, 3);
    app.press(KeyCode::Char('s'), cmd());
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(app.message.as_ref().unwrap().text.contains("Waiting for the save"));
    place_cursor(&mut app, 0);
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Enter, M::NONE);
    // The save ends; the buffer is dirty again, so it asks, about the change's new line.
    assert!(app.settle(|a| a.prompt.is_some()).await);
    assert!(
        matches!(app.prompt, Some(crate::app::Prompt::SaveBeforeStage { line: 5, .. })),
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    app.press(KeyCode::Char('s'), M::NONE);
    assert!(settle_staged(&mut app, "Staged the change at line 6").await);
    let cached = git(&root, &["diff", "--cached"]);
    assert!(cached.contains("+yline 3") && !cached.contains("+\n"), "{cached}");
    assert_eq!(
        git(&root, &["diff", "--numstat"]),
        "2\t0\tf.txt\n",
        "the new lines not staged"
    );

    // A save that's refused (changed on disk) stages nothing.
    place_cursor(&mut app, 8);
    app.type_text("z");
    assert!(settle_markers(&mut app).await);
    fs::write(
        root.join("f.txt"),
        text.replace("line 3\n", "yline 3\n").replace("line 20\n", "outside\n"),
    )
    .unwrap();
    app.press(KeyCode::Char('s'), cmd());
    assert!(app.active_buffer().unwrap().save_in_flight());
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(app.message.as_ref().unwrap().text.contains("Waiting for the save"));
    assert!(app.settle(|a| !a.active_buffer().unwrap().save_in_flight()).await);
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .text
            .contains("changed on disk: not saved")
    );
    assert!(app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await);
    assert_eq!(app.quick_diff.staging, 0);
    assert!(app.prompt.is_none(), "not asked to save again");
    assert!(!git(&root, &["diff", "--cached"]).contains("zline 8"), "not staged");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_unstage_the_change_at_the_cursor_like_reset_p() {
    let (_d, root, state) = dirs();
    let text = numbered(40);
    let staged = text.replace("line 2\n", "two\n").replace("line 30\n", "thirty\n");
    repo_with(&root, &[("f.txt", &text)], &[("f.txt", &staged)]);
    git(&root, &["add", "f.txt"]);
    // Two more lines on disk at the top, not staged: line 32 is "thirty".
    fs::write(root.join("f.txt"), format!("new\nnew\n{staged}")).unwrap();
    let reference = copy_repo(&root, "reference");
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    place_cursor(&mut app, 10);
    run_command(&mut app, "Git: Unstage Change at Cursor");
    assert!(settle_staged(&mut app, "No staged change at line 11").await);
    place_cursor(&mut app, 32);
    run_command(&mut app, "Git: Unstage Change at Cursor");
    assert!(settle_staged(&mut app, "Unstaged the change at line 33").await);
    git_with_input(&reference, &["reset", "-q", "-p", "--", "f.txt"], Some("n\ny\n"));
    assert_eq!(index_state(&root), index_state(&reference));
    // Now unstaged, it's marked in the gutter.
    assert!(settle_markers(&mut app).await);
    let shown = markers(&mut app);
    assert!(shown.iter().any(|(l, _, _)| *l == 32), "{shown:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_next_and_previous_change() {
    let (_d, root, state) = dirs();
    let text = numbered(30);
    let changed = text
        .replace("line 3\n", "three\n")
        .replace("line 10\n", "")
        .replace("line 20\n", "line 20\nadded\n");
    repo_with(&root, &[("f.txt", &text)], &[("f.txt", &changed)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    let next = |app: &mut App| {
        app.press(KeyCode::F(5), M::ALT);
        cursor_line(app)
    };
    // Changes start on lines 3, 9 (the deletion, below line 9) and 20.
    assert_eq!(next(&mut app), 3);
    assert_eq!(next(&mut app), 9);
    assert_eq!(next(&mut app), 20);
    assert_eq!(next(&mut app), 3, "wraps");
    app.press(KeyCode::F(5), M::ALT | M::SHIFT);
    assert_eq!(cursor_line(&app), 20, "wraps back");
    app.press(KeyCode::F(5), M::ALT | M::SHIFT);
    assert_eq!(cursor_line(&app), 9);
    assert_eq!(
        app.keymap
            .binding_for(crate::command::Command::NextChange)
            .map(|k| k.label())
            .as_deref(),
        Some("Alt+F5")
    );
}

/// Typing at every one of 60k cursors in a 1 MB file with gutter markers: the markers follow
/// each edit (mapped, not recomputed per key) within M6's keystroke budget, and are recomputed
/// off the UI thread after.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_typing_at_many_cursors_with_markers() {
    use std::time::{Duration, Instant};
    let size = if cfg!(debug_assertions) { 250_000 } else { 1_000_000 };
    let mut original = String::new();
    let mut i = 0usize;
    while original.len() < size {
        original.push_str(&format!("let item = value_{i}; // item\n"));
        i += 1;
    }
    // Every tenth line changed on disk: thousands of markers.
    let changed: String = original
        .lines()
        .enumerate()
        .map(|(n, l)| {
            if n % 10 == 0 {
                format!("{l} changed\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    let (_d, root, state) = dirs();
    repo_with(&root, &[("big.rs", &original)], &[("big.rs", &changed)]);
    let mut app = open_file_app(&root, &state, "big.rs");
    assert!(settle_markers(&mut app).await);
    let count = app.active_buffer().unwrap().quick_diff.marks().len();
    assert_eq!(count, i.div_ceil(10));
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Char('l'), cmd() | M::SHIFT);
    let cursors = app.group().active_tab().unwrap().view.selections.len();
    assert!(cfg!(debug_assertions) || cursors > 50_000, "{cursors}");
    let n = 5u32;
    let t = Instant::now();
    for c in "thing".chars() {
        app.press(KeyCode::Char(c), M::NONE);
        app.pump();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let typing = t.elapsed();
    println!(
        "typing at {cursors} cursors with {count} markers: {n} keys in {typing:?}, avg {:?}",
        typing / n
    );
    if !cfg!(debug_assertions) {
        assert!(
            typing / n < Duration::from_millis(16),
            "typing too slow: avg {:?}",
            typing / n
        );
    }
    // Every line now differs from the index.
    assert!(settle_markers(&mut app).await);
    let buf = app.active_buffer().unwrap();
    assert_eq!(
        buf.quick_diff.marks().len(),
        1,
        "one modified run: {:?}",
        buf.quick_diff.marks().len()
    );
}

// 10d: the diff view.

use crate::app::Focus;
use crate::command::Command;
use crate::diff_view::{DiffTab, Pair};

fn diff_tab(app: &App) -> &DiffTab {
    app.group().active_diff().expect("the active tab is a diff")
}

/// Waits until every diff tab is loaded and current and the git status read.
async fn settle_diffs(app: &mut App) -> bool {
    app.settle(|a| a.diffs_settled() && a.git.status.is_some()).await
}

fn draw_sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// Every (x, y) where `needle` starts on screen (cells hold one char each here).
fn find_text(term: &Terminal<TestBackend>, needle: &str) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for (y, row) in lines(term).iter().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        let n: Vec<char> = needle.chars().collect();
        for x in 0..chars.len().saturating_sub(n.len() - 1) {
            if chars[x..x + n.len()] == n[..] {
                out.push((x as u16, y as u16));
            }
        }
    }
    out
}

fn bg(term: &Terminal<TestBackend>, (x, y): (u16, u16)) -> Option<Color> {
    term.backend().buffer()[(x, y)].bg.into()
}

fn theme_bg(app: &App, key: &str) -> Option<Color> {
    app.theme.get(key).bg
}

/// Lines 0..30, with line 2 changed, a line added after line 10 and line 20 deleted.
fn three_changes() -> (String, String) {
    let original = numbered(30);
    let changed = original
        .replace("line 2\n", "line two\n")
        .replace("line 10\n", "line 10\nadded\n")
        .replace("line 20\n", "");
    (original, changed)
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_open_changes_shows_aligned_rows() {
    let (_d, root, state) = dirs();
    let (original, changed) = three_changes();
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    assert_eq!(app.group().tabs.len(), 2, "beside the file's tab");
    assert_eq!(diff_tab(&app).pair, Pair::UNSTAGED);

    // Wide: side by side, the index on the left.
    let term = draw_sized(&mut app, 150, 24);
    let screen = lines(&term);
    assert!(screen[0].contains("f.txt (Working Tree)"), "{:?}", screen[0]);
    assert!(
        screen[1].contains(" Index") && screen[1].contains(" Working Tree") && screen[1].contains("3 changes"),
        "{:?}",
        screen[1]
    );
    assert!(!diff_tab(&app).inline);
    let editor = theme_bg(&app, "editor");
    let (removed, inserted) = (
        theme_bg(&app, "diffEditor.removedLineBackground"),
        theme_bg(&app, "diffEditor.insertedLineBackground"),
    );
    let (removed_text, inserted_text) = (
        theme_bg(&app, "diffEditor.removedTextBackground"),
        theme_bg(&app, "diffEditor.insertedTextBackground"),
    );
    // The changed line beside what it became, numbered on each side; the changed part marked.
    let (x_new, y) = find_text(&term, "line two")[0];
    let (x_old, y_old) = *find_text(&term, "line 2 ").first().expect("line 2 shown");
    assert_eq!(y, y_old, "on one row: {:?}", screen[y as usize]);
    assert!(x_old < x_new);
    assert!(
        screen[y as usize].matches("  3  line").count() == 2,
        "{:?}",
        screen[y as usize]
    );
    assert_eq!(bg(&term, (x_old, y)), removed);
    assert_eq!(bg(&term, (x_old + 5, y)), removed_text, "the 2");
    assert_eq!(bg(&term, (x_new, y)), inserted);
    assert_eq!(bg(&term, (x_new + 5, y)), inserted_text, "the two");
    // The added line against filler, the deleted one too; unchanged lines plain, in step.
    let (x, y) = find_text(&term, "added")[0];
    assert_eq!(bg(&term, (x, y)), inserted);
    let left: String = screen[y as usize].chars().take(90).collect();
    assert!(left.contains("╱╱╱"), "{:?}", screen[y as usize]);
    let (x, y) = find_text(&term, "line 5")[0];
    assert_eq!(bg(&term, (x, y)), editor);
    assert_eq!(find_text(&term, "line 5").len(), 2, "both sides");
    // Past the added line each side has its own numbers: line 15 is 16 on the left, 17 on
    // the right.
    let (_, y) = find_text(&term, "line 15")[0];
    let row = &screen[y as usize];
    let (l, r) = (row.find(" 16  line 15"), row.find(" 17  line 15"));
    assert!(l.is_some() && r.is_some() && l < r, "{row:?}");
    // The cursor starts at the first change; the deletion is further down.
    assert_eq!(diff_tab(&app).cursor, 2);
    app.press(KeyCode::F(7), M::NONE);
    app.press(KeyCode::F(7), M::NONE);
    let term = draw_sized(&mut app, 150, 24);
    let (x, y) = find_text(&term, "line 20")[0];
    assert_eq!(bg(&term, (x, y)), removed);
    let right: String = lines(&term)[y as usize].chars().skip(92).collect();
    assert!(right.contains("╱╱╱"), "{right:?}");
    // Each change has its Stage button.
    assert!(!find_text(&term, " Stage ").is_empty());

    // Side by side from `SIDE_BY_SIDE_MIN` editor columns, one column below.
    let extra = 150
        - app
            .hits
            .group(app.groups.active)
            .unwrap()
            .diff
            .as_ref()
            .unwrap()
            .rows
            .width;
    let min = crate::diff_view::SIDE_BY_SIDE_MIN;
    draw_sized(&mut app, min + extra, 24);
    let width = app
        .hits
        .group(app.groups.active)
        .unwrap()
        .diff
        .as_ref()
        .unwrap()
        .rows
        .width;
    assert_eq!(width, min);
    assert!(!diff_tab(&app).inline);
    draw_sized(&mut app, min + extra - 1, 24);
    assert!(diff_tab(&app).inline);
    draw_sized(&mut app, 150, 24);

    // Narrow: one column, `-` lines above `+` lines, the cursor on the same line.
    let row_before = diff_tab(&app).rows()[diff_tab(&app).cursor];
    let term = draw_sized(&mut app, 80, 24);
    assert!(diff_tab(&app).inline);
    let d = diff_tab(&app);
    assert_eq!(d.rows()[d.cursor].left, row_before.left, "still on the deletion");
    app.press(KeyCode::F(7), M::NONE);
    let term2 = draw_sized(&mut app, 80, 24);
    drop(term);
    let (x, y) = *find_text(&term2, "- line 2").first().expect("- row");
    assert_eq!(bg(&term2, (x + 2, y)), removed);
    let (x2, y2) = find_text(&term2, "+ line two")[0];
    assert_eq!((x2, y2), (x, y + 1), "the new line below");
    assert_eq!(bg(&term2, (x2 + 2, y2)), inserted);
    let row = &lines(&term2)[y as usize];
    assert!(row.contains("  3     - line 2"), "{row:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_diff_view_next_previous_change_and_scrolling() {
    let (_d, root, state) = dirs();
    let (original, changed) = three_changes();
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    draw_sized(&mut app, 150, 24);
    let starts = diff_tab(&app).aligned().starts.clone();
    assert_eq!(starts, [2, 11, 21]);
    let cursor = |app: &App| diff_tab(app).cursor;
    assert_eq!(cursor(&app), 2);
    app.press(KeyCode::F(7), M::NONE);
    assert_eq!(cursor(&app), 11);
    app.press(KeyCode::F(7), M::NONE);
    assert_eq!(cursor(&app), 21);
    app.press(KeyCode::F(7), M::NONE);
    assert_eq!(cursor(&app), 2, "wraps");
    app.press(KeyCode::F(7), M::SHIFT);
    assert_eq!(cursor(&app), 21, "wraps back");
    // VS Code's Alt+F5 / Shift+Alt+F5 too.
    app.press(KeyCode::F(5), M::ALT | M::SHIFT);
    assert_eq!(cursor(&app), 11);
    app.press(KeyCode::F(5), M::ALT);
    assert_eq!(cursor(&app), 21);
    // F7 in a text editor is nothing (VS Code).
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(app.group().active_diff().is_none());
    app.press(KeyCode::F(7), M::NONE);
    assert_eq!(app.last_command, Some(Command::PrevTab), "unbound there");
    app.press(KeyCode::PageDown, M::CONTROL);

    // Up / Down / PageDown / the ends move the cursor; drawing keeps it on screen.
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(cursor(&app), 22);
    app.press(KeyCode::Up, M::NONE);
    app.press(KeyCode::Up, M::NONE);
    assert_eq!(cursor(&app), 20);
    app.test_execute(Command::Move {
        motion: crate::command::Motion::DocStart,
        select: false,
    });
    assert_eq!(cursor(&app), 0);
    draw_sized(&mut app, 150, 12);
    app.press(KeyCode::PageDown, M::NONE);
    let term = draw_sized(&mut app, 150, 12);
    let d = diff_tab(&app);
    assert!(
        d.cursor > 0 && d.cursor >= d.top && d.cursor < d.top + d.height,
        "{} {}",
        d.cursor,
        d.top
    );
    app.test_execute(Command::Move {
        motion: crate::command::Motion::DocEnd,
        select: false,
    });
    let term2 = draw_sized(&mut app, 150, 12);
    let d = diff_tab(&app);
    assert_eq!(d.cursor, d.rows().len() - 1);
    assert_eq!(d.top + d.height, d.rows().len(), "the last page full");
    assert!(lines(&term2).iter().any(|r| r.contains("line 29")));
    drop(term);
    // The wheel scrolls without moving the cursor; a click puts it on a row.
    let rows = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap().rows;
    app.mouse(MouseEventKind::ScrollUp, rows.x + 10, rows.y + 1);
    let d = diff_tab(&app);
    assert_eq!(d.top + d.height + 3, d.rows().len());
    assert_eq!(d.cursor, d.rows().len() - 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), rows.x + 10, rows.y + 1);
    let d = diff_tab(&app);
    assert_eq!(d.cursor, d.top + 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_stage_from_the_diff_view_like_add_p() {
    let (_d, root, state) = dirs();
    let original = numbered(40);
    let changed = original
        .replace("line 2\n", "two\n")
        .replace("line 20\n", "twenty\nmore\n")
        .replace("line 35\n", "");
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let reference = copy_repo(&root, "reference");
    // The file open: staged through the gutter's path (from disk once saved).
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_markers(&mut app).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    app.press(KeyCode::F(7), M::NONE);
    assert_eq!(diff_tab(&app).hunk_at_cursor(), Some(1));
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(settle_staged(&mut app, "Staged the change at line 21").await);
    git_with_input(&reference, &["add", "-p", "--", "f.txt"], Some("n\ny\nn\n"));
    assert_eq!(index_state(&root), index_state(&reference));
    // The view follows the index: two changes left. Unstaging here isn't this view's.
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff_tab(&app).doc.as_ref().unwrap().hunks.len(), 2);
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(app.message.as_ref().unwrap().text.contains("No change at the cursor"));
    app.press(KeyCode::F(7), M::NONE);
    run_command(&mut app, "Git: Unstage Change at Cursor");
    assert!(app.message.as_ref().unwrap().text.contains("aren't staged"));

    // The deletion's Stage button.
    let k = diff_tab(&app).hunk_at_cursor().unwrap();
    assert_eq!(k, 1);
    draw_sized(&mut app, 150, 24);
    let hit = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap();
    let (rect, _) = *hit.buttons.iter().find(|(_, h)| *h == k).expect("its button");
    app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, rect.y);
    assert!(settle_staged(&mut app, "Staged the change at line 36").await);
    git(&reference, &["reset", "-q"]);
    git_with_input(&reference, &["add", "-p", "--", "f.txt"], Some("n\ny\ny\n"));
    assert_eq!(index_state(&root), index_state(&reference));

    // A dirty buffer is saved first (asked), as from the gutter.
    assert!(settle_diffs(&mut app).await);
    app.press(KeyCode::PageUp, M::CONTROL);
    place_cursor(&mut app, 2);
    app.type_text("x");
    app.press(KeyCode::PageDown, M::CONTROL);
    assert!(settle_diffs(&mut app).await);
    assert!(
        diff_tab(&app).doc.as_ref().unwrap().right.line(2).starts_with("xtwo"),
        "the buffer's text shows"
    );
    assert!(settle_markers_of(&mut app).await);
    app.press(KeyCode::F(7), M::NONE);
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(
        matches!(app.prompt, Some(crate::app::Prompt::SaveBeforeStage { line: 2, .. })),
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    app.press(KeyCode::Char('s'), M::NONE);
    assert!(settle_staged(&mut app, "Staged the change at line 3").await);
    assert_eq!(git(&root, &["diff"]), "", "everything staged");
    assert!(git(&root, &["diff", "--cached"]).contains("+xtwo"));

    // Stage File from the diff tab of a dirty open file: saved first (asked), then all of it.
    assert!(settle_diffs(&mut app).await);
    app.press(KeyCode::PageUp, M::CONTROL);
    place_cursor(&mut app, 10);
    app.type_text("y");
    place_cursor(&mut app, 30);
    app.type_text("z");
    app.press(KeyCode::PageDown, M::CONTROL);
    assert!(diff_tab(&app).pair == Pair::UNSTAGED);
    run_command(&mut app, "Git: Stage File");
    assert!(
        matches!(app.prompt, Some(crate::app::Prompt::SaveBeforeStage { file: true, .. })),
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    app.press(KeyCode::Char('s'), M::NONE);
    assert!(settle_staged(&mut app, "Staged f.txt").await);
    assert!(!app.buffers.iter().next().unwrap().1.is_dirty());
    assert_eq!(git(&root, &["diff"]), "", "the whole file staged");
    let cached = git(&root, &["diff", "--cached"]);
    assert!(cached.contains("+yline 10") && cached.contains("+zline 29"), "{cached}");
}

/// `settle_markers` for the file open in the first tab while a diff tab is active.
async fn settle_markers_of(app: &mut App) -> bool {
    app.settle(|a| {
        a.buffers.iter().all(|(_, b)| {
            !b.quick_diff.running() && b.quick_diff.want(b.revision()) == crate::quick_diff::Want::Nothing
        }) && a.quick_diff.deadline().is_none()
            && !a.git.running()
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_stage_from_the_diff_of_a_file_not_open() {
    let (_d, root, state) = dirs();
    let original = numbered(40);
    let changed = original.replace("line 2\n", "two\n").replace("line 30\n", "thirty\n");
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let reference = copy_repo(&root, "reference");
    let mut app = open_app(&root, &state);
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    app.open_diff(root.join("f.txt"), Pair::UNSTAGED);
    assert!(settle_diffs(&mut app).await);
    assert_eq!(app.buffers.iter().count(), 0, "not opened in a buffer");
    // The diff tab is the only tab: Esc from the Search view and the Problems list goes back
    // to it.
    app.press(KeyCode::Char('f'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, Focus::SidePanel);
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(app.ui.focus, Focus::Editor, "Search view");
    app.press(KeyCode::Char('m'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, Focus::Panel);
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(app.ui.focus, Focus::Editor, "Problems list");
    app.press(KeyCode::Char('m'), cmd() | M::SHIFT);
    app.press(KeyCode::Char('m'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, Focus::Editor, "hiding the panel");

    // The file changed on disk since the diff was read: staging what's shown is refused.
    let shown = diff_tab(&app).doc.clone().unwrap();
    fs::write(root.join("f.txt"), changed.replace("line 10\n", "ten\n")).unwrap();
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(app.settle(|a| a.diffs.staging == 0 && a.message.is_some()).await);
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Staging failed: the file changed, try again"
    );
    assert_eq!(git(&root, &["diff", "--cached"]), "", "nothing staged");
    assert!(
        app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len() == 3)
            .await,
        "read again"
    );
    assert!(!std::sync::Arc::ptr_eq(&shown, diff_tab(&app).doc.as_ref().unwrap()));
    fs::write(root.join("f.txt"), &changed).unwrap();
    assert!(
        app.settle(|a| a.diffs_settled() && a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len() == 2)
            .await
    );
    // The first change's button: exactly that hunk, as `git add -p`.
    draw_sized(&mut app, 150, 24);
    let hit = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap();
    let (rect, _) = *hit.buttons.iter().find(|(_, h)| *h == 0).expect("its button");
    app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, rect.y);
    assert!(app.settle(|a| a.diffs.staging == 0 && a.message.is_some()).await);
    assert_eq!(app.message.as_ref().unwrap().text, "Staged the change at line 3");
    git_with_input(&reference, &["add", "-p", "--", "f.txt"], Some("y\nn\n"));
    assert_eq!(index_state(&root), index_state(&reference));
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff_tab(&app).doc.as_ref().unwrap().hunks.len(), 1);

    // The whole file, staged and unstaged (palette).
    run_command(&mut app, "Git: Stage File");
    assert!(
        app.settle(|a| a.diffs.staging == 0 && a.message.as_ref().is_some_and(|m| m.text == "Staged f.txt"))
            .await
    );
    assert!(settle_diffs(&mut app).await);
    assert!(diff_tab(&app).doc.as_ref().unwrap().hunks.is_empty());
    assert_eq!(git(&root, &["diff"]), "");
    run_command(&mut app, "Git: Unstage File");
    assert!(
        app.settle(|a| a.diffs.staging == 0 && a.message.as_ref().is_some_and(|m| m.text == "Unstaged f.txt"))
            .await
    );
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff_tab(&app).doc.as_ref().unwrap().hunks.len(), 2);
    assert_eq!(git(&root, &["diff", "--cached"]), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_unstage_from_the_staged_view_like_reset_p() {
    let (_d, root, state) = dirs();
    let text = numbered(40);
    let staged = text.replace("line 2\n", "two\n").replace("line 30\n", "thirty\n");
    repo_with(&root, &[("f.txt", &text)], &[("f.txt", &staged)]);
    git(&root, &["add", "f.txt"]);
    let reference = copy_repo(&root, "reference");
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    // Only staged changes: Open Changes shows the index against HEAD.
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff_tab(&app).pair, Pair::STAGED);
    let screen = lines(&draw_sized(&mut app, 150, 24));
    assert!(screen[0].contains("f.txt (Index)"), "{:?}", screen[0]);
    assert!(screen[1].contains(" HEAD") && screen[1].contains(" Index"));
    assert_eq!(find_text(&draw_sized(&mut app, 150, 50), " Unstage ").len(), 2);
    run_command(&mut app, "Git: Stage Change at Cursor");
    assert!(app.message.as_ref().unwrap().text.contains("already staged"));
    app.press(KeyCode::F(7), M::NONE);
    run_command(&mut app, "Git: Unstage Change at Cursor");
    assert!(
        app.settle(|a| a.diffs.staging == 0 && a.message.as_ref().is_some_and(|m| m.text.starts_with("Unstaged")))
            .await
    );
    assert_eq!(app.message.as_ref().unwrap().text, "Unstaged the change at line 31");
    git_with_input(&reference, &["reset", "-q", "-p", "--", "f.txt"], Some("n\ny\n"));
    assert_eq!(index_state(&root), index_state(&reference));
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff_tab(&app).doc.as_ref().unwrap().hunks.len(), 1);
    // Now it has unstaged changes too: Open Changes from the file shows those.
    app.press(KeyCode::PageUp, M::CONTROL);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff_tab(&app).pair, Pair::UNSTAGED);
    assert_eq!(app.group().tabs.len(), 3);
    // Asked again: the same tab.
    app.press(KeyCode::PageUp, M::CONTROL);
    app.press(KeyCode::PageUp, M::CONTROL);
    run_command(&mut app, "Git: Open Changes");
    assert_eq!(app.group().tabs.len(), 3);
    assert_eq!(diff_tab(&app).pair, Pair::UNSTAGED);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_diff_view_refreshes() {
    let (_d, root, state) = dirs();
    let (original, changed) = three_changes();
    repo_with(
        &root,
        &[("f.txt", &original), ("g.txt", &original)],
        &[("f.txt", &changed), ("g.txt", &changed)],
    );
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_git(&mut app, |s| s.entries.len() == 2).await);
    // A file not open: changed outside red, staged outside red.
    app.open_diff(root.join("g.txt"), Pair::UNSTAGED);
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff_tab(&app).doc.as_ref().unwrap().hunks.len(), 3);
    fs::write(root.join("g.txt"), changed.replace("line 25\n", "outside\n")).unwrap();
    assert!(
        app.settle(|a| a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.len() == 4)
            .await,
        "the file's change"
    );
    git(&root, &["add", "g.txt"]);
    assert!(
        app.settle(|a| a.group().active_diff().unwrap().doc.as_ref().unwrap().hunks.is_empty())
            .await,
        "the index's change"
    );
    let term = draw_sized(&mut app, 150, 24);
    assert!(lines(&term)[1].contains("0 changes"), "{:?}", lines(&term)[1]);

    // The open file: its buffer's text shows, edits and all, without saving.
    app.press(KeyCode::PageUp, M::CONTROL);
    assert!(app.active_buffer().is_some());
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    let revision = |a: &App| a.buffers.iter().next().unwrap().1.revision();
    let loaded = diff_tab(&app).doc.clone().unwrap();
    app.press(KeyCode::PageUp, M::CONTROL);
    assert_eq!(app.active_buffer().unwrap().path, root.join("f.txt"));
    app.type_text("typed ");
    app.pump();
    // Not reloaded at each key: after the delay.
    assert!(app.diffs.deadline().is_some());
    let before = revision(&app);
    assert!(settle_diffs(&mut app).await);
    assert_eq!(revision(&app), before);
    app.press(KeyCode::PageDown, M::CONTROL);
    let d = diff_tab(&app);
    assert!(d.pair == Pair::UNSTAGED && d.path == root.join("f.txt"));
    let doc = d.doc.clone().unwrap();
    assert!(!std::sync::Arc::ptr_eq(&doc, &loaded));
    assert_eq!(doc.right.line(0), "typed line 0");
    assert_eq!(doc.hunks.len(), 4);
    assert!(app.active_buffer().is_none() && app.buffers.iter().next().unwrap().1.is_dirty());

    // Closing the diff tab of a dirty file asks nothing and leaves the buffer as it was.
    app.press(KeyCode::Char('w'), cmd());
    assert!(app.prompt.is_none());
    assert_eq!(app.group().tabs.len(), 2, "the file's tab and g.txt's diff");
    let buf = app.buffers.iter().next().unwrap().1;
    assert!(buf.is_dirty() && buf.rope.to_string().starts_with("typed line 0"));
    // Save from a diff tab saves its file's buffer.
    app.press(KeyCode::PageUp, M::CONTROL);
    assert_eq!(app.active_buffer().unwrap().path, root.join("f.txt"));
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    app.press(KeyCode::Char('s'), cmd());
    assert!(app.settle(|a| !a.buffers.iter().next().unwrap().1.is_dirty()).await);
    assert!(
        fs::read_to_string(root.join("f.txt"))
            .unwrap()
            .starts_with("typed line 0")
    );
    assert!(app.group().active_diff().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_split_and_close_a_diff_tab() {
    let (_d, root, state) = dirs();
    let (original, changed) = three_changes();
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    app.press(KeyCode::F(7), M::NONE);
    let (first, cursor) = (diff_tab(&app).id, diff_tab(&app).cursor);
    app.press(KeyCode::Char('\\'), cmd());
    assert_eq!(app.groups.len(), 2);
    let d = diff_tab(&app);
    assert!(
        d.id != first && d.pair == Pair::UNSTAGED && d.cursor == cursor,
        "a copy, in the same place"
    );
    assert!(d.doc.is_some());
    let screen = lines(&draw_sized(&mut app, 200, 24));
    assert_eq!(screen[0].matches("f.txt (Working Tree)").count(), 2, "{:?}", screen[0]);
    // Both follow the file.
    fs::write(root.join("f.txt"), &original).unwrap();
    assert!(
        app.settle(|a| {
            a.groups.ids().iter().all(|g| {
                let d = a.groups.get(*g).unwrap().active_diff().unwrap();
                d.doc.as_ref().is_some_and(|doc| doc.hunks.is_empty())
            })
        })
        .await,
        "both reloaded"
    );
    // Moving in one leaves the other.
    app.press(KeyCode::Down, M::NONE);
    // Closing it closes its group; the other's close shows the file again.
    app.press(KeyCode::Char('w'), cmd());
    assert_eq!(app.groups.len(), 1);
    assert!(app.group().active_diff().is_some());
    app.press(KeyCode::Char('w'), cmd());
    assert!(app.prompt.is_none(), "no save prompt");
    assert_eq!(app.group().tabs.len(), 1);
    assert_eq!(app.active_buffer().unwrap().path, root.join("f.txt"));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_editor_commands_on_a_diff_tab_do_nothing() {
    let (_d, root, state) = dirs();
    let (original, changed) = three_changes();
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    app.log_dir = state.join("logs");
    app.config_dir = state.join("config");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    let diff = app.group().active;
    let skip = |c: &Command| {
        matches!(
            c,
            Command::Quit
                | Command::CloseTab
                | Command::NextTab
                | Command::PrevTab
                | Command::StageFile
                | Command::UnstageFile
                | Command::StageHunk
                | Command::UnstageHunk
                | Command::OpenUserSettings
                | Command::OpenLogFile
        )
    };
    for c in Command::all().into_iter().filter(|c| !skip(c)) {
        app.test_execute(c);
        app.prompt = None;
        app.palette = None;
        // Back to the diff tab in the first group, focused.
        while app.groups.len() > 1 {
            app.test_execute(Command::FocusGroup(2));
            app.test_execute(Command::CloseTab);
        }
        app.group_mut().active = diff;
        app.test_execute(Command::FocusGroup(1));
        draw_sized(&mut app, 150, 24);
        draw_sized(&mut app, 70, 24);
    }
    // Typed text, paste, mouse in the text: nothing reaches the buffer.
    app.type_text("abc");
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Backspace, M::NONE);
    app.handle_test_event(crossterm::event::Event::Paste("pasted".into()));
    let rows = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap().rows;
    app.mouse(MouseEventKind::Down(MouseButton::Left), rows.x + 30, rows.y + 3);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), rows.x + 40, rows.y + 5);
    app.mouse(MouseEventKind::Up(MouseButton::Left), rows.x + 40, rows.y + 5);
    app.mouse(MouseEventKind::Moved, rows.x + 20, rows.y + 4);
    app.test_hover_tick(std::time::Instant::now() + crate::hover::DELAY);
    assert!(app.settle(|a| !a.buffers.iter().any(|(_, b)| b.save_in_flight())).await);
    let (_, buf) = app.buffers.iter().next().unwrap();
    assert_eq!(buf.rope.to_string(), changed, "the buffer untouched");
    assert!(!buf.is_dirty());
    assert!(app.group().active_diff().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_open_changes_of_a_clean_or_untracked_file() {
    let (_d, root, state) = dirs();
    repo_with(&root, &[("f.txt", "one\n")], &[("new.txt", "a\nb\n")]);
    // No changes: a message, no tab.
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    run_command(&mut app, "Git: Open Changes");
    assert_eq!(app.message.as_ref().unwrap().text, "f.txt has no changes");
    assert_eq!(app.group().tabs.len(), 1);
    assert!(app.group().active_diff().is_none());
    drop(app);
    // Untracked: every line added against an empty index side.
    let mut app = open_file_app(&root, &state, "new.txt");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    let d = diff_tab(&app);
    assert_eq!(d.pair, Pair::UNSTAGED);
    let doc = d.doc.as_ref().unwrap();
    assert_eq!((doc.left.len(), doc.right.len(), doc.hunks.len()), (0, 2, 1));
    let term = draw_sized(&mut app, 150, 24);
    let (x, y) = *find_text(&term, "  1  a").first().expect("line a, numbered 1");
    assert_eq!(
        bg(&term, (x + 5, y)),
        theme_bg(&app, "diffEditor.insertedLineBackground")
    );
    let before: String = lines(&term)[y as usize].chars().take(x as usize).collect();
    assert!(before.contains("╱╱╱"), "the index side is filler: {before:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_banner_show_diff() {
    let (_d, root, state) = dirs();
    let path = root.join("a.txt");
    fs::write(&path, "one\ntwo\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    app.type_text("mine ");
    fs::write(&path, "theirs\ntwo\n").unwrap();
    assert!(app.settle(|a| a.active_buffer().unwrap().disk_conflict.is_some()).await);
    let term = draw_sized(&mut app, 150, 24);
    assert!(find_text(&term, " Show Diff ").len() == 1);
    let button = |app: &App, action| {
        let b = app.hits.groups[0].banner.as_ref().expect("banner drawn");
        b.actions.iter().find(|(_, a)| *a == action).unwrap().0
    };
    let r = button(&app, BannerAction::ShowDiff);
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    assert_eq!(diff_tab(&app).pair, Pair::SAVED);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let term = draw_sized(&mut app, 150, 24);
    let screen = lines(&term);
    assert!(screen[0].contains("a.txt (On Disk ↔ Yours)"), "{:?}", screen[0]);
    assert!(
        screen[1].contains("has changed on disk"),
        "the banner over the diff too: {screen:#?}"
    );
    assert!(screen[2].contains(" On Disk") && screen[2].contains(" Yours"));
    let (x, y) = find_text(&term, "mine one")[0];
    assert_eq!(find_text(&term, "theirs")[0].1, y, "side by side");
    assert_eq!(
        bg(&term, (x, y)),
        theme_bg(&app, "diffEditor.insertedTextBackground"),
        "mine"
    );
    assert_eq!(
        bg(&term, (x + 9, y)),
        theme_bg(&app, "diffEditor.insertedLineBackground"),
        "after it"
    );
    assert!(find_text(&term, " Stage ").is_empty(), "nothing to stage here");
    // Not in a repository: no git diff to open.
    run_command(&mut app, "Git: Open Changes");
    assert_eq!(app.message.as_ref().unwrap().text, "Not in a git repository");
    assert_eq!(app.group().tabs.len(), 2);
    // The file changes on disk again: the diff reads it again (no git status to say so).
    fs::write(&path, "theirs again\ntwo\n").unwrap();
    assert!(
        app.settle(|a| a.diffs_settled() && diff_tab(a).doc.as_ref().unwrap().left.line(0) == "theirs again")
            .await
    );
    // The banner's Reload from the diff tab: the buffer takes the file, the diff empties.
    draw_sized(&mut app, 150, 24);
    let r = button(&app, BannerAction::Reload);
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    assert!(
        app.settle(|a| a.buffers.iter().next().unwrap().1.rope == "theirs again\ntwo\n")
            .await
    );
    assert!(
        app.settle(|a| a.diffs_settled() && diff_tab(a).doc.as_ref().unwrap().hunks.is_empty())
            .await
    );
    assert!(
        !lines(&draw_sized(&mut app, 150, 24))
            .iter()
            .any(|r| r.contains("has changed on disk"))
    );
}

/// A diff of a 1 MB file with thousands of changes opens off the UI thread and scrolls within
/// a frame (release).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_10_diff_of_a_big_file() {
    use std::time::{Duration, Instant};
    let size = if cfg!(debug_assertions) { 250_000 } else { 1_000_000 };
    let mut original = String::new();
    let mut i = 0usize;
    while original.len() < size {
        original.push_str(&format!("let item = value_{i}; // item\n"));
        i += 1;
    }
    let changed: String = original
        .lines()
        .enumerate()
        .filter(|(n, _)| n % 17 != 5)
        .map(|(n, l)| {
            if n % 10 == 0 {
                format!("{l} changed\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    let (_d, root, state) = dirs();
    repo_with(&root, &[("big.rs", &original)], &[("big.rs", &changed)]);
    let mut app = open_app(&root, &state);
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    let t = Instant::now();
    app.open_diff(root.join("big.rs"), Pair::UNSTAGED);
    let opened = t.elapsed();
    assert!(settle_diffs(&mut app).await);
    let loaded = t.elapsed();
    let hunks = diff_tab(&app).doc.as_ref().unwrap().hunks.len();
    let mut term = Terminal::new(TestBackend::new(200, 50)).unwrap();
    let n = 40u32;
    let t = Instant::now();
    for _ in 0..n {
        app.press(KeyCode::PageDown, M::NONE);
        app.press(KeyCode::F(7), M::NONE);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    }
    let frames = t.elapsed();
    println!(
        "diff of {} bytes, {hunks} changes: open {opened:?}, loaded in {loaded:?}, {n} frames avg {:?}",
        original.len(),
        frames / n
    );
    assert!(diff_tab(&app).cursor > 0);
    assert!(
        opened < Duration::from_millis(50),
        "the UI thread doesn't diff: {opened:?}"
    );
    if !cfg!(debug_assertions) {
        assert!(loaded < Duration::from_secs(2), "slow load: {loaded:?}");
        assert!(frames / n < Duration::from_millis(16), "slow frames: {:?}", frames / n);
    }
}
