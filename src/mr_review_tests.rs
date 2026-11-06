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
    assert!(text.contains("✎"), "the activity bar's icon: {text}");
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
    let list = app.hits.review.clone().unwrap().list;
    app.mouse(MouseEventKind::Down(MouseButton::Right), list.x + 8, list.y + 1);
    let text = screen(&mut app);
    for item in ["Toggle Viewed", "Refresh", "Switch Merge Request"] {
        assert!(text.contains(item), "{text}");
    }
    let files: Vec<String> = app.review.files().iter().map(|f| f.path.clone()).collect();
    assert_eq!(app.review.view.selected_path(), Some(files[1].as_str()));
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
        let (root, main) = make_cpp_repo(dir);
        let server = one_mr(&main);
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
    fn rc(
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
    fn with_comments(main: &str, comments: &str) -> FakeServer {
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
    async fn mr_review_the_divider_drags_and_the_header_collapses() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app(dir.path(), None).await;
        app.review.view.conv.section_height = 10;
        screen(&mut app);
        let hit = app.hits.review.clone().unwrap();
        assert!(hit.divider.height == 1 && hit.conv_list.height > 0);
        let before = app.review.view.conv.section_height;
        // Dragging the divider up by two rows makes the section two rows taller.
        app.mouse(
            MouseEventKind::Down(MouseButton::Left),
            hit.divider.x + 5,
            hit.divider.y,
        );
        app.mouse(
            MouseEventKind::Drag(MouseButton::Left),
            hit.divider.x + 5,
            hit.divider.y - 2,
        );
        app.mouse(
            MouseEventKind::Up(MouseButton::Left),
            hit.divider.x + 5,
            hit.divider.y - 2,
        );
        assert_eq!(app.review.view.conv.section_height, before + 2);
        screen(&mut app);
        let again = app.hits.review.clone().unwrap();
        assert_eq!(again.divider.y, hit.divider.y - 2);
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
}
