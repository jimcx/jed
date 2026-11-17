//! mr_review-h, the GitHub adapter: a `kind = "github"` entry pointing at the local fake server
//! (an explicit `http://` API URL, `/api/v3` as on Enterprise). The panel finds the pull request
//! (head filter, compare for the merge base), a multi-line thread from GraphQL sits over its real
//! range with no jed marker, and a draft is submitted in GitHub's shape (`line`, `side`,
//! `start_line`, `start_side`, `Bearer` token).

use super::*;
use crate::diff_caret::Caret;

const API: &str = "/api/v3/repos/alice/project";

pub(crate) fn config_github(server: &FakeServer) -> ForgeConfig {
    ForgeConfig {
        entries: vec![Entry {
            host: "git.example.com".into(),
            kind: Kind::GitHub,
            api_url: Some(format!("{}/api/v3", server.url())),
            token_env: Some("FAKE_TOKEN".into()),
            token: None,
            mcp_read_only: false,
        }],
        errors: Vec::new(),
    }
}

fn lines(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect()
}

pub(crate) fn graphql(connection: &str, nodes: &str) -> String {
    format!(
        r#"{{"data":{{"repository":{{"pullRequest":{{"{connection}":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":{nodes}}}}}}}}}}}"#
    )
}

pub(crate) fn server(main: &str, head: &str) -> FakeServer {
    server_with(main, head, true, false)
}

/// As `server`, but the thread says it has more than the 100 comments it returned.
pub(crate) fn server_with_long_thread(main: &str, head: &str) -> FakeServer {
    server_with(main, head, true, true)
}

/// `replies`: the fake takes a reply and a resolve mutation (else both are 404s).
fn server_with(main: &str, head: &str, replies: bool, long: bool) -> FakeServer {
    server_plus(main, head, replies, long, Vec::new())
}

/// As `server_with`, with `extra` routes too.
pub(crate) fn server_plus(
    main: &str,
    head: &str,
    replies: bool,
    long: bool,
    extra: Vec<(String, Reply)>,
) -> FakeServer {
    server_full(main, head, replies, long, false, "[]", extra)
}

/// As `server`, but GitHub calls the thread outdated (`isOutdated`, no current `line`, its
/// `originalLine`s on the original commit, as the real API says).
pub(crate) fn server_outdated(main: &str, head: &str) -> FakeServer {
    server_full(main, head, true, false, true, "[]", Vec::new())
}

/// As `server`, with these `reviews` nodes (a JSON array) and `extra` routes.
pub(crate) fn server_reviews(main: &str, head: &str, reviews: &str, extra: Vec<(String, Reply)>) -> FakeServer {
    server_full(main, head, true, false, false, reviews, extra)
}

