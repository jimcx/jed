//! mr_review-i, the GitLab adapter: a `kind = "gitlab"` entry pointing at the local fake server (an
//! explicit `http://` API URL ending in `/api/v4`, the project path encoded as one segment). The
//! panel finds the merge request (`source_branch`, `diff_refs` for the merge base), a multi-line
//! discussion (`line_range`) sits over its real range with no jed marker, an old commit's
//! discussion goes under CONVERSATIONS, and a draft is submitted in GitLab's shape (draft notes,
//! `bulk_publish`, `approve`, `PRIVATE-TOKEN`).

use super::*;
use crate::diff_caret::Caret;

pub(super) const API: &str = "/api/v4/projects/alice%2Fproject";

pub(super) fn config_gitlab(server: &FakeServer) -> ForgeConfig {
    ForgeConfig {
        entries: vec![Entry {
            host: "git.example.com".into(),
            kind: Kind::GitLab,
            api_url: Some(format!("{}/api/v4", server.url())),
            token_env: Some("FAKE_TOKEN".into()),
            token: None,
            mcp_read_only: false,
        }],
        errors: Vec::new(),
    }
}

/// The merge request `!7` at `head`, a multi-line discussion on `mod.txt` lines 2-3 at `head`, one
/// made at an old commit, a plain comment and a system note. `publish` answers `bulk_publish`.
pub(super) fn server(main: &str, head: &str, publish: Reply) -> FakeServer {
    let mr = format!(
        r#"{{"iid":7,"title":"Add review panel","description":"","state":"opened","draft":false,
        "author":{{"username":"ann"}},"web_url":"https://git.example.com/alice/project/-/merge_requests/7",
        "source_branch":"feature","target_branch":"main","sha":"{head}",
        "diff_refs":{{"base_sha":"{main}","start_sha":"{main}","head_sha":"{head}"}}}}"#
    );
    let old = "d".repeat(40);
    let discussions = format!(
        r#"[{{"id":"aa11","individual_note":false,"notes":[{{"id":101,"body":"Natively ranged remark",
        "author":{{"username":"bob"}},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z",
        "system":false,"resolvable":true,"resolved":false,"position":{{"head_sha":"{head}","base_sha":"{main}",
        "start_sha":"{main}","old_path":"mod.txt","new_path":"mod.txt","position_type":"text","new_line":3,
        "old_line":null,"line_range":{{"start":{{"line_code":"x_0_2","type":"new","old_line":null,"new_line":2}},
        "end":{{"line_code":"x_0_3","type":"new","old_line":null,"new_line":3}}}}}}}}]}},
        {{"id":"bb22","individual_note":false,"notes":[{{"id":102,"body":"Stale remark",
        "author":{{"username":"cy"}},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z",
        "system":false,"resolvable":true,"resolved":false,"position":{{"head_sha":"{old}","old_path":"mod.txt",
        "new_path":"mod.txt","position_type":"text","new_line":1,"old_line":null}}}}]}},
        {{"id":"cc33","individual_note":true,"notes":[{{"id":103,"body":"Looks good overall",
        "author":{{"username":"ann"}},"created_at":"2026-01-02T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","system":false}}]}},
        {{"id":"dd44","individual_note":true,"notes":[{{"id":104,"body":"added 1 commit","author":{{"username":"ann"}},"system":true}}]}}]"#
    );
    FakeServer::start(vec![
        (
            format!("{API}/merge_requests?state=opened&source_branch=feature&per_page=100&page=1"),
            Reply::json(&format!("[{mr}]")),
        ),
        (format!("{API}/merge_requests/7"), Reply::json(&mr)),
        (format!("{API}/merge_requests/7/diffs"), Reply::json("[]")),
        (format!("{API}/merge_requests/7/draft_notes"), Reply::json("[]")),
        (format!("{API}/merge_requests/7/discussions"), Reply::json(&discussions)),
        ("/api/v4/user".into(), Reply::json(r#"{"username":"sam"}"#)),
        (
            format!("POST {API}/merge_requests/7/draft_notes"),
            Reply::json(r#"{"id":31}"#),
        ),
        (format!("POST {API}/merge_requests/7/draft_notes/bulk_publish"), publish),
        (format!("POST {API}/merge_requests/7/approve"), Reply::json("{}")),
        (
            format!("POST {API}/merge_requests/7/discussions/aa11/notes"),
            Reply::status(201).with_body("{}"),
        ),
        (
            format!("PUT {API}/merge_requests/7/discussions/aa11"),
            Reply::json("{}"),
        ),
    ])
}

/// The panel loaded on the standard repository against `server`, `mod.txt`'s diff open.
pub(super) async fn boot(root: &Path, server: &FakeServer, state: Option<PathBuf>) -> App {
    let mut app = open_app(root, state, config_gitlab(server), true);
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

fn lines(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_gitlab_shows_a_multi_line_discussion_and_submits_in_gitlab_shape() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let server = server(&main, &head, Reply::status(204));
    let mut app = boot(&root, &server, None).await;

    let seen = server.requests();
    let first = seen
        .iter()
        .find(|r| r.target.contains("/merge_requests?state=opened"))
        .unwrap();
    assert_eq!(first.header("private-token"), Some("secret"));
    assert_eq!(first.header("authorization"), None);

    // The discussion is placed over its range (lines 2-3 of the new file), with no marker.
    let text = lines(&mut app, 120, 30);
    assert!(text.contains("Natively ranged remark"), "{text}");
    assert!(!text.contains("jed:range"), "{text}");
    let r = app.review.review().unwrap();
    let t = r.threads.iter().find(|t| t.thread.id == "7:aa11").unwrap();
    assert_eq!((t.thread.anchor.start_line, t.thread.anchor.end_line), (2, 3));
    // The one made at an old commit, and the plain comment, are listed under CONVERSATIONS.
    let panel = screen(&mut app);
    assert!(panel.contains("Stale remark"), "{panel}");
    assert!(panel.contains("Looks good overall"), "{panel}");
    assert!(!panel.contains("added 1 commit"), "system notes are skipped: {panel}");

    // A multi-line draft goes out as draft notes, published together, then approved.
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
    let posts: Vec<_> = server.requests().into_iter().filter(|r| r.method == "POST").collect();
    let draft = posts
        .iter()
        .find(|r| r.target.ends_with("/draft_notes"))
        .expect("a draft note");
    assert_eq!(draft.header("private-token"), Some("secret"));
    let v: serde_json::Value = serde_json::from_str(&draft.body).unwrap();
    assert_eq!(v["note"], "Please rename");
    let p = &v["position"];
    assert_eq!(
        (
            p["position_type"].as_str(),
            p["new_path"].as_str(),
            p["new_line"].as_u64()
        ),
        (Some("text"), Some("mod.txt"), Some(3))
    );
    assert_eq!(
        (p["base_sha"].as_str(), p["start_sha"].as_str(), p["head_sha"].as_str()),
        (Some(main.as_str()), Some(main.as_str()), Some(head.as_str()))
    );
    assert_eq!(p["line_range"]["start"]["new_line"], 2);
    assert_eq!(p["line_range"]["end"]["type"], "new");
    let order: Vec<_> = posts
        .iter()
        .map(|r| r.target.rsplit('/').next().unwrap().to_string())
        .collect();
    assert_eq!(order, ["draft_notes", "bulk_publish", "approve"]);
    let approve = posts.iter().find(|r| r.target.ends_with("/approve")).unwrap();
    assert_eq!(approve.body, format!(r#"{{"sha":"{head}"}}"#));
}

/// Claude's reply draft to `thread`, as the MCP tool stores it.
fn claude_reply(app: &mut App, thread: &str, body: &str, resolve: bool) {
    let key = app.review.drafts_key().unwrap();
    let mut d = crate::review_drafts::Draft::reply("mod.txt".into(), true, (2, 3), thread.into(), body.into(), resolve);
    d.by_claude = true;
    app.review.drafts.store.as_mut().unwrap().set_reply(&key, d);
    assert!(!app.review.draft_rows().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_gitlab_posts_a_reply_draft_and_resolves_at_submit_without_a_review() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let server = server(&main, &head, Reply::status(204));
    let mut app = boot(&root, &server, None).await;
    claude_reply(&mut app, "7:aa11", "Done in abc", true);
    let writes = |s: &FakeServer| {
        s.requests()
            .into_iter()
            .filter(|r| r.method != "GET")
            .collect::<Vec<_>>()
    };
    assert!(writes(&server).is_empty(), "drafting posts nothing");
    app.test_execute(Command::ReviewSubmitComment);
    assert!(
        app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
            .await
    );
    let sent = writes(&server);
    let targets: Vec<_> = sent.iter().map(|r| format!("{} {}", r.method, r.target)).collect();
    assert_eq!(
        targets,
        vec![
            format!("POST {API}/merge_requests/7/discussions/aa11/notes"),
            format!("PUT {API}/merge_requests/7/discussions/aa11"),
        ],
        "no draft note, publish or approval was forged"
    );
    assert_eq!(sent[0].body, r#"{"body":"Done in abc"}"#);
    assert_eq!(sent[1].body, r#"{"resolved":true}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_ide_gitlab_keeps_a_reply_draft_whose_post_failed() {
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = make_repo(dir.path());
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let server = server(&main, &head, Reply::status(204));
    let mut app = boot(&root, &server, None).await;
    // Thread bb22 has no reply route on the fake: a 404.
    claude_reply(&mut app, "7:bb22", "Will fail", true);
    app.test_execute(Command::ReviewSubmitComment);
    assert!(
        app.settle(|a| !a.review.writing && !a.review.drafts.errors.is_empty())
            .await
    );
    assert_eq!(app.review.draft_rows().len(), 1, "kept for another try");
    assert!(lines(&mut app, 140, 30).contains("failed:"));
    let sent: Vec<_> = server.requests().into_iter().filter(|r| r.method != "GET").collect();
    assert!(
        sent.iter()
            .all(|r| r.method == "POST" && r.target.ends_with("/discussions/bb22/notes")),
        "only the failed reply was tried, nothing resolved: {:?}",
        sent.iter().map(|r| &r.target).collect::<Vec<_>>()
    );
}
