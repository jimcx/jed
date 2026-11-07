//! End-to-end tests for the Code Review task (`mr_review`). So far mr_review-b, the panel: the
//! activity bar's icon and the panel (no key by default; "Code Review: Show" in the palette); the
//! merge request of the checked-out branch found through a local fake forge (`forge::fake`) on a
//! `git::test_repo` repository; the summary and the FILES list (status letters with renames,
//! `+`/`-` counts) from the local diff of the merge base to the working tree; the viewed marks
//! (local, in the state directory, cleared when a file's content changes); several merge requests
//! with the newest first and a switcher; the empty states (no repository, no remote, unknown host,
//! no token, detached HEAD, no merge request); missing objects ("run `git fetch`"); a failed
//! lookup keeping the last result under its error and in the tool log; lookups at launch, when
//! HEAD moves and on a timer that only runs while the panel is shown.

mod claude;
mod done_when;
mod github;
mod gitlab;
mod suggest;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::command::Command;
use crate::forge::Kind;
use crate::forge::config::{Entry, ForgeConfig};
use crate::forge::fake::{FakeServer, Reply};
use crate::git::test_repo::{commit_all, git, init};
use crate::logs::Tool;
use crate::review::{Content, Empty};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

const REMOTE: &str = "ssh://git@git.example.com:2200/alice/project.git";
const PULLS: &str = "/api/v1/repos/alice/project/pulls";

fn pr_json(number: u64, title: &str, merge_base: &str) -> String {
    format!(
        r#"{{"number":{number},"title":"{title}","body":"","state":"open","draft":false,"merged":false,
        "user":{{"login":"ann"}},"html_url":"https://git.example.com/alice/project/pulls/{number}",
        "merge_base":"{merge_base}",
        "base":{{"ref":"main","sha":"{merge_base}","repo":{{"full_name":"alice/project"}}}},
        "head":{{"ref":"feature","sha":"h0","repo":{{"full_name":"alice/project"}}}}}}"#
    )
}

/// `<base>/proj`: `main` with `a.txt`, `gone.txt`, `mod.txt`, then `feature` (checked out) renaming
/// `a.txt`, editing `mod.txt`, adding `new.txt`, deleting `gone.txt`. Returns the repository and
/// main's commit id.
fn make_repo(base: &Path) -> (PathBuf, String) {
    let root = base.join("proj");
    fs::create_dir_all(&root).unwrap();
    init(&root);
    fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\nfive\nsix\n").unwrap();
    fs::write(root.join("gone.txt"), "x\n").unwrap();
    fs::write(root.join("mod.txt"), "m1\nm2\n").unwrap();
    commit_all(&root, "base");
    let main = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&root, &["remote", "add", "origin", REMOTE]);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    git(&root, &["mv", "a.txt", "renamed.txt"]);
    fs::write(root.join("renamed.txt"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\n").unwrap();
    fs::remove_file(root.join("gone.txt")).unwrap();
    fs::write(root.join("mod.txt"), "m1\nm2 changed\nm3\n").unwrap();
    fs::write(root.join("new.txt"), "n\n").unwrap();
    commit_all(&root, "work");
    (root, main)
}

/// The forge config for a fake server.
fn config_for(server: &FakeServer) -> ForgeConfig {
    ForgeConfig {
        entries: vec![Entry {
            host: "git.example.com".into(),
            kind: Kind::Gitea,
            api_url: Some(format!("{}/api/v1", server.url())),
            token_env: Some("FAKE_TOKEN".into()),
            token: None,
            mcp_read_only: false,
        }],
        errors: Vec::new(),
    }
}

/// An app on `root` that looks merge requests up in `config`, with the token `secret` (or none).
fn open_app(root: &Path, state: Option<PathBuf>, config: ForgeConfig, token: bool) -> App {
    let base = workspace::normalize(root);
    let trust = TrustStore::load(base.join("trust.toml"));
    let mut app = App::new(Workspace::load(base), trust, None, true);
    app.state_dir = state;
    app.review.config = Some(Arc::new(config));
    app.review.env = Arc::new(move |n| (token && n == "FAKE_TOKEN").then(|| "secret".to_string()));
    app.review.auto = true;
    app.test_start();
    app
}

/// A fake forge with one open merge request `!7` whose merge base is `main`'s commit.
fn one_mr(main: &str) -> FakeServer {
    FakeServer::start(vec![
        (
            format!("{PULLS}?state=open&page=1&limit=50"),
            Reply::json(&format!("[{}]", pr_json(7, "Add review panel", main))),
        ),
        (format!("{PULLS}/7"), Reply::json(&pr_json(7, "Add review panel", main))),
    ])
}

fn loaded(app: &App) -> bool {
    app.review.review().is_some()
}

fn lookups(server: &FakeServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|r| r.target.starts_with(&format!("{PULLS}?")))
        .count()
}

