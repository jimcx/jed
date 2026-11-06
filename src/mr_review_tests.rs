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
