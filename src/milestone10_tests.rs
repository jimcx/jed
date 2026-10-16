//! Milestone 10 end to end (so far 10a): the git status read off-thread at startup and kept
//! current through `notify` (work tree edits, staging and branch switches made outside red),
//! the branch in the status bar (unborn and detached HEAD too), git colouring in the explorer
//! (changed files and the folders holding them), no repository (git off), and a workspace
//! that is a subdirectory of the work tree (its `.git` watched separately).

use std::fs;
use std::path::{Path, PathBuf};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::app::App;
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