fn screen(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(70, 24)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..24)
        .map(|y| (0..70).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn idle(app: &mut App) {
    assert!(app.settle(|a| !a.review.loading).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_finds_the_merge_request_and_lists_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    let review = app.review.review().unwrap();
    assert_eq!((review.mr.number, review.mr.title.as_str()), (7, "Add review panel"));
    let file = |p: &str| {
        review
            .files
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("{p}"))
    };
    assert_eq!(review.files.len(), 4, "{:?}", review.files);
    assert_eq!(file("renamed.txt").status, 'R');
    assert_eq!(file("renamed.txt").old_path.as_deref(), Some("a.txt"));
    assert_eq!(file("renamed.txt").additions, 1);
    assert_eq!(file("mod.txt").status, 'M');
    assert_eq!(file("new.txt").status, 'A');
    assert_eq!(file("gone.txt").status, 'D');
    assert_eq!(server.requests()[0].header("authorization"), Some("token secret"));
    assert!(app.logs.texts(&Tool::Review).iter().any(|l| l.contains("Found !7")));

    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    let text = screen(&mut app);
    assert!(text.contains("CODE REVIEW"), "{text}");
    assert!(text.contains("⇄"), "the activity bar's icon: {text}");
    assert!(text.contains("!7 Add review panel"), "{text}");
    assert!(text.contains("feature → main"), "{text}");
    assert!(text.contains("4 files · open"), "{text}");
    assert!(text.contains("▾ FILES"), "{text}");
    assert!(text.contains("R renamed.txt"), "{text}");
    assert!(text.contains("D gone.txt"), "{text}");
    assert!(text.contains("+1"), "{text}");
    assert!(!text.contains("secret"), "{text}");
    assert!(app.logs.texts(&Tool::Review).iter().all(|l| !l.contains("secret")));
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_viewed_marks_are_local_and_clear_when_a_file_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let server = one_mr(&main);
    let state = dir.path().join("state");
    let mut app = open_app(&root, Some(state.clone()), config_for(&server), true);
    assert!(app.settle(loaded).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    let files = app.review.files().to_vec();
    app.review.view.select_path(&files, "mod.txt");
    assert_eq!(app.review.viewed_count(), 0);
    app.press(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.review.viewed_count(), 1);
    assert!(screen(&mut app).contains("✓"));
    assert!(screen(&mut app).contains("1 viewed"));
    let saved = state.join("review-viewed.json");
    assert!(app.settle(|_| saved.exists()).await);
    assert!(fs::read_to_string(&saved).unwrap().contains("alice/project#7:mod.txt"));

    // A new session reads the mark back.
    let mut second = open_app(&root, Some(state.clone()), config_for(&server), true);
    assert!(second.settle(loaded).await);
    assert_eq!(second.review.viewed_count(), 1);

    // Another change to the file clears it, and the next write forgets it.
    fs::write(root.join("mod.txt"), "different\n").unwrap();
    app.test_execute(Command::ReviewRefresh);
    idle(&mut app).await;
    assert_eq!(app.review.viewed_count(), 0);
    assert!(
        app.settle(|_| !fs::read_to_string(&saved).unwrap().contains("mod.txt"))
            .await
    );

    // Space again un-marks.
    app.review.view.select_path(&files, "mod.txt");
    app.press(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.review.viewed_count(), 1);
    app.press(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.review.viewed_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_empty_states_say_why() {
    // No repository.
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let mut app = open_app(&plain, None, ForgeConfig::default(), true);
    app.test_execute(Command::ShowReview);
    assert_eq!(app.review.content, Content::Empty(Empty::NoRepo));
    assert!(screen(&mut app).contains("No git repository."));

    let (root, main) = make_repo(dir.path());
    // A host that is not in forges.toml.
    let mut app = open_app(&root, None, ForgeConfig::default(), true);
    app.test_execute(Command::ShowReview);
    let want = Content::Empty(Empty::UnknownHost("git.example.com".into()));
    assert!(app.settle(|a| a.review.content == want).await);
    assert!(screen(&mut app).contains("configured"));

    // No token in the environment.
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), false);
    app.test_execute(Command::ShowReview);
    let want = Content::Empty(Empty::NoToken("FAKE_TOKEN".into()));
    assert!(app.settle(|a| a.review.content == want).await);
    assert!(screen(&mut app).contains("No token: set FAKE_TOKEN"));
    assert!(server.requests().is_empty(), "nothing is asked without a token");

    // No open merge request.
    let none = FakeServer::start(vec![(format!("{PULLS}?state=open&page=1&limit=50"), Reply::json("[]"))]);
    let mut app = open_app(&root, None, config_for(&none), true);
    app.test_execute(Command::ShowReview);
    let want = Content::Empty(Empty::NoMr("feature".into()));
    assert!(app.settle(|a| a.review.content == want).await);
    assert!(screen(&mut app).contains("feature."));

    // No remote.
    git(&root, &["remote", "remove", "origin"]);
    let mut app = open_app(&root, None, config_for(&server), true);
    app.test_execute(Command::ShowReview);
    assert!(
        app.settle(|a| a.review.content == Content::Empty(Empty::NoRemote))
            .await
    );

    // A detached HEAD.
    git(&root, &["remote", "add", "origin", REMOTE]);
    git(&root, &["checkout", "-q", "--detach"]);
    let mut app = open_app(&root, None, config_for(&server), true);
    app.test_execute(Command::ShowReview);
    assert!(
        app.settle(|a| a.review.content == Content::Empty(Empty::Detached))
            .await
    );
    assert!(screen(&mut app).contains("No branch is checked out."));
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_several_merge_requests_show_the_newest_and_switch() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let server = FakeServer::start(vec![
        (
            format!("{PULLS}?state=open&page=1&limit=50"),
            Reply::json(&format!(
                "[{},{}]",
                pr_json(7, "Old one", &main),
                pr_json(9, "Newer one", &main)
            )),
        ),
        (format!("{PULLS}/7"), Reply::json(&pr_json(7, "Old one", &main))),
        (format!("{PULLS}/9"), Reply::json(&pr_json(9, "Newer one", &main))),
    ]);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    assert_eq!(app.review.review().unwrap().mr.number, 9);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    let text = screen(&mut app);
    assert!(text.contains("!9 Newer one ‹1/2›"), "{text}");

    app.test_execute(Command::ReviewSwitch);
    assert!(
        app.settle(|a| a.review.review().is_some_and(|r| r.mr.number == 7) && !a.review.loading)
            .await
    );
    assert!(screen(&mut app).contains("!7 Old one ‹2/2›"));
    // A refresh keeps the choice; clicking the summary switches back.
    app.test_execute(Command::ReviewRefresh);
    idle(&mut app).await;
    assert_eq!(app.review.review().unwrap().mr.number, 7);
    screen(&mut app);
    let summary = app.hits.review.clone().unwrap().summary;
    app.mouse(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        summary.x + 3,
        summary.y,
    );
    assert!(
        app.settle(|a| a.review.review().is_some_and(|r| r.mr.number == 9) && !a.review.loading)
            .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_a_failed_lookup_keeps_the_last_result() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    // The server now answers 404 to everything.
    let broken = FakeServer::start(vec![]);
    app.review.config = Some(Arc::new(config_for(&broken)));
    app.test_execute(Command::ReviewRefresh);
    assert!(app.settle(|a| a.review.error.is_some()).await);
    idle(&mut app).await;
    assert!(app.review.review().is_some(), "the last result stays");
    let text = screen(&mut app);
    assert!(text.contains("server answered 404"), "{text}");
    assert!(text.contains("!7 Add review panel"), "{text}");
    assert!(app.logs.texts(&Tool::Review).iter().any(|l| l.contains("404")));
    assert!(!text.contains("secret"), "the token never reaches the panel");
    assert!(
        app.logs.texts(&Tool::Review).iter().all(|l| !l.contains("secret")),
        "nor the tool log"
    );
    // The next good answer clears the error.
    app.review.config = Some(Arc::new(config_for(&server)));
    app.test_execute(Command::ReviewRefresh);
    assert!(app.settle(|a| a.review.error.is_none() && !a.review.loading).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_missing_objects_say_to_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let (root, _main) = make_repo(dir.path());
    let server = one_mr("1234567890123456789012345678901234567890");
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    let review = app.review.review().unwrap();
    assert!(review.files.is_empty());
    assert!(review.files_error.as_deref().unwrap().contains("git fetch"));
    let text = screen(&mut app);
    assert!(text.contains("!7 Add review panel"), "{text}");
    assert!(text.contains("git fetch"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_looks_again_when_head_moves_and_polls_only_while_shown() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    idle(&mut app).await;
    assert_eq!(lookups(&server), 1, "one lookup at launch");

    // A commit moves HEAD.
    fs::write(root.join("new.txt"), "n\nm\n").unwrap();
    commit_all(&root, "more");
    app.test_refresh_git();
    assert!(app.settle(|_| lookups(&server) == 2).await);
    idle(&mut app).await;

    // Hidden, no poll is due however short the interval.
    app.review.interval = Duration::from_millis(20);
    app.ui.side_panel = crate::layout::SidePanel::Explorer;
    app.review.next_poll = Some(std::time::Instant::now());
    assert_eq!(app.test_review_deadline(), None);
    // Shown: showing looks up, and the timer then repeats it.
    app.test_execute(Command::ShowReview);
    assert!(app.settle(|_| lookups(&server) >= 4).await, "{}", lookups(&server));
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_file_rows_select_by_key_and_click_and_scroll_by_wheel() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    screen(&mut app);
    // The first file is selected on showing; the arrows, End and Home move it.
    let path = |app: &App| app.review.view.selected_path().map(String::from);
    let files: Vec<String> = app.review.files().iter().map(|f| f.path.clone()).collect();
    assert_eq!(path(&app), Some(files[0].clone()));
    app.press(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(path(&app), Some(files[1].clone()));
    app.press(KeyCode::End, KeyModifiers::NONE);
    assert_eq!(path(&app), Some(files[3].clone()));
    app.press(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(path(&app), Some(files[0].clone()));
    // A click selects the row under it.
    screen(&mut app);
    let list = app.hits.review.clone().unwrap().list;
    app.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 8, list.y + 2);
    assert_eq!(path(&app), Some(files[2].clone()));
    // A wheel turn over a list that fits changes nothing.
    app.mouse(MouseEventKind::ScrollDown, list.x + 8, list.y + 1);
    assert_eq!(app.hits.review.clone().unwrap().rows.len(), 4);
    // Esc hands the keys back to the editor when there is one; with no tab it stays.
    app.press(KeyCode::Esc, KeyModifiers::NONE);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_right_click_opens_the_panels_menu() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    screen(&mut app);
    let hit = app.hits.review.clone().unwrap();
    // A file row: only what applies to a file.
    app.mouse(MouseEventKind::Down(MouseButton::Right), hit.list.x + 8, hit.list.y + 1);
    assert_eq!(menu_labels(&app), ["Toggle Viewed: on"]);
    let files: Vec<String> = app.review.files().iter().map(|f| f.path.clone()).collect();
    assert_eq!(app.review.view.selected_path(), Some(files[1].as_str()));
    // The summary: the merge request's own.
    screen(&mut app);
    app.mouse(
        MouseEventKind::Down(MouseButton::Right),
        hit.summary.x + 2,
        hit.summary.y,
    );
    let labels = menu_labels(&app);
    assert_eq!(labels[..2], ["Refresh: on", "Switch Merge Request: on"]);
    assert!(
        !labels
            .iter()
            .any(|l| l.contains("Toggle Viewed") || l.contains("Draft")),
        "{labels:?}"
    );
}

/// The open context menu's items as `label: on` / `label: off` (separators left out).
fn menu_labels(app: &App) -> Vec<String> {
    app.context_menu
        .as_ref()
        .expect("a menu is open")
        .items
        .iter()
        .filter_map(|i| match i {
            crate::context_menu::MenuItem::Item { label, enabled, .. } => {
                Some(format!("{label}: {}", if *enabled { "on" } else { "off" }))
            }
            crate::context_menu::MenuItem::Separator => None,
        })
        .collect()
}

/// An app with the panel shown and loaded on the standard repository.
async fn shown(dir: &Path) -> (App, FakeServer, String) {
    let (root, main) = make_repo(dir);
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    screen(&mut app);
    (app, server, main)
}

/// Selects `path` in the panel and opens it with Enter; the diff tab loaded.
async fn open_file(app: &mut App, path: &str) {
    let (view, files) = app.review.split();
    view.select_path(files, path);
    app.ui.focus = crate::app::Focus::SidePanel;
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
}

fn diff(app: &App) -> &crate::diff_view::DiffTab {
    app.group().active_diff().expect("a diff tab")
}

fn draw_wide(app: &mut App, width: u16) {
    let mut term = Terminal::new(TestBackend::new(width, 24)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

fn left_right(app: &App) -> (String, String) {
    let doc = diff(app).doc.as_ref().unwrap();
    let all = |l: &crate::diff_view::Lines| (0..l.len()).map(|i| format!("{}\n", l.line(i))).collect::<String>();
    (all(&doc.left), all(&doc.right))
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_enter_opens_an_inline_diff_of_the_merge_base_and_the_work_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server, main) = shown(dir.path()).await;
    open_file(&mut app, "mod.txt").await;
    let base = git2::Oid::from_str(&main).unwrap();
    let d = diff(&app);
    assert_eq!(d.pair, crate::diff_view::Pair::review(base));
    assert!(d.path.ends_with("mod.txt"));
    let (left, right) = left_right(&app);
    assert_eq!((left.as_str(), right.as_str()), ("m1\nm2\n", "m1\nm2 changed\nm3\n"));
    // Inline at any width: a 200-column draw would be side by side for any other diff tab.
    draw_wide(&mut app, 200);
    assert!(diff(&app).inline);
    assert!(diff(&app).forced_inline());
    // The buffer opened behind the tab.
    assert!(app.settle(|a| a.buffers.iter().any(|(_, b)| b.has_file())).await);
    // A second Enter reuses the tab.
    let tabs = app.group().tabs.len();
    app.ui.focus = crate::app::Focus::SidePanel;
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.group().tabs.len(), tabs);
    open_file(&mut app, "new.txt").await;
    let diffs = |app: &App| app.group().tabs.iter().filter(|t| t.diff().is_some()).count();
    assert_eq!(diffs(&app), 2);
    open_file(&mut app, "mod.txt").await;
    assert_eq!(diffs(&app), 2);
    assert!(diff(&app).path.ends_with("mod.txt"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_click_opens_and_other_diff_tabs_stay_side_by_side() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server, _main) = shown(dir.path()).await;
    let list = app.hits.review.clone().unwrap().list;
    let paths = app.hits.review.clone().unwrap().rows;
    let row = paths.iter().position(|p| p == "new.txt").unwrap() as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 8, list.y + row);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert!(diff(&app).path.ends_with("new.txt"));
    let (left, right) = left_right(&app);
    assert_eq!((left.as_str(), right.as_str()), ("", "n\n"));
    // A diff of another kind of the same file is not forced inline.
    let path = diff(&app).path.clone();
    app.open_diff(path, crate::diff_view::Pair::UNSTAGED);
    assert!(app.settle(|a| a.diffs_settled()).await);
    draw_wide(&mut app, 200);
    assert!(!diff(&app).inline);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_deleted_and_renamed_files_read_the_right_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server, _main) = shown(dir.path()).await;
    open_file(&mut app, "gone.txt").await;
    let (left, right) = left_right(&app);
    assert_eq!((left.as_str(), right.as_str()), ("x\n", ""));
    // No buffer for a file that is gone.
    assert!(
        !app.buffers
            .iter()
            .any(|(_, b)| b.has_file() && b.path.ends_with("gone.txt"))
    );
    open_file(&mut app, "renamed.txt").await;
    let (left, right) = left_right(&app);
    assert_eq!(left, "one\ntwo\nthree\nfour\nfive\nsix\n");
    assert_eq!(right, "one\ntwo\nthree\nfour\nfive\nsix\nseven\n");
    draw_wide(&mut app, 120);
    assert!(diff(&app).inline);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_a_binary_file_shows_the_binary_state() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    fs::write(root.join("blob.bin"), b"a\0b\0c").unwrap();
    commit_all(&root, "binary");
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    assert!(app.settle(loaded).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    screen(&mut app);
    open_file(&mut app, "blob.bin").await;
    assert!(diff(&app).doc.as_ref().unwrap().binary);
    // The background open of the binary file is dropped: no hex tab, and the diff keeps focus.
    let path = diff(&app).path.clone();
    idle(&mut app).await;
    assert!(app.group().find_hex(&path).is_none());
    assert!(app.group().tabs.iter().all(|t| t.diff().is_some()));
    assert!(app.group().active_diff().is_some());
}

/// Opening a binary file normally still gives a hex tab (the dropped case is background-only).
#[tokio::test(flavor = "multi_thread")]
async fn mr_review_a_normal_open_of_a_binary_file_is_still_a_hex_tab() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    fs::write(root.join("blob.bin"), b"a\0b\0c").unwrap();
    commit_all(&root, "binary");
    let server = one_mr(&main);
    let mut app = open_app(&root, None, config_for(&server), true);
    app.test_open_file(root.join("blob.bin"));
    assert!(
        app.settle(|a| a.group().find_hex(&root.join("blob.bin")).is_some())
            .await
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_edits_on_the_right_side_go_through_the_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server, _main) = shown(dir.path()).await;
    open_file(&mut app, "mod.txt").await;
    assert!(app.settle(|a| a.buffers.iter().any(|(_, b)| b.has_file())).await);
    draw_wide(&mut app, 200);
    // Rows: m1, -m2, +m2 changed, +m3. Down onto an added row, then type.
    app.press(KeyCode::Down, KeyModifiers::NONE);
    draw_wide(&mut app, 200);
    app.type_text("X");
    let text = |app: &App| {
        app.buffers
            .iter()
            .find(|(_, b)| b.has_file())
            .unwrap()
            .1
            .rope
            .to_string()
    };
    assert!(text(&app).contains('X'), "{}", text(&app));
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert!(left_right(&app).1.contains('X'));
    // The merge base side is untouched and undo reverts the buffer.
    assert_eq!(left_right(&app).0, "m1\nm2\n");
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), "m1\nm2 changed\nm3\n");
}

// ---- mr_review-d: definition, references and hover on a review's diff tab ----

mod lsp {
    use super::*;
    use crate::lsp::fake::{FakeLauncher, FakeOptions};
    use crate::milestone9_tests::{cmd, draw, find_cells, idle, location, popup_rows, terminal, uri};
    use serde_json::json;

    /// `main` with `a.cpp` / `b.cpp`, then `feature` renaming `value` to `target` in `a.cpp`.
    fn make_cpp_repo(base: &Path) -> (PathBuf, String) {
        let root = base.join("proj");
        fs::create_dir_all(&root).unwrap();
        init(&root);
        fs::write(root.join("a.cpp"), "int value = 1;\nint use = value;\nint gone = 0;\n").unwrap();
        fs::write(root.join("b.cpp"), "int other = 1;\nint more = 2;\n").unwrap();
        commit_all(&root, "base");
        let main = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&root, &["remote", "add", "origin", REMOTE]);
        git(&root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("a.cpp"), "int target = 1;\nint use = target;\n").unwrap();
        commit_all(&root, "work");
        (root, main)
    }

    /// The panel's app with a fake language server, `a.cpp`'s review diff open and its buffer
    /// synced with the server.
    async fn review_app(dir: &Path) -> (App, Arc<FakeLauncher>, PathBuf, FakeServer) {
        review_app_with(dir, None).await
    }

    /// `review_app`, with a thread (author, body) on `a.cpp`'s first right line when given.
    async fn review_app_with(
        dir: &Path,
        thread: Option<(&str, &str)>,
    ) -> (App, Arc<FakeLauncher>, PathBuf, FakeServer) {
        let (root, main) = make_cpp_repo(dir);
        let server = match thread {
            None => one_mr(&main),
            Some((who, body)) => {
                let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
                let c = super::threads::rc(11, who, body, "a.cpp", 1, &head, "+int target = 1;", false);
                super::threads::with_comments(&main, &format!("[{c}]"))
            }
        };
        let fake = FakeLauncher::new(FakeOptions::default());
        let base = workspace::normalize(&root);
        let trust = TrustStore::load(base.join("trust.toml"));
        let mut app = App::new(Workspace::load(base.clone()), trust, None, true);
        app.lsp.set_launcher(fake.clone());
        app.review.config = Some(Arc::new(config_for(&server)));
        app.review.env = Arc::new(|n| (n == "FAKE_TOKEN").then(|| "secret".to_string()));
        app.review.auto = true;
        app.test_start();
        assert!(app.settle(loaded).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        screen(&mut app);
        open_file(&mut app, "a.cpp").await;
        let a = base.join("a.cpp");
        assert!(app.settle(|_| fake.document(&uri(&a)).is_some()).await);
        (app, fake, a, server)
    }

    /// Puts the diff's caret on a line of the right side (or a deleted line of the left) at
    /// display column `col`.
    fn caret_at(app: &mut App, right: bool, line: usize, col: usize) {
        let d = app.group_mut().active_diff_mut().unwrap();
        let row = d
            .rows()
            .iter()
            .position(|r| if right { r.right } else { r.left } == Some(line) && (right || r.right.is_none()))
            .expect("a row");
        d.caret_click(row, right, col, false);
    }

    fn position(fake: &FakeLauncher, method: &str) -> (u64, u64) {
        let p = fake.requests(method).last().expect("a request").clone();
        (
            p["position"]["line"].as_u64().unwrap(),
            p["position"]["character"].as_u64().unwrap(),
        )
    }

    fn message(app: &App) -> String {
        app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_f12_on_a_right_side_line_asks_for_the_definition_and_back_returns() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, a, _server) = review_app(dir.path()).await;
        let b = a.parent().unwrap().join("b.cpp");
        caret_at(&mut app, true, 1, 12);
        // Same file: the buffer's text tab, with the cursor at the answer.
        fake.respond("textDocument/definition", location(&a, 0, 4, 10));
        app.press(KeyCode::F(12), KeyModifiers::NONE);
        assert!(
            app.settle(|a| a.group().active_tab().is_some()).await,
            "{}",
            message(&app)
        );
        assert_eq!(position(&fake, "textDocument/definition"), (1, 12));
        assert_eq!(
            fake.requests("textDocument/definition")[0]["textDocument"]["uri"],
            json!(uri(&a))
        );
        assert_eq!(app.active_buffer().unwrap().path, a);
        app.test_execute(Command::NavBack);
        assert!(app.group().active_diff().is_some(), "Back returns to the diff");
        // Another file.
        fake.respond("textDocument/definition", location(&b, 1, 4, 8));
        app.press(KeyCode::F(12), KeyModifiers::NONE);
        assert!(
            app.settle(|a| a.active_buffer().is_some_and(|b2| b2.path == b)).await,
            "{}",
            message(&app)
        );
        app.test_execute(Command::NavBack);
        assert!(app.group().active_diff().is_some(), "Back returns to the diff");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_shift_f12_lists_references_from_the_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, a, _server) = review_app(dir.path()).await;
        caret_at(&mut app, true, 0, 5);
        fake.respond(
            "textDocument/references",
            json!([location(&a, 0, 4, 10), location(&a, 1, 11, 17)]),
        );
        app.press(KeyCode::F(12), KeyModifiers::SHIFT);
        assert!(
            app.settle(|_| !fake.requests("textDocument/references").is_empty())
                .await,
            "{}",
            message(&app)
        );
        assert_eq!(position(&fake, "textDocument/references"), (0, 5));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_deleted_lines_have_no_language_server() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, _a, _server) = review_app(dir.path()).await;
        caret_at(&mut app, false, 2, 5);
        assert!(!diff(&app).caret.right);
        for keys in [
            (KeyCode::F(12), KeyModifiers::NONE),
            (KeyCode::F(12), KeyModifiers::SHIFT),
            (KeyCode::Char('i'), cmd() | KeyModifiers::ALT),
        ] {
            app.message = None;
            app.press(keys.0, keys.1);
            assert_eq!(message(&app), "Deleted lines have no language server", "{keys:?}");
        }
        idle(&mut app).await;
        for m in [
            "textDocument/definition",
            "textDocument/references",
            "textDocument/hover",
        ] {
            assert!(fake.requests(m).is_empty(), "{m}");
        }
        assert!(app.group().active_diff().is_some());
    }

    /// The Done-when walk's language part (it does not depend on the forge): the panel lists the
    /// merge request and its file, which opens as an inline diff; F12 and hover work on a
    /// right-side line (Back returns), and a deleted line says why not.
    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_done_when_gitea_language_navigation_from_the_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, a, _server) = review_app_with(dir.path(), Some(("bob", "Please check"))).await;
        let panel = screen(&mut app);
        assert!(panel.contains("Add review panel") && panel.contains("a.cpp"), "{panel}");
        assert!(app.group().active_diff().is_some());
        let mut term = terminal();
        draw(&mut term, &mut app);
        assert!(find_cells(&term, "Please check").is_some(), "the thread shows inline");
        caret_at(&mut app, true, 1, 12);
        fake.respond("textDocument/definition", location(&a, 0, 4, 10));
        app.press(KeyCode::F(12), KeyModifiers::NONE);
        assert!(app.settle(|a| a.active_buffer().is_some()).await, "{}", message(&app));
        app.test_execute(Command::NavBack);
        assert!(app.group().active_diff().is_some(), "Back returns to the diff");
        let range = json!({ "start": { "line": 1, "character": 11 }, "end": { "line": 1, "character": 17 } });
        fake.respond(
            "textDocument/hover",
            json!({ "contents": { "kind": "plaintext", "value": "int target" }, "range": range }),
        );
        caret_at(&mut app, true, 1, 12);
        app.press(KeyCode::Char('i'), cmd() | KeyModifiers::ALT);
        assert!(app.settle(|a| a.hover.popup.is_some()).await, "{}", message(&app));
        app.hover.popup = None;
        caret_at(&mut app, false, 2, 5);
        app.message = None;
        app.press(KeyCode::F(12), KeyModifiers::NONE);
        assert_eq!(message(&app), "Deleted lines have no language server");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_hover_by_keyboard_and_mouse_shows_a_popup_over_the_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, _a, _server) = review_app(dir.path()).await;
        let range = json!({ "start": { "line": 1, "character": 11 }, "end": { "line": 1, "character": 17 } });
        fake.respond(
            "textDocument/hover",
            json!({ "contents": { "kind": "plaintext", "value": "int target" }, "range": range }),
        );
        let mut term = terminal();
        caret_at(&mut app, true, 1, 12);
        draw(&mut term, &mut app);
        let (x, y) = find_cells(&term, "use = target;").unwrap();
        app.press(KeyCode::Char('i'), cmd() | KeyModifiers::ALT);
        assert!(app.settle(|a| a.hover.popup.is_some()).await, "{}", message(&app));
        assert_eq!(position(&fake, "textDocument/hover"), (1, 12));
        draw(&mut term, &mut app);
        let r = app.hits.hover.expect("popup drawn");
        assert_eq!(popup_rows(&term, &app), vec!["int target"]);
        assert_eq!(r.y + r.height, y, "just above the hovered row");
        // The mouse resting on the word.
        app.hover.popup = None;
        draw(&mut term, &mut app);
        app.mouse(crossterm::event::MouseEventKind::Moved, x + 7, y);
        app.test_hover_tick(std::time::Instant::now() + crate::hover::DELAY);
        assert!(app.settle(|a| a.hover.popup.is_some()).await, "{}", message(&app));
        assert!(app.hover.popup.as_ref().unwrap().mouse);
        assert_eq!(position(&fake, "textDocument/hover"), (1, 11));
        // Resting over a deleted line starts nothing.
        app.hover.popup = None;
        let before = fake.requests("textDocument/hover").len();
        draw(&mut term, &mut app);
        let (dx, dy) = find_cells(&term, "int gone = 0;").unwrap();
        app.mouse(crossterm::event::MouseEventKind::Moved, dx + 7, dy);
        assert!(app.hover.rest.is_none());
        idle(&mut app).await;
        assert_eq!(fake.requests("textDocument/hover").len(), before);
    }

    /// A thread block between two code rows: Ctrl+click, F12 and a hover rest on the row below
    /// it reach the buffer position of that row, and the popup is drawn just above that row.
    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_rows_below_a_thread_block_still_hit_the_right_position() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, a, _server) = review_app_with(dir.path(), Some(("sam", "Why this name"))).await;
        let mut term = terminal();
        draw(&mut term, &mut app);
        assert_eq!(diff(&app).threads.blocks().list().len(), 1, "the thread is placed");
        let (_, by) = find_cells(&term, "Why this name").expect("the block is drawn");
        let (x, y) = find_cells(&term, "use = target;").unwrap();
        assert!(y > by + 1, "the code row is below the block's footer: {by} {y}");
        // Ctrl+click on the word.
        fake.respond("textDocument/definition", location(&a, 0, 4, 10));
        app.mouse_with(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            x + 7,
            y,
            cmd(),
        );
        assert!(
            app.settle(|_| !fake.requests("textDocument/definition").is_empty())
                .await
        );
        assert_eq!(position(&fake, "textDocument/definition"), (1, 11));
        // The answer opens the buffer's text tab; Back returns to the diff.
        assert!(app.settle(|a| a.group().active_diff().is_none()).await);
        app.test_execute(Command::NavBack);
        assert!(app.group().active_diff().is_some());
        // F12 from a caret placed on that row.
        caret_at(&mut app, true, 1, 12);
        let before = fake.requests("textDocument/definition").len();
        app.press(KeyCode::F(12), KeyModifiers::NONE);
        assert!(
            app.settle(|_| fake.requests("textDocument/definition").len() > before)
                .await
        );
        assert_eq!(position(&fake, "textDocument/definition"), (1, 12));
        assert!(app.settle(|a| a.group().active_diff().is_none()).await);
        app.test_execute(Command::NavBack);
        assert!(app.group().active_diff().is_some());
        // The mouse resting on the word.
        let range = json!({ "start": { "line": 1, "character": 11 }, "end": { "line": 1, "character": 17 } });
        fake.respond(
            "textDocument/hover",
            json!({ "contents": { "kind": "plaintext", "value": "int target" }, "range": range }),
        );
        app.hover.popup = None;
        draw(&mut term, &mut app);
        let (x, y) = find_cells(&term, "use = target;").unwrap();
        app.mouse(crossterm::event::MouseEventKind::Moved, x + 7, y);
        app.test_hover_tick(std::time::Instant::now() + crate::hover::DELAY);
        assert!(app.settle(|a| a.hover.popup.is_some()).await, "{}", message(&app));
        assert_eq!(position(&fake, "textDocument/hover"), (1, 11));
        draw(&mut term, &mut app);
        let r = app.hits.hover.expect("popup drawn");
        assert_eq!(popup_rows(&term, &app), vec!["int target"]);
        assert_eq!(r.y + r.height, y, "just above the hovered row, under the block");
        // Resting on the block's text starts nothing.
        app.hover.popup = None;
        let before = fake.requests("textDocument/hover").len();
        draw(&mut term, &mut app);
        let (bx, by) = find_cells(&term, "Why this name").unwrap();
        app.mouse(crossterm::event::MouseEventKind::Moved, bx + 2, by);
        assert!(app.hover.rest.is_none());
        idle(&mut app).await;
        assert_eq!(fake.requests("textDocument/hover").len(), before);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_ctrl_click_on_a_right_side_word_goes_to_its_definition() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, a, _server) = review_app(dir.path()).await;
        fake.respond("textDocument/definition", location(&a, 0, 4, 10));
        let mut term = terminal();
        draw(&mut term, &mut app);
        let (x, y) = find_cells(&term, "use = target;").unwrap();
        app.mouse_with(
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            x + 7,
            y,
            cmd(),
        );
        assert!(
            app.settle(|a| a.group().active_tab().is_some()).await,
            "{}",
            message(&app)
        );
        assert_eq!(position(&fake, "textDocument/definition"), (1, 11));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_other_diff_tabs_have_no_language_requests() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, fake, a, _server) = review_app(dir.path()).await;
        app.open_diff(a, crate::diff_view::Pair::UNSTAGED);
        assert!(app.settle(|a| a.diffs_settled()).await);
        app.message = None;
        app.press(KeyCode::F(12), KeyModifiers::NONE);
        app.press(KeyCode::Char('i'), cmd() | KeyModifiers::ALT);
        idle(&mut app).await;
        assert!(fake.requests("textDocument/definition").is_empty());
        assert!(fake.requests("textDocument/hover").is_empty());
        assert!(message(&app).is_empty(), "{}", message(&app));
    }
}

