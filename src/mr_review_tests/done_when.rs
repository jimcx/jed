//! mr_review-i, the "Done when" walk of the `mr_review` task, once per forge: the panel lists the
//! merge request and its files, a file opens in an inline diff tab, other people's threads show
//! inline (an old one under CONVERSATIONS), a comment on selected lines is kept as a draft across
//! a failed submit and a restart and then posted by Submit Review. The language navigation and the
//! suggestions are forge-independent and walk once, on Gitea (`lsp::mr_review_done_when_*`,
//! `suggest::mr_review_done_when_*`). `.workspace` can't set a host, API URL or token (last test).

use super::*;
use crate::diff_caret::Caret;
use crate::forge::fake::Request;

const STORE: &str = "review-drafts.json";
const GITEA_REVIEWS: &str = "/api/v1/repos/alice/project/pulls/7/reviews";
const GITHUB: &str = "/api/v3/repos/alice/project";

fn screen_text(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn store_text(state: &Path) -> String {
    fs::read_to_string(state.join(STORE)).unwrap_or_default()
}

fn gitea_server(main: &str, head: &str, review: Reply) -> FakeServer {
    let comments = format!(
        "[{},{}]",
        threads::rc(11, "sam", "Natively ranged remark", "mod.txt", 3, head, "+m3", false),
        threads::rc(13, "cy", "Stale remark", "mod.txt", 1, &"d".repeat(40), "+m1", false),
    );
    let pr = pr_json(7, "Add review panel", main).replace(
        r#""ref":"feature","sha":"h0""#,
        &format!(r#""ref":"feature","sha":"{head}""#),
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
            Reply::json(
                r#"[{"id":99,"user":{"login":"ann"},"body":"Looks good overall","created_at":"2026-01-01T10:00:00Z","updated_at":"","html_url":"u"}]"#,
            ),
        ),
        ("/api/v1/user".into(), Reply::json(r#"{"login":"sam"}"#)),
        (format!("POST {GITEA_REVIEWS}"), review),
    ])
}

fn github_server(main: &str, head: &str, review: Reply) -> FakeServer {
    let pr = format!(
        r#"{{"number":7,"title":"Add review panel","body":null,"state":"open","draft":false,
        "user":{{"login":"ann"}},"html_url":"https://git.example.com/alice/project/pull/7",
        "base":{{"ref":"main","sha":"{main}","repo":{{"full_name":"alice/project"}}}},
        "head":{{"ref":"feature","sha":"{head}","repo":{{"full_name":"alice/project"}}}}}}"#
    );
    let old = "d".repeat(40);
    let thread = |id: &str, db: u32, body: &str, line: u32, start: u32, outdated: bool, commit: &str| {
        format!(
            r#"{{"id":"{id}","isResolved":false,"isOutdated":{outdated},"path":"mod.txt","line":{line},"startLine":{start},
            "originalLine":{line},"originalStartLine":{start},"diffSide":"RIGHT","comments":{{"nodes":[
            {{"id":"C{db}","databaseId":{db},"body":"{body}","createdAt":"2026-01-01T00:00:00Z",
            "updatedAt":"2026-01-01T00:00:00Z","url":"","author":{{"login":"bob"}},
            "commit":{{"oid":"{commit}"}},"originalCommit":{{"oid":"{commit}"}}}}]}}}}"#
        )
    };
    let threads = format!(
        "[{},{}]",
        thread("PRRT_1", 501, "Natively ranged remark", 3, 2, false, head),
        thread("PRRT_2", 502, "Stale remark", 1, 1, true, &old),
    );
    let conn = |name: &str, nodes: &str| {
        Reply::json(&format!(
            r#"{{"data":{{"repository":{{"pullRequest":{{"{name}":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":{nodes}}}}}}}}}}}"#
        ))
    };
    FakeServer::start(vec![
        (
            format!("{GITHUB}/pulls?state=open&head=alice:feature&per_page=100"),
            Reply::json(&format!("[{pr}]")),
        ),
        (format!("{GITHUB}/pulls/7"), Reply::json(&pr)),
        (
            format!("{GITHUB}/compare/{main}...{head}"),
            Reply::json(&format!(r#"{{"merge_base_commit":{{"sha":"{main}"}}}}"#)),
        ),
        (format!("{GITHUB}/pulls/7/files"), Reply::json("[]")),
        ("/api/v3/user".into(), Reply::json(r#"{"login":"sam"}"#)),
        (
            "POST /api/graphql ~reviewThreads".into(),
            conn("reviewThreads", &threads),
        ),
        ("POST /api/graphql ~reviews(".into(), conn("reviews", "[]")),
        (
            "POST /api/graphql ~comments(first:100,after".into(),
            conn(
                "comments",
                r#"[{"databaseId":99,"body":"Looks good overall","createdAt":"2026-01-01T00:00:00Z","updatedAt":"","url":"","author":{"login":"ann"}}]"#,
            ),
        ),
        (format!("POST {GITHUB}/pulls/7/reviews"), review),
    ])
}

fn config_github(server: &FakeServer) -> ForgeConfig {
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

async fn boot(root: &Path, config: ForgeConfig, state: &Path) -> App {
    let mut app = open_app(root, Some(state.to_path_buf()), config, true);
    assert!(app.settle(loaded).await);
    assert!(app.settle(|a| a.git.status.is_some()).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    assert!(app.settle(|a| a.review.drafts.store.is_some()).await);
    assert!(app.settle(|a| a.review.user.is_some()).await);
    app.review.view.conv.section_height = 16;
    screen(&mut app);
    app
}

/// The walk: `serve(main, head, review_reply)` builds the forge's fake server, `config` points a
/// client at it, `posted` checks what the successful submit sent.
async fn walk(
    serve: impl Fn(&str, &str, Reply) -> FakeServer,
    config: fn(&FakeServer) -> ForgeConfig,
    posted: impl Fn(&[Request], &str),
) {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();

    // The panel lists the merge request and its files since the merge base.
    let failing = serve(&main, &head, Reply::status(500).with_body(r#"{"message":"boom"}"#));
    let mut app = boot(&root, config(&failing), &state).await;
    let panel = screen(&mut app);
    for want in ["Add review panel", "mod.txt", "4 files"] {
        assert!(panel.contains(want), "{want}: {panel}");
    }

    // A file opens in an inline diff tab; the others' threads show inline, the old one under
    // CONVERSATIONS.
    open_file(&mut app, "mod.txt").await;
    draw_wide(&mut app, 120);
    assert!(app.group().active_diff().is_some());
    let text = screen_text(&mut app, 120, 30);
    assert!(text.contains("Natively ranged remark"), "{text}");
    let panel = screen(&mut app);
    assert!(
        panel.contains("Stale remark") && panel.contains("Looks good overall"),
        "{panel}"
    );

    // A comment on selected lines is a draft, kept over a failed submit.
    app.group_mut().active_diff_mut().unwrap().go_to_side_line(true, 2);
    draw_wide(&mut app, 120);
    app.group_mut().active_diff_mut().unwrap().caret = Caret {
        right: true,
        anchor: (1, 0),
        head: (2, 1),
        col: None,
    };
    app.test_execute(Command::ReviewComment);
    app.type_text("Walk draft");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(app.settle(|_| store_text(&state).contains("Walk draft")).await);
    app.test_execute(Command::ReviewSubmitComment);
    assert!(
        app.settle(|a| !a.review.writing && !a.review.drafts.errors.is_empty())
            .await
    );
    assert_eq!(app.review.draft_rows().len(), 1, "the failed draft stays");
    assert!(store_text(&state).contains("Walk draft"));
    drop(app);

    // A restart keeps it; Submit Review then posts it.
    let working = serve(&main, &head, Reply::json("{}"));
    let mut app = boot(&root, config(&working), &state).await;
    assert!(app.settle(|a| !a.review.draft_rows().is_empty()).await);
    app.test_execute(Command::ReviewSubmitComment);
    assert!(
        app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
            .await
    );
    assert!(app.settle(|_| !store_text(&state).contains("Walk draft")).await);
    posted(&working.requests(), &head);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_done_when_gitea() {
    walk(gitea_server, config_for, |reqs, head| {
        let sent = reqs
            .iter()
            .find(|r| r.method == "POST" && r.target == GITEA_REVIEWS)
            .expect("the review was posted");
        let v: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(
            (v["event"].as_str(), v["commit_id"].as_str()),
            (Some("COMMENT"), Some(head))
        );
        assert_eq!(v["comments"][0]["body"], "Walk draft\n\n<!-- jed:range=2-3 -->");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_done_when_github() {
    walk(github_server, config_github, |reqs, head| {
        let sent = reqs
            .iter()
            .find(|r| r.method == "POST" && r.target == format!("{GITHUB}/pulls/7/reviews"))
            .expect("the review was posted");
        let v: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!((v["event"].as_str(), v["commit_id"].as_str()), (Some("COMMENT"), Some(head)));
        assert_eq!(
            v["comments"][0],
            serde_json::json!({"path":"mod.txt","body":"Walk draft","line":3,"side":"RIGHT","start_line":2,"start_side":"RIGHT"})
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_done_when_gitlab() {
    walk(gitlab::server, gitlab::config_gitlab, |reqs, _head| {
        let draft = reqs
            .iter()
            .find(|r| r.method == "POST" && r.target.ends_with("/draft_notes"))
            .expect("a draft note was posted");
        let v: serde_json::Value = serde_json::from_str(&draft.body).unwrap();
        assert_eq!(v["note"], "Walk draft");
        assert_eq!(v["position"]["line_range"]["start"]["new_line"], 2);
        assert!(reqs.iter().any(|r| r.target.ends_with("/draft_notes/bulk_publish")));
        assert!(
            !reqs.iter().any(|r| r.target.ends_with("/approve")),
            "Comment does not approve"
        );
    })
    .await;
}

/// `.workspace` can't choose a host, an API URL or a token variable: with a `[forge]` table (and
/// the same keys at the top level) pointing at a local server and at a variable that exists, the
/// host stays unknown, nothing is asked of that server and no variable is read.
#[tokio::test(flavor = "multi_thread")]
async fn mr_review_done_when_workspace_cannot_set_a_host_or_token() {
    let dir = tempfile::tempdir().unwrap();
    let (root, _main) = make_repo(dir.path());
    let evil = FakeServer::start(vec![]);
    let url = evil.url();
    fs::write(
        root.join(".workspace"),
        format!(
            "host = \"git.example.com\"\nkind = \"gitea\"\napi_url = \"{url}/api/v1\"\ntoken_env = \"PATH\"\n\
             [forge]\nhost = \"git.example.com\"\nkind = \"gitea\"\napi_url = \"{url}/api/v1\"\ntoken_env = \"PATH\"\n\
             [[forge]]\nhost = \"git.example.com\"\nkind = \"gitlab\"\napi_url = \"{url}/api/v4\"\ntoken_env = \"HOME\"\n"
        ),
    )
    .unwrap();
    let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let base = workspace::normalize(&root);
    let trust = TrustStore::load(base.join("trust.toml"));
    let mut app = App::new(Workspace::load(base), trust, None, true);
    app.review.config = Some(Arc::new(ForgeConfig::default()));
    let log = asked.clone();
    app.review.env = Arc::new(move |name| {
        log.lock().unwrap().push(name.to_string());
        Some("secret".to_string())
    });
    app.review.auto = true;
    app.test_start();
    app.test_execute(Command::ShowReview);
    let want = Content::Empty(Empty::UnknownHost("git.example.com".into()));
    assert!(app.settle(|a| a.review.content == want).await);
    idle(&mut app).await;
    assert!(evil.requests().is_empty(), "no request reached the workspace's server");
    assert!(
        asked.lock().unwrap().is_empty(),
        "no variable was read: {:?}",
        asked.lock().unwrap()
    );
}
