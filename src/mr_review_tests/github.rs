//! mr_review-h, the GitHub adapter: a `kind = "github"` entry pointing at the local fake server
//! (an explicit `http://` API URL, `/api/v3` as on Enterprise). The panel finds the pull request
//! (head filter, compare for the merge base), a multi-line thread from GraphQL sits over its real
//! range with no jed marker, and a draft is submitted in GitHub's shape (`line`, `side`,
//! `start_line`, `start_side`, `Bearer` token).

use super::*;
use crate::diff_caret::Caret;

const API: &str = "/api/v3/repos/alice/project";

fn config_github(server: &FakeServer) -> ForgeConfig {
    ForgeConfig {
        entries: vec![Entry {
            host: "git.example.com".into(),
            kind: Kind::GitHub,
            api_url: Some(format!("{}/api/v3", server.url())),
            token_env: Some("FAKE_TOKEN".into()),
            token: None,
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

fn graphql(connection: &str, nodes: &str) -> String {
    format!(
        r#"{{"data":{{"repository":{{"pullRequest":{{"{connection}":{{"pageInfo":{{"hasNextPage":false,"endCursor":null}},"nodes":{nodes}}}}}}}}}}}"#
    )
}

fn server(main: &str, head: &str) -> FakeServer {
    let pr = format!(
        r#"{{"number":7,"title":"Add review panel","body":null,"state":"open","draft":false,
        "user":{{"login":"ann"}},"html_url":"https://git.example.com/alice/project/pull/7",
        "base":{{"ref":"main","sha":"{main}","repo":{{"full_name":"alice/project"}}}},
        "head":{{"ref":"feature","sha":"{head}","repo":{{"full_name":"alice/project"}}}}}}"#
    );
    let thread = format!(
        r#"[{{"id":"PRRT_1","isResolved":false,"isOutdated":false,"path":"mod.txt","line":3,"startLine":2,
        "originalLine":3,"originalStartLine":2,"diffSide":"RIGHT","comments":{{"nodes":[
        {{"id":"PRRC_1","databaseId":501,"body":"Natively ranged remark","createdAt":"2026-01-01T00:00:00Z",
        "updatedAt":"2026-01-01T00:00:00Z","url":"","author":{{"login":"bob"}},
        "commit":{{"oid":"{head}"}},"originalCommit":{{"oid":"{head}"}}}}]}}}}]"#
    );
    FakeServer::start(vec![
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
            Reply::json(&graphql("reviews", "[]")),
        ),
        (
            "POST /api/graphql ~comments(first:100,after".into(),
            Reply::json(&graphql("comments", "[]")),
        ),
        (format!("POST {API}/pulls/7/reviews"), Reply::json("{}")),
    ])
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
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    app.test_execute(Command::ReviewSubmitApprove);
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