/// mr_review-e1: threads loaded with the lookup, placed against the live text, the unresolved
/// count and the CONVERSATIONS section (a divider that drags, a collapsing header, Enter / a
/// click opening the file at the thread's line).
mod threads {
    use super::*;
    use crate::forge::Side;
    use crate::review_anchor::Placement;
    use crossterm::event::{MouseButton, MouseEventKind};

    const ISSUES: &str = "/api/v1/repos/alice/project/issues/7/comments?page=1&limit=50";

    #[allow(clippy::too_many_arguments)]
    pub(super) fn rc(
        id: u64,
        user: &str,
        body: &str,
        path: &str,
        line: u32,
        commit: &str,
        hunk_last: &str,
        resolved: bool,
    ) -> String {
        format!(
            r#"{{"id":{id},"user":{{"login":"{user}"}},"body":"{body}","path":"{path}","position":{line},
            "original_position":{line},"commit_id":"{commit}","original_commit_id":"{commit}",
            "diff_hunk":"@@ -1 +1 @@\n{hunk_last}","resolver":{}}}"#,
            if resolved { r#"{"login":"bob"}"# } else { "null" }
        )
    }

    /// A fake forge whose review has these line comments (a JSON array) and one plain comment.
    pub(super) fn with_comments(main: &str, comments: &str) -> FakeServer {
        FakeServer::start(vec![
            (
                format!("{PULLS}?state=open&page=1&limit=50"),
                Reply::json(&format!("[{}]", pr_json(7, "Add review panel", main))),
            ),
            (format!("{PULLS}/7"), Reply::json(&pr_json(7, "Add review panel", main))),
            (
                format!("{PULLS}/7/reviews?page=1&limit=50"),
                Reply::json(
                    r#"[{"id":1,"user":{"login":"bob"},"body":"","state":"COMMENT","comments_count":9,
                    "submitted_at":"2026-01-02T10:00:00Z","html_url":"u"}]"#,
                ),
            ),
            (format!("{PULLS}/7/reviews/1/comments"), Reply::json(comments)),
            (
                ISSUES.to_string(),
                Reply::json(
                    r#"[{"id":99,"user":{"login":"ann"},"body":"Thanks for\nlooking","created_at":"2026-01-01T10:00:00Z",
                    "updated_at":"","html_url":"u99"}]"#,
                ),
            ),
        ])
    }

    fn standard(head: &str, main: &str) -> String {
        format!(
            "[{},{},{}]",
            rc(
                11,
                "sam",
                "Can this overflow on a narrow terminal",
                "mod.txt",
                3,
                head,
                "+m3",
                false
            ),
            rc(12, "bob", "Why was this removed", "mod.txt", 2, main, "-m2", true),
            rc(13, "cy", "Old commit", "mod.txt", 1, &"d".repeat(40), "+m1", false),
        )
    }

    async fn loaded_app(dir: &Path, edit: Option<(&str, &str)>) -> (App, FakeServer) {
        let (root, main) = make_repo(dir);
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        if let Some((file, text)) = edit {
            fs::write(root.join(file), text).unwrap();
        }
        let server = with_comments(&main, &standard(&head, &main));
        let mut app = open_app(&root, None, config_for(&server), true);
        assert!(app.settle(loaded).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        app.review.view.conv.section_height = 16;
        screen(&mut app);
        (app, server)
    }

    fn places(app: &App) -> Vec<(String, Placement)> {
        app.review
            .review()
            .unwrap()
            .threads
            .iter()
            .map(|t| (t.thread.id.clone(), t.place.clone()))
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_threads_are_placed_outdated_or_missing_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        let r = app.review.review().unwrap();
        assert_eq!((r.unresolved(), r.threads.len(), r.conversation.len()), (2, 3, 1));
        assert!(r.comments_error.is_none());
        // Sorted by path and line: the missing commit's line 1, the deleted line 2, the new line 3.
        let p = places(&app);
        assert_eq!(p[0], ("13".into(), Placement::Missing));
        assert_eq!(
            p[1],
            (
                "12".into(),
                Placement::Placed {
                    side: Side::Left,
                    lines: 1..2
                }
            )
        );
        assert_eq!(
            p[2],
            (
                "11".into(),
                Placement::Placed {
                    side: Side::Right,
                    lines: 2..3
                }
            )
        );
        let text = screen(&mut app);
        assert!(text.contains("2 unresolved"), "{text}");
        assert!(text.contains("CONVERSATIONS (4)"), "{text}");
        assert!(text.contains("mod.txt:3  sam"), "{text}");
        assert!(text.contains("Can this overflow"), "{text}");
        assert!(text.contains("no commit"), "{text}");
        assert!(text.contains("ann  (merge request)"), "{text}");
        assert!(text.contains("Thanks for"), "{text}");
        assert!(!text.contains("looking"), "only the first line shows");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_local_edit_outdates_the_thread_on_that_line() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), Some(("mod.txt", "m1\nm2 changed\nm3 edited\n"))).await;
        let p = places(&app);
        assert_eq!(p[2], ("11".into(), Placement::Outdated));
        // The deleted line is judged against the merge base: still placed.
        assert!(matches!(p[1].1, Placement::Placed { side: Side::Left, .. }));
        let text = screen(&mut app);
        assert!(text.contains("outdated"), "{text}");
        // Enter opens the file near where the thread was written (line 3).
        let conv = &mut app.review.view.conv;
        conv.focused = true;
        conv.select(4, 2);
        app.ui.focus = crate::app::Focus::SidePanel;
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|a| a.diffs_settled()).await);
        assert!(diff(&app).path.ends_with("mod.txt"));
        assert_eq!(diff(&app).cursor_line(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_deleted_line_thread_opens_on_the_left_side() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        // The rows: 0 the missing commit, 1 the deleted line, 2 the new line, 3 the MR comment.
        let hit = app.hits.review.clone().unwrap();
        app.mouse(
            MouseEventKind::Down(MouseButton::Left),
            hit.conv_list.x + 6,
            hit.conv_list.y + 2,
        );
        assert!(app.settle(|a| a.diffs_settled()).await);
        let d = diff(&app);
        assert!(d.path.ends_with("mod.txt"));
        let row = d.rows()[d.cursor];
        assert_eq!(row.left, Some(1), "the line m2 of the merge base");
        // A placed right-side thread goes to its line.
        app.ui.focus = crate::app::Focus::SidePanel;
        app.review.view.conv.focused = true;
        app.review.view.conv.select(4, 2);
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|a| a.diffs_settled()).await);
        assert_eq!(diff(&app).cursor_line(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_thread_on_a_line_with_no_right_counterpart_opens_a_left_only_row() {
        let dir = tempfile::tempdir().unwrap();
        // m2 is deleted outright (no replacement line); the right-side thread on line 3 is now
        // past the end of the file.
        let (mut app, _server) = loaded_app(dir.path(), Some(("mod.txt", "m1\n"))).await;
        let p = places(&app);
        assert_eq!(
            p[1],
            (
                "12".into(),
                Placement::Placed {
                    side: Side::Left,
                    lines: 1..2
                }
            )
        );
        assert_eq!(p[2], ("11".into(), Placement::Outdated));
        let hit = app.hits.review.clone().unwrap();
        app.mouse(
            MouseEventKind::Down(MouseButton::Left),
            hit.conv_list.x + 6,
            hit.conv_list.y + 2,
        );
        assert!(app.settle(|a| a.diffs_settled()).await);
        let d = diff(&app);
        assert!(d.path.ends_with("mod.txt"));
        let row = d.rows()[d.cursor];
        assert_eq!(
            (row.left, row.right),
            (Some(1), None),
            "the removed m2, a left-only row"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_selected_conversation_is_highlighted_behind_its_text() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        app.review.view.conv.section_height = 10;
        app.review.view.conv.focused = true;
        screen(&mut app);
        let mut term = Terminal::new(TestBackend::new(70, 24)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let hit = app.hits.review.clone().unwrap();
        let buf = term.backend().buffer();
        let row = hit.conv_list;
        let want = app.theme.get("list.active").bg;
        // Both lines of the selected row, under the text as well as past it (the side border's
        // column aside).
        for y in [row.y, row.y + 1] {
            for x in row.x..row.x + row.width - 1 {
                assert_eq!(
                    buf[(x, y)].bg,
                    want.unwrap(),
                    "at ({x}, {y}) `{}`",
                    buf[(x, y)].symbol()
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_thread_rows_menu_has_its_own_items() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        app.review.user = Some("nobody".into());
        let hit = app.hits.review.clone().unwrap();
        app.mouse(
            MouseEventKind::Down(MouseButton::Right),
            hit.conv_list.x + 4,
            hit.conv_list.y,
        );
        assert!(app.review.view.conv.focused);
        assert_eq!(app.review.view.conv.selected(hit.conv_len), Some(0));
        assert_eq!(
            super::menu_labels(&app),
            [
                "Go to Thread: on",
                "Reply: on",
                "Resolve / Unresolve Thread: on",
                "Edit Comment: off",
                "Delete Comment: off",
                "Apply Suggestion: off",
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_divider_drags_and_the_header_collapses() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        app.review.view.conv.section_height = 10;
        screen(&mut app);
        let hit = app.hits.review.clone().unwrap();
        let divider = hit.dividers[0].0;
        assert!(divider.height == 1 && hit.conv_list.height > 0);
        assert_eq!(hit.dividers.len(), 1, "no PENDING without drafts");
        // The pointer shows a resize over it.
        app.mouse(MouseEventKind::Moved, divider.x + 5, divider.y);
        assert_eq!(app.pointer, crate::terminal::Pointer::RowResize);
        app.mouse(MouseEventKind::Moved, divider.x + 5, divider.y + 1);
        assert_eq!(app.pointer, crate::terminal::Pointer::Default);
        let before = app.review.view.conv.section_height;
        // Dragging the divider up by two rows makes the section two rows taller.
        app.mouse(MouseEventKind::Down(MouseButton::Left), divider.x + 5, divider.y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), divider.x + 5, divider.y - 2);
        app.mouse(MouseEventKind::Up(MouseButton::Left), divider.x + 5, divider.y - 2);
        assert_eq!(app.review.view.conv.section_height, before + 2);
        screen(&mut app);
        let again = app.hits.review.clone().unwrap();
        assert_eq!(again.dividers[0].0.y, divider.y - 2);
        // The header collapses the list.
        app.mouse(
            MouseEventKind::Down(MouseButton::Left),
            again.conv_header.x + 5,
            again.conv_header.y,
        );
        let text = screen(&mut app);
        assert!(app.review.view.conv.collapsed);
        assert!(
            text.contains("CONVERSATIONS (4)") && !text.contains("Can this overflow"),
            "{text}"
        );
        assert_eq!(app.hits.review.clone().unwrap().conv_list.height, 0);
        // And opens it again.
        let h = app.hits.review.clone().unwrap();
        app.mouse(
            MouseEventKind::Down(MouseButton::Left),
            h.conv_header.x + 5,
            h.conv_header.y,
        );
        assert!(!app.review.view.conv.collapsed);
        // Down from the last file enters the list; Up from its first row leaves it.
        app.review.view.conv.focused = false;
        screen(&mut app);
        app.press(KeyCode::End, KeyModifiers::NONE);
        app.press(KeyCode::Down, KeyModifiers::NONE);
        assert!(app.review.view.conv.focused);
        app.press(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.review.view.conv.selected(4), Some(1));
        app.press(KeyCode::Up, KeyModifiers::NONE);
        app.press(KeyCode::Up, KeyModifiers::NONE);
        assert!(!app.review.view.conv.focused);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_comments_failure_keeps_the_merge_request_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        // The plain fake has no review routes: the comments 404.
        let server = one_mr(&main);
        let mut app = open_app(&root, None, config_for(&server), true);
        assert!(app.settle(loaded).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        let r = app.review.review().unwrap();
        assert_eq!(r.files.len(), 4);
        assert!(r.comments_error.as_deref().is_some_and(|e| e.starts_with("comments:")));
        assert!(app.review.error.is_none());
        let text = screen(&mut app);
        assert!(
            text.contains("CONVERSATIONS (0)") && text.contains("comments:"),
            "{text}"
        );
    }

    /// The drawn screen as lines (width `w`, 30 rows).
    fn lines_at(app: &mut App, w: u16) -> Vec<String> {
        lines_sized(app, w, 30)
    }

    fn lines_sized(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect()
    }

    fn row_of(lines: &[String], needle: &str) -> usize {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no `{needle}` in\n{}", lines.join("\n")))
    }

    fn diff_hit(app: &App) -> crate::layout::DiffHit {
        app.hits.groups.iter().find_map(|g| g.diff.clone()).expect("a diff hit")
    }

    fn block_rows(app: &App) -> usize {
        diff(app).threads.blocks().list().iter().map(|b| b.height).sum()
    }

    async fn opened() -> (App, FakeServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, server) = loaded_app(dir.path(), None).await;
        open_file(&mut app, "mod.txt").await;
        (app, server, dir)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_thread_blocks_sit_under_their_lines_and_the_resolved_one_is_collapsed() {
        let (mut app, _server, _dir) = opened().await;
        let l = lines_at(&mut app, 120);
        // m1, -m2, the resolved (left side) thread folded to one row, +m2 changed, +m3, the open one.
        let (m2, bob, changed, m3, sam, foot) = (
            row_of(&l, "m2"),
            row_of(&l, "bob"),
            row_of(&l, "m2 changed"),
            row_of(&l, "m3"),
            row_of(&l, "sam"),
            row_of(&l, "└"),
        );
        assert!(m2 < bob && bob < changed && changed < m3 && m3 < sam, "{l:#?}");
        assert_eq!((bob, sam), (m2 + 1, m3 + 1));
        assert!(
            l[bob].contains("✓") && l[bob].contains("Why was this removed") && l[bob].contains("▸"),
            "{}",
            l[bob]
        );
        assert!(
            l[sam].contains("▾") && l[sam + 1].contains("Can this overflow"),
            "{l:#?}"
        );
        assert_eq!(foot, sam + 3);
        let d = diff(&app);
        assert_eq!(d.threads.blocks().list().len(), 2);
        assert_eq!(d.threads.blocks().list()[0].after_row, 1, "after the removed row");
        // The scroll bar counts the blocks' rows.
        assert_eq!(d.visual_len(), d.rows().len() + block_rows(&app));
        assert_eq!(block_rows(&app), 1 + 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_toggle_collapses_and_expands_a_block() {
        let (mut app, _server, _dir) = opened().await;
        lines_at(&mut app, 120);
        let hit = diff_hit(&app);
        assert_eq!(hit.blocks.len(), 2);
        // Expand the resolved one by its toggle.
        let t = hit.blocks[0].toggle.expect("header in view");
        app.mouse(MouseEventKind::Down(MouseButton::Left), t.x + 1, t.y);
        let l = lines_at(&mut app, 120);
        assert_eq!(block_rows(&app), 4 + 4);
        assert!(l[row_of(&l, "bob")].contains("▾"), "{l:#?}");
        // And fold the open one.
        let hit = diff_hit(&app);
        let t = hit.blocks[1].toggle.unwrap();
        app.mouse(MouseEventKind::Down(MouseButton::Left), t.x + 1, t.y);
        let l = lines_at(&mut app, 120);
        assert!(l[row_of(&l, "sam")].contains("▸"), "{l:#?}");
        assert_eq!(block_rows(&app), 4 + 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_caret_steps_over_blocks_and_clicks_map_through_them() {
        let (mut app, _server, _dir) = opened().await;
        lines_at(&mut app, 120);
        // Down from the removed row lands on the next code row, not on the block.
        app.group_mut().active_diff_mut().unwrap().cursor = 1;
        app.press(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(diff(&app).cursor, 2);
        app.press(KeyCode::Down, KeyModifiers::NONE);
        app.press(KeyCode::Up, KeyModifiers::NONE);
        app.press(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(diff(&app).cursor, 1);
        // A click on the line under the folded block lands on that row, one screen row lower
        // than the row index says; a click inside the block moves nothing.
        let l = lines_at(&mut app, 120);
        let hit = diff_hit(&app);
        let y = row_of(&l, "m2 changed") as u16;
        assert_eq!(y, hit.rows.y + 3);
        app.mouse(MouseEventKind::Down(MouseButton::Left), hit.text_x[1] + 1, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), hit.text_x[1] + 1, y);
        assert_eq!(diff(&app).cursor, 2);
        let y = row_of(&l, "+ m3").max(row_of(&l, "m3")) as u16;
        app.mouse(MouseEventKind::Down(MouseButton::Left), hit.text_x[1] + 1, y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), hit.text_x[1] + 1, y);
        assert_eq!(diff(&app).cursor, 3);
        app.group_mut().active_diff_mut().unwrap().cursor = 0;
        let block_y = row_of(&l, "Can this overflow") as u16;
        app.mouse(MouseEventKind::Down(MouseButton::Left), hit.text_x[1] + 4, block_y);
        app.mouse(MouseEventKind::Up(MouseButton::Left), hit.text_x[1] + 4, block_y);
        assert_eq!(diff(&app).cursor, 0, "a block is not text");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_next_and_previous_thread_wrap() {
        let (mut app, _server, _dir) = opened().await;
        lines_at(&mut app, 120);
        app.group_mut().active_diff_mut().unwrap().cursor = 0;
        let mut seen = Vec::new();
        for _ in 0..3 {
            app.test_execute(Command::ReviewNextThread);
            seen.push(diff(&app).cursor);
        }
        assert_eq!(seen, [1, 3, 1]);
        app.test_execute(Command::ReviewPrevThread);
        assert_eq!(diff(&app).cursor, 3);
        app.test_execute(Command::ReviewPrevThread);
        assert_eq!(diff(&app).cursor, 1);
        assert_eq!(Command::ReviewNextThread.title(), "Code Review: Next Thread");
        assert!(Command::all().contains(&Command::ReviewPrevThread));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_narrower_view_rewraps_the_blocks() {
        let (mut app, _server, _dir) = opened().await;
        lines_at(&mut app, 120);
        let wide = block_rows(&app);
        let l = lines_at(&mut app, 50);
        let narrow = block_rows(&app);
        assert!(narrow > wide, "{narrow} > {wide}\n{l:#?}");
        for line in &l {
            assert!(line.chars().count() <= 50);
        }
        lines_at(&mut app, 120);
        assert_eq!(block_rows(&app), wide);
    }

    /// `mod.txt`'s review diff with a tall thread on its first row (taller than a short
    /// terminal) and a short one on its last.
    async fn tall() -> (App, FakeServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let long = "alpha beta gamma delta ".repeat(20);
        let comments = format!(
            "[{},{}]",
            rc(21, "sam", long.trim(), "mod.txt", 1, &head, "+m1", false),
            rc(22, "cy", "short", "mod.txt", 3, &head, "+m3", false)
        );
        let server = with_comments(&main, &comments);
        let mut app = open_app(&root, None, config_for(&server), true);
        assert!(app.settle(loaded).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        screen(&mut app);
        open_file(&mut app, "mod.txt").await;
        (app, server, dir)
    }

    /// The first body row of the last frame.
    fn first_body_row(app: &App, l: &[String]) -> String {
        l[diff_hit(app).rows.y as usize].clone()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_scrolling_one_row_at_a_time_through_a_block_taller_than_the_view() {
        let (mut app, _server, _dir) = tall().await;
        let l = lines_sized(&mut app, 40, 14);
        let h = diff_hit(&app).rows.height as usize;
        let d = diff(&app);
        assert_eq!(d.threads.blocks().list().len(), 2, "{l:#?}");
        let tall_h = d.threads.blocks().list()[0].height;
        assert!(tall_h > h, "block {tall_h} taller than the view {h}\n{l:#?}");
        let total = d.visual_len();
        let thread = d.threads.list()[d.threads.blocks().list()[0].thread].clone();
        let lines = crate::review_blocks::lines(
            &thread,
            false,
            crate::review_blocks::block_width(diff_hit(&app).rows.width as usize),
        );
        assert_eq!(lines.len(), tall_h);
        let mut last_top = 0;
        loop {
            let top = diff(&app).top;
            let l = lines_sized(&mut app, 40, 14);
            let first = first_body_row(&app, &l);
            // Visual row 0 is `m1`; 1..=tall_h the block's lines; then the code again.
            if top == 0 {
                assert!(first.contains("m1"), "{first}");
            } else if top <= tall_h {
                let want = lines[top - 1].text.trim_end();
                assert!(first.contains(want), "top {top}: {first:?} lacks {want:?}");
            } else {
                assert!(!first.contains('│') && !first.contains('└'), "top {top}: {first:?}");
            }
            // Every visible block row is a block line, never code text (no row drawn twice).
            assert_eq!(
                l.iter().filter(|s| s.contains("m1")).count(),
                usize::from(top == 0),
                "top {top}"
            );
            assert!(diff(&app).top >= last_top);
            last_top = diff(&app).top;
            let before = diff(&app).top;
            app.group_mut().active_diff_mut().unwrap().scroll_by(1);
            if diff(&app).top == before {
                break;
            }
        }
        // Scrolled to the end: the bar's maximum, the last row is the last code row.
        let l = lines_sized(&mut app, 40, 14);
        let d = diff(&app);
        assert_eq!(
            (d.visual_len(), d.top),
            (total, total - diff_hit(&app).rows.height as usize),
            "the last row at the bottom"
        );
        assert!(l.iter().any(|s| s.contains("short")), "{l:#?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_next_thread_reveals_a_tall_block_and_the_caret_stays_on_screen() {
        let (mut app, _server, _dir) = tall().await;
        lines_sized(&mut app, 40, 14);
        app.group_mut().active_diff_mut().unwrap().cursor = 2;
        // 2 -> the block under row 3 (short) -> wraps to the tall one under row 0.
        app.test_execute(Command::ReviewNextThread);
        let l = lines_sized(&mut app, 40, 14);
        assert_eq!(diff(&app).cursor, 3);
        assert!(l.iter().any(|s| s.contains("short")), "{l:#?}");
        app.test_execute(Command::ReviewNextThread);
        let l = lines_sized(&mut app, 40, 14);
        assert_eq!(diff(&app).cursor, 0);
        assert_eq!(diff(&app).top, 0, "the cursor row first, the block as far as it fits");
        assert!(first_body_row(&app, &l).contains("m1"), "{l:#?}");
        // Down from the cursor above the tall block: the next code row, which scrolls into view.
        app.press(KeyCode::Down, KeyModifiers::NONE);
        let l = lines_sized(&mut app, 40, 14);
        assert_eq!(diff(&app).cursor, 1);
        assert!(l.iter().any(|s| s.contains("m2")), "the caret row is on screen\n{l:#?}");
        // And back up to row 0 from far below.
        app.press(KeyCode::Up, KeyModifiers::NONE);
        let l = lines_sized(&mut app, 40, 14);
        assert!(l.iter().any(|s| s.contains("m1")), "{l:#?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_tiny_views_a_width_change_while_scrolled_and_a_fold_do_not_panic() {
        let (mut app, _server, _dir) = tall().await;
        lines_sized(&mut app, 40, 14);
        app.group_mut().active_diff_mut().unwrap().scroll_by(5);
        // A view with no body rows, then one row.
        for h in [1, 2, 3, 4, 5] {
            lines_sized(&mut app, 40, h);
            lines_sized(&mut app, 3, h);
        }
        // Wider while the window starts inside the block: still within the new total.
        lines_sized(&mut app, 40, 14);
        app.group_mut().active_diff_mut().unwrap().scroll_by(5);
        lines_sized(&mut app, 200, 14);
        let d = diff(&app);
        assert!(d.top < d.visual_len(), "{} {}", d.top, d.visual_len());
        // Folding the block under a window that starts inside it keeps the window valid.
        lines_sized(&mut app, 40, 14);
        app.group_mut().active_diff_mut().unwrap().scroll_by(6);
        app.group_mut().active_diff_mut().unwrap().threads.toggle(0);
        let l = lines_sized(&mut app, 40, 14);
        let d = diff(&app);
        assert!(d.top < d.visual_len(), "{l:#?}");
        assert!(l.iter().any(|s| s.contains("▸")) || d.top > 1, "{l:#?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_odd_comment_text_stays_inside_the_block() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let body = "tab\\there 日本語日本語日本語日本語日本語日本語 😀😀😀😀😀😀😀😀😀😀😀😀 \
                    Supercalifragilisticexpialidocious_Supercalifragilisticexpialidocious";
        let comments = format!(
            "[{},{}]",
            rc(21, "sam", body, "mod.txt", 1, &head, "+m1", false),
            rc(22, "cy", "", "mod.txt", 3, &head, "+m3", false)
        );
        let server = with_comments(&main, &comments);
        let mut app = open_app(&root, None, config_for(&server), true);
        assert!(app.settle(loaded).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        screen(&mut app);
        open_file(&mut app, "mod.txt").await;
        let l = lines_sized(&mut app, 100, 30);
        assert_eq!(diff(&app).threads.blocks().list().len(), 2, "{l:#?}");
        let hit = diff_hit(&app);
        assert!(l.iter().any(|s| s.contains("tab    here")), "{l:#?}");
        // The footer's rule ends where the block does: nothing is drawn past it.
        let r = hit.blocks[0].rect;
        let foot = l.iter().find(|s| s.contains('└')).unwrap();
        assert!(foot.chars().filter(|c| *c == '─').count() >= 10);
        // No block row has text past the block's right edge (its 3-column indent plus width).
        for y in r.y..r.y + r.height {
            let right: String = l[y as usize]
                .chars()
                .skip((r.x + r.width) as usize)
                .take(96usize.saturating_sub((r.x + r.width) as usize))
                .collect();
            assert!(right.trim().is_empty(), "{:?}", l[y as usize]);
        }
        // The empty comment still has its header and footer.
        assert!(l.iter().filter(|s| s.contains('└')).count() == 2, "{l:#?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_new_lookup_updates_the_blocks() {
        let (mut app, _server, _dir) = opened().await;
        lines_at(&mut app, 120);
        assert_eq!(diff(&app).threads.list().len(), 2);
        if let crate::review::Content::Loaded(r) = &mut app.review.content {
            r.threads.clear();
        }
        app.review.generation += 1;
        app.pump();
        lines_at(&mut app, 120);
        assert!(diff(&app).threads.list().is_empty());
        assert!(diff(&app).threads.blocks().list().is_empty());
    }

    /// 20k lines, 500 changed ones with a thread each: drawing, wheel scrolling and caret moves
    /// stay cheap (a frame draws only the visible rows; the mapping is a binary search).
    #[tokio::test(flavor = "multi_thread")]
    async fn milestone_mr_review_threads_budget_20k_lines_500_threads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        init(&root);
        let text = |changed: bool| {
            (0..20_000)
                .map(|i| {
                    if changed && i % 40 == 0 {
                        format!("changed {i}\n")
                    } else {
                        format!("line {i}\n")
                    }
                })
                .collect::<String>()
        };
        fs::write(root.join("big.txt"), text(false)).unwrap();
        commit_all(&root, "base");
        let main = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&root, &["remote", "add", "origin", REMOTE]);
        git(&root, &["checkout", "-q", "-b", "feature"]);
        fs::write(root.join("big.txt"), text(true)).unwrap();
        commit_all(&root, "work");
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let comments = (0..500)
            .map(|k| {
                let body = "Check this line, it may be wrong when narrow";
                rc(
                    1000 + k,
                    "sam",
                    body,
                    "big.txt",
                    (k * 40 + 1) as u32,
                    &head,
                    "+x",
                    false,
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let server = with_comments(&main, &format!("[{comments}]"));
        let mut app = open_app(&root, None, config_for(&server), true);
        assert!(app.settle(loaded).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        screen(&mut app);
        open_file(&mut app, "big.txt").await;
        lines_at(&mut app, 120);
        assert_eq!(diff(&app).threads.blocks().list().len(), 500);
        let slack = if cfg!(debug_assertions) { 20 } else { 1 };
        let start = std::time::Instant::now();
        for _ in 0..50 {
            lines_at(&mut app, 120);
        }
        let draw = start.elapsed() / 50;
        let start = std::time::Instant::now();
        for _ in 0..300 {
            app.mouse(MouseEventKind::ScrollDown, 60, 10);
            lines_at(&mut app, 120);
        }
        let scroll = start.elapsed() / 300;
        let start = std::time::Instant::now();
        for _ in 0..300 {
            app.press(KeyCode::Down, KeyModifiers::NONE);
            app.test_execute(Command::ReviewNextThread);
        }
        let moves = start.elapsed() / 600;
        // A width change recomputes the heights of all 500 blocks once.
        let start = std::time::Instant::now();
        lines_at(&mut app, 80);
        let rewrap = start.elapsed();
        eprintln!("threads budget: draw {draw:?}, scroll+draw {scroll:?}, move {moves:?}, rewrap {rewrap:?}");
        let ms = std::time::Duration::from_millis;
        assert!(draw < ms(60 * slack), "draw {draw:?}");
        assert!(scroll < ms(80 * slack), "scroll {scroll:?}");
        assert!(moves < ms(10 * slack), "moves {moves:?}");
        assert!(rewrap < ms(200 * slack), "rewrap {rewrap:?}");
    }
}

/// mr_review-f1: the comment box under the selection, drafts kept in the state directory (across
/// a restart), the PENDING list and drafts held back when their lines change.
mod drafts {
    use super::*;
    use crate::diff_caret::Caret;
    use crossterm::event::{MouseButton, MouseEventKind};

    const FILE: &str = "review-drafts.json";

    fn lines(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect()
    }

    fn screen_text(app: &mut App) -> String {
        lines(app, 120, 30).join("\n")
    }

    fn row_of(l: &[String], needle: &str) -> usize {
        l.iter()
            .position(|x| x.contains(needle))
            .unwrap_or_else(|| panic!("no `{needle}` in\n{}", l.join("\n")))
    }

    fn col_of(line: &str, needle: &str) -> u16 {
        line.split(needle).next().unwrap().chars().count() as u16
    }

    fn message(app: &App) -> String {
        app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    /// The repository and a fake forge with one merge request without comments.
    fn world(dir: &Path) -> (PathBuf, String, FakeServer) {
        let (root, main) = make_repo(dir);
        let server = threads::with_comments(&main, "[]");
        (root, main, server)
    }

    /// An app on the world with `mod.txt`'s review diff open.
    async fn boot(root: &Path, server: &FakeServer, state: Option<PathBuf>) -> App {
        let mut app = open_app(root, state, config_for(server), true);
        assert!(app.settle(loaded).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        assert!(app.settle(|a| a.review.drafts.store.is_some()).await);
        screen(&mut app);
        open_file(&mut app, "mod.txt").await;
        draw_wide(&mut app, 120);
        app
    }

    /// The caret from `(first, 0)` to `(last, col)` of a side.
    fn select(app: &mut App, right: bool, first: usize, last: usize, col: usize) {
        app.group_mut().active_diff_mut().unwrap().go_to_side_line(right, last);
        draw_wide(app, 120);
        app.group_mut().active_diff_mut().unwrap().caret = Caret {
            right,
            anchor: (first, 0),
            head: (last, col),
            col: None,
        };
    }

    /// Lines 2-3 of the right side ("m2 changed", "m3") selected and a comment written.
    fn write_comment(app: &mut App, text: &str) {
        select(app, true, 1, 2, 2);
        app.test_execute(Command::ReviewComment);
        app.type_text(text);
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    }

    fn store_text(state: &Path) -> String {
        fs::read_to_string(state.join(FILE)).unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_comment_box_shows_the_typed_text_and_esc_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, Some(state.clone())).await;
        select(&mut app, true, 1, 2, 2);
        app.test_execute(Command::ReviewComment);
        assert_eq!(app.test_composer_text().as_deref(), Some(""));
        app.type_text("Why?");
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        app.type_text("Really");
        let l = lines(&mut app, 120, 30);
        assert!(l.iter().any(|x| x.contains("┌ Comment on lines 2–3 ─")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("│ Why?")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("│ Really")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("[Comment Ctrl+Enter] [Cancel Esc]")));
        // The box sits right under the last selected line, and grows with its text.
        let (m3, head) = (row_of(&l, "m3"), row_of(&l, "Comment on lines"));
        assert_eq!(head, m3 + 1, "{l:#?}");
        // Esc: gone, nothing kept.
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.test_composer_text(), None);
        let text = screen_text(&mut app);
        assert!(
            !text.contains("Comment on lines") && !text.contains("PENDING"),
            "{text}"
        );
        assert!(app.review.draft_rows().is_empty());
        idle(&mut app).await;
        assert_eq!(store_text(&state), "", "nothing was written");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_ctrl_enter_keeps_a_draft_in_the_diff_the_pending_list_and_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, Some(state.clone())).await;
        write_comment(&mut app, "This can underflow.");
        assert_eq!(app.test_composer_text(), None);
        let rows = app.review.draft_rows();
        assert_eq!(rows.len(), 1);
        let d = &rows[0].draft;
        assert_eq!((d.path.as_str(), d.right, d.start, d.end), ("mod.txt", true, 2, 3));
        assert_eq!((d.commit.as_str(), d.body.as_str()), ("h0", "This can underflow."));
        assert_eq!(d.lines, vec!["m2 changed", "m3"]);
        assert_eq!(rows[0].held, None);
        let l = lines(&mut app, 120, 30);
        assert!(l.iter().any(|x| x.contains("┌ Draft · lines 2–3")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("│ This can underflow.")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("PENDING (1)")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("● mod.txt:2  This can unde")), "{l:#?}");
        assert!(app.settle(|_| store_text(&state).contains("This can underflow.")).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_draft_is_kept_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, Some(state.clone())).await;
        write_comment(&mut app, "Keep me");
        assert!(app.settle(|_| store_text(&state).contains("Keep me")).await);
        drop(app);
        let mut app = boot(&root, &server, Some(state.clone())).await;
        assert!(app.settle(|a| !a.review.draft_rows().is_empty()).await);
        assert_eq!(app.review.draft_rows()[0].draft.body, "Keep me");
        let l = lines(&mut app, 120, 30);
        assert!(l.iter().any(|x| x.contains("┌ Draft · lines 2–3")), "{l:#?}");
        assert!(l.iter().any(|x| x.contains("PENDING (1)")), "{l:#?}");
        // Another merge request's drafts are not these.
        assert_eq!(app.review.draft_rows().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_left_side_comment_is_anchored_to_the_merge_base() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        // The deleted line "m2" (line 2 of the old file).
        select(&mut app, false, 1, 1, 2);
        app.test_execute(Command::ReviewComment);
        app.type_text("Why remove this?");
        app.press(KeyCode::Enter, KeyModifiers::ALT);
        let rows = app.review.draft_rows();
        let d = &rows[0].draft;
        assert_eq!(
            (d.right, d.start, d.end, d.commit.as_str()),
            (false, 2, 2, main.as_str())
        );
        assert_eq!(d.lines, vec!["m2"]);
        let l = lines(&mut app, 120, 30);
        let (removed, draft) = (row_of(&l, "m2 "), row_of(&l, "Draft · line 2"));
        assert!(draft > removed, "{l:#?}");
        assert!(l[draft + 1].contains("Why remove this?"), "{l:#?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_selection_ending_at_a_line_start_leaves_that_line_out() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        select(&mut app, true, 0, 2, 0);
        app.test_execute(Command::ReviewComment);
        app.type_text("x");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        let d = app.review.draft_rows()[0].draft.clone();
        assert_eq!((d.start, d.end), (1, 2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_an_empty_comment_is_not_saved_and_other_tabs_refuse_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        select(&mut app, true, 1, 1, 0);
        app.test_execute(Command::ReviewComment);
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(app.test_composer_text().is_some(), "still open");
        assert!(message(&app).contains("empty"), "{}", message(&app));
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.review.draft_rows().is_empty());
        // An ordinary diff tab is not a review's.
        app.open_diff(root.join("mod.txt"), crate::diff_view::Pair::UNSTAGED);
        app.test_execute(Command::ReviewComment);
        assert_eq!(app.test_composer_text(), None);
        assert!(
            message(&app).contains("Not in a code review's diff"),
            "{}",
            message(&app)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_local_edit_of_the_lines_holds_the_draft_back_and_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        write_comment(&mut app, "Check this");
        assert!(screen_text(&mut app).contains("Draft · lines 2–3"));
        // Typing in the diff edits line 2 of the file.
        select(&mut app, true, 1, 1, 0);
        app.type_text("X");
        assert!(app.settle(|a| a.review.draft_rows()[0].held.is_some()).await);
        let text = screen_text(&mut app);
        // The side panel is narrow: the reason is cut (`draft_rows` has it whole).
        assert_eq!(app.review.draft_rows()[0].held, Some("lines changed"));
        assert!(text.contains("held back: li"), "{text}");
        assert!(text.contains("mod.txt:2"), "{text}");
        assert!(
            !text.contains("Draft · lines 2–3"),
            "not drawn at a guessed place:\n{text}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_files_on_disk_hold_back_a_draft_after_a_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        write_comment(&mut app, "Check this");
        assert_eq!(app.review.draft_rows()[0].held, None);
        // No tab judges the file now; the next lookup reads it from disk.
        app.test_execute(Command::CloseTab);
        fs::write(root.join("mod.txt"), "m1\nm2 changed\nCHANGED\n").unwrap();
        app.test_execute(Command::ReviewRefresh);
        assert!(app.settle(|a| a.review.draft_rows()[0].held.is_some()).await);
        assert_eq!(app.review.draft_rows()[0].held, Some("lines changed"));
        let text = screen_text(&mut app);
        assert!(text.contains("held back: li"), "{text}");
        // A line that still reads the same stays placed.
        fs::write(root.join("mod.txt"), "m1\nm2 changed\nm3\n").unwrap();
        app.test_execute(Command::ReviewRefresh);
        assert!(app.settle(|a| a.review.draft_rows()[0].held.is_none()).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_delete_draft_removes_it_from_the_diff_the_list_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, Some(state.clone())).await;
        write_comment(&mut app, "Remove me");
        assert!(app.settle(|_| store_text(&state).contains("Remove me")).await);
        select(&mut app, true, 2, 2, 1);
        app.test_execute(Command::ReviewDeleteDraft);
        assert!(app.review.draft_rows().is_empty());
        let text = screen_text(&mut app);
        assert!(!text.contains("Draft ·") && !text.contains("PENDING"), "{text}");
        assert!(app.settle(|_| !store_text(&state).contains("Remove me")).await);
        // Nothing under the caret: said, nothing else happens.
        app.test_execute(Command::ReviewDeleteDraft);
        assert!(message(&app).contains("No draft here"), "{}", message(&app));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_pending_list_has_keys_to_open_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        write_comment(&mut app, "Pending one");
        app.test_execute(Command::CloseTab);
        app.ui.focus = crate::app::Focus::SidePanel;
        app.press(KeyCode::End, KeyModifiers::NONE);
        // Down from the last file: the one conversation, then PENDING.
        app.press(KeyCode::Down, KeyModifiers::NONE);
        assert!(app.review.view.conv.focused);
        app.press(KeyCode::Down, KeyModifiers::NONE);
        assert!(app.review.view.pending.focused && !app.review.view.conv.focused);
        // Enter opens the file at the draft's first line.
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|a| a.diffs_settled()).await);
        let d = diff(&app);
        assert!(d.path.ends_with("mod.txt"));
        assert_eq!(d.rows()[d.cursor].right, Some(1));
        // Delete removes it and gives the keys back to the files.
        app.ui.focus = crate::app::Focus::SidePanel;
        app.press(KeyCode::Delete, KeyModifiers::NONE);
        assert!(app.review.draft_rows().is_empty());
        assert!(!app.review.view.pending.focused);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_right_click_menus_fit_the_row_and_delete_a_pending_draft() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        write_comment(&mut app, "Pending one");
        // The caret leaves the draft's lines: the menu must act on the row clicked.
        select(&mut app, true, 5, 5, 0);
        lines(&mut app, 120, 30);
        let hit = app.hits.review.clone().unwrap();
        let right = MouseEventKind::Down(MouseButton::Right);
        // A draft: its own items only.
        app.mouse(right, hit.pending_list.x + 4, hit.pending_list.y);
        assert_eq!(
            super::menu_labels(&app),
            ["Go to Draft: on", "Edit Draft: on", "Delete Draft: on"]
        );
        assert!(app.review.view.pending.focused && !app.review.view.conv.focused);
        // Clicking Delete Draft (the fourth row, after a separator) deletes it.
        lines(&mut app, 120, 30);
        let menu = app.hits.context_menu.expect("menu drawn").rect;
        app.mouse(MouseEventKind::Down(MouseButton::Left), menu.x + 2, menu.y + 3);
        assert!(app.review.draft_rows().is_empty(), "{}", message(&app));
        assert!(message(&app).contains("Draft deleted"), "{}", message(&app));
        // A merge request comment under CONVERSATIONS: thread items, none available.
        lines(&mut app, 120, 30);
        let hit = app.hits.review.clone().unwrap();
        app.mouse(right, hit.conv_list.x + 4, hit.conv_list.y);
        let labels = super::menu_labels(&app);
        assert!(labels.iter().all(|l| l.ends_with(": off")), "{labels:?}");
        assert!(labels.contains(&"Reply: off".to_string()), "{labels:?}");
        assert!(app.review.view.conv.focused);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_pending_is_a_section_with_a_divider_buttons_and_a_collapsing_header() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        write_comment(&mut app, "Pending one");
        app.test_execute(Command::CloseTab);
        let l = lines(&mut app, 120, 30);
        let hit = app.hits.review.clone().unwrap();
        let (divider, pane) = *hit.dividers.last().unwrap();
        assert_eq!(pane, crate::review::Pane::Pending);
        assert_eq!(hit.pending_header.y, divider.y + 1);
        // The buttons wrap in a narrow panel: Request Changes on its own row.
        let first = &l[hit.pending_header.y as usize + 1];
        let second = &l[hit.pending_header.y as usize + 2];
        assert!(first.contains(" Submit ") && first.contains(" Approve "), "{l:#?}");
        assert!(second.contains(" Request Changes "), "{l:#?}");
        assert_eq!(hit.pending_list.y, hit.pending_header.y + 3);
        let commands: Vec<Command> = hit.pending_buttons.iter().map(|b| b.1).collect();
        assert_eq!(
            commands,
            [
                Command::ReviewSubmitComment,
                Command::ReviewSubmitApprove,
                Command::ReviewSubmitRequestChanges
            ]
        );
        assert_eq!(hit.pending_list.height as usize, app.review.view.pending.height);
        assert!(hit.pending_list.height >= 1);
        // A resize pointer over its divider; a drag up by two rows makes it two rows taller.
        app.mouse(MouseEventKind::Moved, divider.x + 5, divider.y);
        assert_eq!(app.pointer, crate::terminal::Pointer::RowResize);
        let before = hit.pending_list.height;
        app.mouse(MouseEventKind::Down(MouseButton::Left), divider.x + 5, divider.y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), divider.x + 5, divider.y - 2);
        app.mouse(MouseEventKind::Up(MouseButton::Left), divider.x + 5, divider.y - 2);
        lines(&mut app, 120, 30);
        let hit = app.hits.review.clone().unwrap();
        assert_eq!(hit.pending_list.height, before + 2);
        // The header collapses it to its divider and header, and opens it again.
        let header = hit.pending_header;
        app.mouse(MouseEventKind::Down(MouseButton::Left), header.x + 5, header.y);
        let text = lines(&mut app, 120, 30).join("\n");
        assert!(app.review.view.pending.collapsed);
        assert!(text.contains("▸ PENDING (1)") && !text.contains(" Submit "), "{text}");
        let hit = app.hits.review.clone().unwrap();
        assert!(hit.pending_buttons.is_empty() && hit.pending_list.height == 0);
        app.mouse(
            MouseEventKind::Down(MouseButton::Left),
            header.x + 5,
            hit.pending_header.y,
        );
        lines(&mut app, 120, 30);
        assert!(!app.review.view.pending.collapsed);
        // Submit runs Submit Review (Comment): here local HEAD is not the merge request's head.
        let hit = app.hits.review.clone().unwrap();
        let rect = hit.pending_buttons[0].0;
        app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, rect.y);
        assert!(message(&app).starts_with("Can't submit"), "{}", message(&app));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_edit_draft_reopens_it_and_saving_replaces_its_body() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        write_comment(&mut app, "first");
        let id = app.review.draft_rows()[0].draft.id.clone();
        select(&mut app, true, 1, 1, 0);
        app.test_execute(Command::ReviewEditDraft);
        assert_eq!(app.test_composer_text().as_deref(), Some("first"));
        let l = lines(&mut app, 120, 30);
        assert!(l.iter().any(|x| x.contains("Edit comment on lines 2–3")), "{l:#?}");
        assert!(
            !l.iter().any(|x| x.contains("Draft ·")),
            "the draft is hidden while edited"
        );
        // Esc keeps the draft as it was.
        app.type_text(" and more");
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.review.draft_rows()[0].draft.body, "first");
        app.test_execute(Command::ReviewEditDraft);
        app.type_text(" and more");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        let rows = app.review.draft_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].draft.id.as_str(), rows[0].draft.body.as_str()),
            (id.as_str(), "first and more")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_clicks_in_the_box_place_the_caret_and_press_its_buttons() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        select(&mut app, true, 1, 1, 0);
        app.test_execute(Command::ReviewComment);
        app.type_text("hello world");
        let l = lines(&mut app, 120, 30);
        let y = row_of(&l, "│ hello world");
        let x = col_of(&l[y], "world");
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y as u16);
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, y as u16);
        app.type_text("X");
        assert_eq!(app.test_composer_text().as_deref(), Some("hello Xworld"));
        // Submit by its button.
        let l = lines(&mut app, 120, 30);
        let y = row_of(&l, "[Comment Ctrl+Enter]");
        let x = col_of(&l[y], "[Comment") + 2;
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y as u16);
        assert_eq!(app.test_composer_text(), None);
        assert_eq!(app.review.draft_rows()[0].draft.body, "hello Xworld");
        // Cancel by its button.
        select(&mut app, true, 0, 0, 0);
        app.test_execute(Command::ReviewComment);
        let l = lines(&mut app, 120, 30);
        let y = row_of(&l, "[Cancel Esc]");
        let x = col_of(&l[y], "[Cancel") + 2;
        app.mouse(MouseEventKind::Down(MouseButton::Left), x, y as u16);
        assert_eq!(app.test_composer_text(), None);
        assert_eq!(app.review.draft_rows().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_the_box_takes_moves_paste_and_the_editor_keys() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _main, server) = world(dir.path());
        let mut app = boot(&root, &server, None).await;
        select(&mut app, true, 0, 0, 0);
        app.test_execute(Command::ReviewComment);
        app.type_text("abc");
        app.press(KeyCode::Left, KeyModifiers::NONE);
        app.press(KeyCode::Backspace, KeyModifiers::NONE);
        app.press(KeyCode::Home, KeyModifiers::NONE);
        app.press(KeyCode::Char('z'), KeyModifiers::NONE);
        app.handle_test_event(crossterm::event::Event::Paste("PASTED".into()));
        assert_eq!(app.test_composer_text().as_deref(), Some("zPASTEDac"));
        // The file's own text and caret were not touched.
        let (_, right) = left_right(&app);
        assert_eq!(right, "m1\nm2 changed\nm3\n");
        app.press(KeyCode::Esc, KeyModifiers::NONE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_corrupt_drafts_file_is_ignored_and_said_in_the_tool_log() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join(FILE), "{ nope").unwrap();
        let (root, _main, server) = world(dir.path());
        let app = boot(&root, &server, Some(state)).await;
        assert!(app.review.draft_rows().is_empty());
        assert!(app.logs.texts(&Tool::Review).iter().any(|l| l.contains("unreadable")));
    }
}

/// mr_review-f2: Submit Review, reply, resolve, edit and delete against the fake forge's canned
/// POST / PATCH / DELETE replies.
mod write {
    use super::*;
    use crate::diff_caret::Caret;
    use crossterm::event::{MouseButton, MouseEventKind};

    const FILE: &str = "review-drafts.json";
    const REVIEWS: &str = "/api/v1/repos/alice/project/pulls/7/reviews";

    fn lines(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect()
    }

    fn message(app: &App) -> String {
        app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    fn store_text(state: &Path) -> String {
        fs::read_to_string(state.join(FILE)).unwrap_or_default()
    }

    /// The fake forge: `!7` whose head is `mr_head`, sam's thread (11, line 3) and bob's resolved
    /// one (12, deleted line 2), the token's user `sam`, and `review` as the answer to a submit.
    fn server(main: &str, head: &str, mr_head: &str, review: Reply) -> FakeServer {
        let pr = pr_json(7, "Add review panel", main).replace(
            r#""ref":"feature","sha":"h0""#,
            &format!(r#""ref":"feature","sha":"{mr_head}""#),
        );
        let comments = format!(
            "[{},{}]",
            threads::rc(11, "sam", "Can this overflow", "mod.txt", 3, head, "+m3", false),
            threads::rc(12, "bob", "Why was this removed", "mod.txt", 2, main, "-m2", true)
        );
        FakeServer::start(vec![
            (
                format!("{PULLS}?state=open&page=1&limit=50"),
                Reply::json(&format!("[{pr}]")),
            ),
            (format!("{PULLS}/7"), Reply::json(&pr)),
            (
                format!("{PULLS}/7/reviews?page=1&limit=50"),
                Reply::json(r#"[{"id":1,"user":{"login":"bob"},"body":"","state":"COMMENT","comments_count":9}]"#),
            ),
            (format!("{PULLS}/7/reviews/1/comments"), Reply::json(&comments)),
            (
                "/api/v1/repos/alice/project/issues/7/comments?page=1&limit=50".into(),
                Reply::json("[]"),
            ),
            ("/api/v1/user".into(), Reply::json(r#"{"login":"sam"}"#)),
            (format!("POST {REVIEWS}"), review),
            (format!("POST {PULLS}/7/comments/11/replies"), Reply::json("{}")),
            (format!("POST {PULLS}/comments/11/resolve"), Reply::status(204)),
            (format!("POST {PULLS}/comments/12/unresolve"), Reply::status(204)),
            (
                "PATCH /api/v1/repos/alice/project/issues/comments/11".into(),
                Reply::json("{}"),
            ),
            (
                "DELETE /api/v1/repos/alice/project/issues/comments/11".into(),
                Reply::status(204),
            ),
        ])
    }

    /// The repository, its HEAD and main's commit.
    fn repo(dir: &Path) -> (PathBuf, String, String) {
        let (root, main) = make_repo(dir);
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        (root, head, main)
    }

    async fn boot(root: &Path, server: &FakeServer, state: Option<PathBuf>) -> App {
        let mut app = open_app(root, state, config_for(server), true);
        assert!(app.settle(loaded).await);
        assert!(app.settle(|a| a.git.status.is_some()).await);
        app.test_execute(Command::ShowReview);
        idle(&mut app).await;
        assert!(app.settle(|a| a.review.drafts.store.is_some()).await);
        assert!(app.settle(|a| a.review.user.is_some()).await);
        screen(&mut app);
        open_file(&mut app, "mod.txt").await;
        draw_wide(&mut app, 120);
        app
    }

    /// The caret on `first..=last` of a side.
    fn select(app: &mut App, right: bool, first: usize, last: usize) {
        app.group_mut().active_diff_mut().unwrap().go_to_side_line(right, last);
        draw_wide(app, 120);
        app.group_mut().active_diff_mut().unwrap().caret = Caret {
            right,
            anchor: (first, 0),
            head: (last, 1),
            col: None,
        };
    }

    fn comment(app: &mut App, first: usize, last: usize, text: &str) {
        select(app, true, first, last);
        app.test_execute(Command::ReviewComment);
        app.type_text(text);
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    }

    fn requests(server: &FakeServer, method: &str) -> Vec<crate::forge::fake::Request> {
        server.requests().into_iter().filter(|r| r.method == method).collect()
    }

    fn review_post(server: &FakeServer) -> Option<crate::forge::fake::Request> {
        requests(server, "POST").into_iter().find(|r| r.target == REVIEWS)
    }

    fn json(r: &crate::forge::fake::Request) -> serde_json::Value {
        serde_json::from_str(&r.body).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_submit_posts_one_review_and_removes_the_confirmed_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, Some(state.clone())).await;
        comment(&mut app, 1, 2, "Fix this range");
        assert!(app.settle(|_| store_text(&state).contains("Fix this range")).await);
        let before = lookups(&server);
        app.test_execute(Command::ReviewSubmitApprove);
        assert!(
            app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
                .await
        );
        idle(&mut app).await;
        let sent = review_post(&server).unwrap();
        assert_eq!(sent.header("authorization"), Some("token secret"));
        let v = json(&sent);
        assert_eq!(v["event"], "APPROVED");
        assert_eq!(v["commit_id"], head.as_str());
        assert_eq!(v["comments"].as_array().unwrap().len(), 1);
        let c = &v["comments"][0];
        assert_eq!(
            (c["path"].as_str(), c["new_position"].as_u64()),
            (Some("mod.txt"), Some(3))
        );
        assert_eq!(c["old_position"].as_u64(), Some(0));
        assert_eq!(c["body"], "Fix this range\n\n<!-- jed:range=2-3 -->");
        assert!(lookups(&server) > before, "the panel looks again");
        assert!(app.settle(|_| !store_text(&state).contains("Fix this range")).await);
        let screen = lines(&mut app, 120, 30).join("\n");
        assert!(!screen.contains("secret") && !screen.contains("PENDING"), "{screen}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_failed_submit_keeps_the_draft_over_a_restart_and_a_later_submit_posts_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, head, main) = repo(dir.path());
        let failing = Reply::status(500).with_body(r#"{"message":"boom secret"}"#);
        let server1 = server(&main, &head, &head, failing);
        let mut app = boot(&root, &server1, Some(state.clone())).await;
        comment(&mut app, 1, 2, "Keep me");
        assert!(app.settle(|_| store_text(&state).contains("Keep me")).await);
        app.test_execute(Command::ReviewSubmitComment);
        assert!(
            app.settle(|a| !a.review.writing && !a.review.drafts.errors.is_empty())
                .await
        );
        assert_eq!(app.review.draft_rows().len(), 1);
        let l = lines(&mut app, 160, 30).join("\n");
        assert!(l.contains("failed: serve"), "{l}");
        assert_eq!(
            app.review.drafts.errors.values().next().map(String::as_str),
            Some("server answered 500: boom <token>")
        );
        assert!(!l.contains("secret"), "{l}");
        assert!(!message(&app).contains("secret"));
        let log = app.logs.texts(&Tool::Review);
        assert!(log.iter().all(|t| !t.contains("secret")), "{log:?}");
        assert!(log.iter().any(|t| t.contains("500")), "{log:?}");
        assert!(store_text(&state).contains("Keep me"));
        drop(app);
        // A restart: the draft is still there, and a working server takes it.
        let server2 = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server2, Some(state.clone())).await;
        assert!(app.settle(|a| !a.review.draft_rows().is_empty()).await);
        app.test_execute(Command::ReviewSubmitComment);
        assert!(app.settle(|a| a.review.draft_rows().is_empty()).await);
        let v = json(&review_post(&server2).unwrap());
        assert_eq!(v["event"], "COMMENT");
        assert_eq!(v["comments"][0]["body"], "Keep me\n\n<!-- jed:range=2-3 -->");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_held_back_drafts_are_not_sent_and_a_verdict_alone_can_be_submitted() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        comment(&mut app, 1, 2, "held");
        comment(&mut app, 0, 0, "sent");
        let held = app
            .review
            .draft_rows()
            .iter()
            .find(|r| r.draft.body == "held")
            .unwrap()
            .draft
            .id
            .clone();
        app.review.drafts.live.insert(held, Some("the lines changed".into()));
        app.test_execute(Command::ReviewSubmitRequestChanges);
        assert!(
            app.settle(|a| !a.review.writing && a.review.draft_rows().len() == 1)
                .await
        );
        let v = json(&review_post(&server).unwrap());
        assert_eq!(v["event"], "REQUEST_CHANGES");
        assert_eq!(v["comments"].as_array().unwrap().len(), 1);
        assert_eq!(v["comments"][0]["body"], "sent");
        assert_eq!(app.review.draft_rows()[0].draft.body, "held");
        // Only a held back draft is left: Comment has nothing to send.
        idle(&mut app).await;
        let left = app.review.draft_rows()[0].draft.id.clone();
        app.review.drafts.live.insert(left, Some("the lines changed".into()));
        app.test_execute(Command::ReviewSubmitComment);
        assert!(message(&app).contains("held back"), "{}", message(&app));
        assert_eq!(
            requests(&server, "POST").iter().filter(|r| r.target == REVIEWS).count(),
            1
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_submit_refuses_when_local_head_is_not_the_merge_request_head() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        // The merge request is at main's commit: local is ahead of it.
        let server = server(&main, &head, &main, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        comment(&mut app, 1, 2, "x");
        app.test_execute(Command::ReviewSubmitComment);
        assert!(message(&app).contains("Can't submit"), "{}", message(&app));
        let l = lines(&mut app, 120, 30).join("\n");
        assert!(l.contains("local is ahead of the merge request: push"), "{l}");
        assert!(requests(&server, "POST").is_empty());
        assert_eq!(app.review.draft_rows().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_reply_posts_at_once_and_resolve_toggles_the_thread() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        select(&mut app, true, 2, 2);
        app.test_execute(Command::ReviewReply);
        assert!(app.test_composer_text().is_some());
        app.type_text("Yes, fixed");
        let l = lines(&mut app, 120, 30).join("\n");
        assert!(l.contains("Reply (posted at once)"), "{l}");
        let before = lookups(&server);
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(
            app.settle(|a| a.test_composer_text().is_none() && !a.review.writing)
                .await
        );
        let reply = requests(&server, "POST")
            .into_iter()
            .find(|r| r.target.ends_with("/comments/11/replies"))
            .unwrap();
        assert_eq!(reply.body, r#"{"body":"Yes, fixed"}"#);
        assert!(app.review.draft_rows().is_empty(), "a reply is not a draft");
        assert!(app.settle(|_| lookups(&server) > before).await);
        idle(&mut app).await;
        // Resolve: sam's thread is open; bob's (12, on the left) is resolved.
        select(&mut app, true, 2, 2);
        app.test_execute(Command::ReviewResolve);
        assert!(app.settle(|a| !a.review.writing).await);
        assert!(
            requests(&server, "POST")
                .iter()
                .any(|r| r.target.ends_with("/comments/11/resolve"))
        );
        idle(&mut app).await;
        select(&mut app, false, 1, 1);
        app.test_execute(Command::ReviewResolve);
        assert!(app.settle(|a| !a.review.writing).await);
        assert!(
            requests(&server, "POST")
                .iter()
                .any(|r| r.target.ends_with("/comments/12/unresolve"))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_only_your_own_comments_can_be_edited_or_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        // bob's thread: refused, no request.
        select(&mut app, false, 1, 1);
        app.test_execute(Command::ReviewDeleteComment);
        assert!(message(&app).contains("own comments"), "{}", message(&app));
        app.test_execute(Command::ReviewEditComment);
        assert_eq!(app.test_composer_text(), None);
        assert!(requests(&server, "DELETE").is_empty() && requests(&server, "PATCH").is_empty());
        // sam's: edit shows the text, saving patches the comment.
        select(&mut app, true, 2, 2);
        app.test_execute(Command::ReviewEditComment);
        assert_eq!(app.test_composer_text().as_deref(), Some("Can this overflow"));
        app.type_text("?");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(
            app.settle(|a| a.test_composer_text().is_none() && !a.review.writing)
                .await
        );
        assert_eq!(requests(&server, "PATCH")[0].body, r#"{"body":"Can this overflow?"}"#);
        idle(&mut app).await;
        select(&mut app, true, 2, 2);
        app.test_execute(Command::ReviewDeleteComment);
        assert!(matches!(
            app.prompt,
            Some(crate::app::Prompt::ConfirmDeleteComment { .. })
        ));
        // Declined: nothing is sent.
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.prompt.is_none());
        idle(&mut app).await;
        assert!(requests(&server, "DELETE").is_empty());
        app.test_execute(Command::ReviewDeleteComment);
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|_| !requests(&server, "DELETE").is_empty()).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_threads_show_buttons_and_a_click_on_reply_opens_the_box() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        let l = lines(&mut app, 120, 30);
        let row = l
            .iter()
            .position(|x| x.contains("[Reply] [Resolve] [Edit] [Delete]"))
            .unwrap_or_else(|| panic!("{l:#?}"));
        let col = l[row].split("[Reply]").next().unwrap().chars().count() as u16;
        app.mouse(MouseEventKind::Down(MouseButton::Left), col + 2, row as u16);
        assert!(app.test_composer_text().is_some());
        let l = lines(&mut app, 120, 30).join("\n");
        assert!(l.contains("Reply (posted at once)"), "{l}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mr_review_a_forge_error_on_a_reply_keeps_the_box_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        // bob's thread has no reply route on the fake: a 404.
        select(&mut app, false, 1, 1);
        app.test_execute(Command::ReviewReply);
        app.type_text("hello");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(app.settle(|a| a.review.error.is_some()).await);
        assert_eq!(app.test_composer_text().as_deref(), Some("hello"), "the text is kept");
        assert!(message(&app).contains("404"), "{}", message(&app));
        assert!(!app.review.writing);
    }
}