fn server_full(
    main: &str,
    head: &str,
    replies: bool,
    long: bool,
    outdated: bool,
    reviews: &str,
    extra: Vec<(String, Reply)>,
) -> FakeServer {
    let (is_outdated, line, start) = if outdated {
        ("true", "null", "null")
    } else {
        ("false", "3", "2")
    };
    let page = if long {
        r#""pageInfo":{"hasNextPage":true},"#
    } else {
        ""
    };
    let pr = format!(
        r#"{{"number":7,"title":"Add review panel","body":null,"state":"open","draft":false,
        "user":{{"login":"ann"}},"html_url":"https://git.example.com/alice/project/pull/7",
        "base":{{"ref":"main","sha":"{main}","repo":{{"full_name":"alice/project"}}}},
        "head":{{"ref":"feature","sha":"{head}","repo":{{"full_name":"alice/project"}}}}}}"#
    );
    let thread = format!(
        r#"[{{"id":"PRRT_1","isResolved":false,"isOutdated":{is_outdated},"path":"mod.txt","line":{line},"startLine":{start},
        "originalLine":3,"originalStartLine":2,"diffSide":"RIGHT","comments":{{{page}"nodes":[
        {{"id":"PRRC_1","databaseId":501,"body":"Natively ranged remark","createdAt":"2026-01-01T00:00:00Z",
        "updatedAt":"2026-01-01T00:00:00Z","url":"","author":{{"login":"bob"}},
        "commit":{{"oid":"{head}"}},"originalCommit":{{"oid":"{head}"}}}}]}}}}]"#
    );
    let mut routes = Vec::new();
    if replies {
        routes.push((
            "POST /api/graphql ~addPullRequestReviewThreadReply".to_string(),
            Reply::json(r#"{"data":{"addPullRequestReviewThreadReply":{"comment":{"id":"x"}}}}"#),
        ));
        routes.push((
            "POST /api/graphql ~resolveReviewThread".to_string(),
            Reply::json(r#"{"data":{"resolveReviewThread":{"thread":{"id":"PRRT_1"}}}}"#),
        ));
    }
    routes.extend(vec![
        (
            format!("{API}/pulls?state=open&head=alice:feature&per_page=100"),
            Reply::json(&format!("[{pr}]")),
        ),
        (format!("{API}/pulls/7"), Reply::json(&pr)),
        (
            format!("{API}/compare/{main}...{head}"),
            Reply::json(&format!(r#"{{"merge_base_commit":{{"sha":"{main}"}}}}"#)),
        ),
        (format!("{API}/pulls/7/files"), Reply::json("[]")),
        ("/api/v3/user".into(), Reply::json(r#"{"login":"sam"}"#)),
        (
            "POST /api/graphql ~reviewThreads".into(),
            Reply::json(&graphql("reviewThreads", &thread)),
        ),
        (
            "POST /api/graphql ~reviews(".into(),
            Reply::json(&graphql("reviews", reviews)),
        ),
        (
            "POST /api/graphql ~comments(first:100,after".into(),
            Reply::json(&graphql("comments", "[]")),
        ),
        (format!("POST {API}/pulls/7/reviews"), Reply::json("{}")),
    ]);
    routes.extend(extra);
    FakeServer::start(routes)
}

/// Claude's reply draft to `thread`, as the MCP tool stores it.
fn claude_reply(app: &mut App, thread: &str, body: &str, resolve: bool) {
    let key = app.review.drafts_key().unwrap();
    let mut d = crate::review_drafts::Draft::reply("mod.txt".into(), true, (2, 3), thread.into(), body.into(), resolve);
    d.by_claude = true;
    app.review.drafts.store.as_mut().unwrap().set_reply(&key, d);
    assert!(!app.review.draft_rows().is_empty());
}

pub(crate) async fn boot_github(root: &Path, server: &FakeServer) -> App {
    let mut app = open_app(root, None, config_github(server), true);
    assert!(app.settle(loaded).await);
    assert!(app.settle(|a| a.git.status.is_some()).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    assert!(app.settle(|a| a.review.drafts.store.is_some()).await);
    app
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_github_posts_a_reply_draft_and_resolves_at_submit_without_a_review() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let server = server(&main, &head);
    let mut app = boot_github(&root, &server).await;
    claude_reply(&mut app, "PRRT_1", "Done in abc", true);
    let writes = |s: &FakeServer| {
        s.requests()
            .into_iter()
            .filter(|r| r.body.contains("mutation") || r.target.ends_with("/reviews"))
            .collect::<Vec<_>>()
    };
    assert!(writes(&server).is_empty(), "drafting posts nothing");
    app.test_submit(Command::ReviewSubmitComment);
    assert!(
        app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
            .await
    );
    // The graphql reads are POSTs too: a write is a mutation or a review post.
    let sent = writes(&server);
    assert_eq!(sent.len(), 2, "{:?}", sent.iter().map(|r| &r.body).collect::<Vec<_>>());
    assert!(sent[0].body.contains("addPullRequestReviewThreadReply") && sent[0].body.contains("Done in abc"));
    assert!(sent[0].body.contains("PRRT_1"));
    assert!(sent[1].body.contains("resolveReviewThread") && sent[1].body.contains("PRRT_1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_github_keeps_a_reply_draft_whose_post_failed() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let server = server_with(&main, &head, false, false);
    let mut app = boot_github(&root, &server).await;
    claude_reply(&mut app, "PRRT_1", "Will fail", true);
    app.test_submit(Command::ReviewSubmitComment);
    assert!(
        app.settle(|a| !a.review.writing && !a.review.drafts.errors.is_empty())
            .await
    );
    assert_eq!(app.review.draft_rows().len(), 1, "kept for another try");
    assert!(lines(&mut app, 140, 30).join("\n").contains("failed:"));
    let writes: Vec<_> = server.requests().into_iter().filter(|r| r.method != "GET").collect();
    assert!(
        writes
            .iter()
            .all(|r| !r.body.contains("resolveReviewThread") && !r.target.ends_with("/reviews")),
        "a failed reply neither resolves nor reviews"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_github_shows_a_multi_line_thread_and_submits_in_github_shape() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let server = server(&main, &head);
    let mut app = open_app(&root, None, config_github(&server), true);
    assert!(app.settle(loaded).await);
    assert!(app.settle(|a| a.git.status.is_some()).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    assert!(app.settle(|a| a.review.drafts.store.is_some()).await);
    assert!(app.settle(|a| a.review.user.is_some()).await);
    screen(&mut app);
    open_file(&mut app, "mod.txt").await;
    draw_wide(&mut app, 120);

    let seen = server.requests();
    let first = seen.iter().find(|r| r.target.contains("/pulls?state=open")).unwrap();
    assert_eq!(first.header("authorization"), Some("Bearer secret"));
    assert_eq!(first.header("x-github-api-version"), Some("2022-11-28"));
    assert!(seen.iter().any(|r| r.target.starts_with(&format!("{API}/compare/"))));

    // The thread is placed over its range (lines 2-3 of the new file), with no marker anywhere.
    let screen_text = lines(&mut app, 120, 30).join("\n");
    assert!(screen_text.contains("Natively ranged remark"), "{screen_text}");
    assert!(!screen_text.contains("jed:range"), "{screen_text}");
    let t = &app.review.review().unwrap().threads[0];
    assert_eq!((t.thread.anchor.start_line, t.thread.anchor.end_line), (2, 3));

    // A multi-line draft goes out as one review in GitHub's shape.
    app.group_mut().active_diff_mut().unwrap().go_to_side_line(true, 2);
    draw_wide(&mut app, 120);
    app.group_mut().active_diff_mut().unwrap().caret = Caret {
        right: true,
        anchor: (1, 0),
        head: (2, 1),
        col: None,
    };
    app.test_execute(Command::ReviewComment);
    app.type_text("Please rename");
    app.press_bound(Command::ReviewSaveComment);
    app.test_submit(Command::ReviewSubmitApprove);
    assert!(
        app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
            .await
    );
    let sent = server
        .requests()
        .into_iter()
        .find(|r| r.method == "POST" && r.target.ends_with("/pulls/7/reviews"))
        .expect("the review was posted");
    assert_eq!(sent.header("authorization"), Some("Bearer secret"));
    let v: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(
        (v["event"].as_str(), v["commit_id"].as_str()),
        (Some("APPROVE"), Some(head.as_str()))
    );
    assert_eq!(
        v["comments"][0],
        serde_json::json!({
            "path": "mod.txt", "body": "Please rename", "line": 3, "side": "RIGHT",
            "start_line": 2, "start_side": "RIGHT"
        })
    );
}
