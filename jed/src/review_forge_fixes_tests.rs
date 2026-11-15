//! Code review forge fixes, subtask a (suggestions mean what the forge means), driven through the
//! app against the fake GitLab server: a plain ` ```suggestion ` fence in a multi-line GitLab
//! comment replaces only the comment's last line when applied, `suggestion:-N+M` its range; a
//! reply or an edit from jed on GitLab is posted with the plain fence spelled out (`-N+0`), so
//! GitLab's page means what jed's box showed.

mod gitlab_suggestions {
    use crate::app::App;
    use crate::command::Command;
    use crate::diff_caret::Caret;
    use crate::forge::fake::{FakeServer, Reply};
    use crate::git::test_repo::git;
    use crate::mr_review_tests::gitlab::{boot, server_with_note};
    use crate::mr_review_tests::make_repo;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// mod.txt is `m1 / m2 changed / m3`; the discussion `aa11` is on lines 2-3.
    async fn world(dir: &Path, author: &str, body: &str) -> (PathBuf, FakeServer, App) {
        let (root, main) = make_repo(dir);
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = server_with_note(&main, &head, Reply::status(204), author, body);
        let app = boot(&root, &server, None).await;
        (root, server, app)
    }

    /// The caret over the thread (lines 2-3 of the new file).
    fn on_thread(app: &mut App) {
        app.group_mut().active_diff_mut().unwrap().go_to_side_line(true, 2);
        crate::mr_review_tests::draw_wide(app, 120);
        app.group_mut().active_diff_mut().unwrap().caret = Caret {
            right: true,
            anchor: (2, 0),
            head: (2, 1),
            col: None,
        };
    }

    fn text(app: &App, root: &Path) -> String {
        let id = app.buffers.find_path(&root.join("mod.txt")).expect("mod.txt is open");
        app.buffers.get(id).unwrap().rope.to_string()
    }

    async fn apply(app: &mut App) {
        on_thread(app);
        app.test_execute(Command::ReviewApplySuggestion);
        for _ in 0..40 {
            app.test_turn(Duration::from_millis(25)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_a_gitlab_plain_fence_applies_to_the_last_line_only() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _server, mut app) = world(dir.path(), "bob", "Try\\n```suggestion\\nX\\n```").await;
        apply(&mut app).await;
        assert_eq!(text(&app, &root), "m1\nm2 changed\nX\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_a_gitlab_range_fence_applies_to_its_range() {
        let dir = tempfile::tempdir().unwrap();
        let (root, _server, mut app) = world(dir.path(), "bob", "Try\\n```suggestion:-1+0\\nX\\nY\\n```").await;
        apply(&mut app).await;
        assert_eq!(text(&app, &root), "m1\nX\nY\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_a_gitlab_reply_and_an_edit_post_the_plain_fence_spelled_out() {
        let dir = tempfile::tempdir().unwrap();
        let (_root, server, mut app) = world(dir.path(), "sam", "mine").await;
        // A reply with a plain fence on a two-line thread: the whole range (`-1+0`).
        on_thread(&mut app);
        app.test_execute(Command::ReviewReply);
        app.type_text("```suggestion\nX\n```");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(
            app.settle(|a| a.test_composer_text().is_none() && !a.review.writing)
                .await
        );
        let posted = server
            .requests()
            .into_iter()
            .find(|r| r.method == "POST" && r.target.ends_with("/discussions/aa11/notes"))
            .expect("the reply was posted");
        assert_eq!(posted.body, r#"{"body":"```suggestion:-1+0\nX\n```"}"#);
        // Editing the thread's first comment: the same.
        crate::mr_review_tests::idle(&mut app).await;
        on_thread(&mut app);
        app.test_execute(Command::ReviewEditComment);
        assert_eq!(app.test_composer_text().as_deref(), Some("mine"));
        app.type_text("\n```suggestion\nY\n```");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(
            app.settle(|a| a.test_composer_text().is_none() && !a.review.writing)
                .await
        );
        let put = server
            .requests()
            .into_iter()
            .find(|r| r.method == "PUT" && r.target.ends_with("/discussions/aa11/notes/101"))
            .expect("the edit was sent");
        assert_eq!(put.body, r#"{"body":"mine\n```suggestion:-1+0\nY\n```"}"#);
    }
}

/// Subtask b: verdicts. A dismissed review clears its author's verdict on Gitea (and GitHub); a
/// GitLab reviewer who requested changes shows in the panel, and a lookup fetches the merge
/// request once; a Request Changes with no drafts carries a body on Gitea, whose fake refuses an
/// empty one as the real server does.
mod verdicts {
    use crate::command::Command;
    use crate::forge::fake::Reply;
    use crate::mr_review_tests::gitlab::{API, boot as boot_gitlab, server_with_routes};
    use crate::mr_review_tests::threads::loaded_app_reviews;
    use crate::mr_review_tests::write::{REVIEWS, boot, repo, server};
    use crate::mr_review_tests::{idle, screen};

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_b_a_dismissed_review_leaves_no_verdict_in_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = r#"[
            {"id":1,"user":{"login":"bob"},"body":"","state":"REQUEST_CHANGES","submitted_at":"2026-01-02T10:00:00Z","html_url":"u"},
            {"id":2,"user":{"login":"bob"},"body":"","state":"APPROVED","dismissed":true,"submitted_at":"2026-01-02T11:00:00Z","html_url":"u"},
            {"id":3,"user":{"login":"sam"},"body":"","state":"APPROVED","stale":true,"submitted_at":"2026-01-02T12:00:00Z","html_url":"u"},
            {"id":4,"user":{"login":"zed"},"body":"","state":"REQUEST_CHANGES","dismissed":true,"submitted_at":"2026-01-02T13:00:00Z","html_url":"u"}]"#;
        let (mut app, _server) = loaded_app_reviews(dir.path(), None, Some(reviews)).await;
        idle(&mut app).await;
        let text = screen(&mut app);
        assert!(text.contains("✓ approved by sam"), "{text}");
        assert!(!text.contains("bob") && !text.contains("zed"), "{text}");
        assert!(!text.contains("changes requested"), "{text}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_b_gitea_request_changes_without_drafts_sends_a_body() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        app.test_execute(Command::ReviewSubmitRequestChanges);
        assert!(app.settle(|a| !a.review.writing).await);
        let post = server
            .requests()
            .into_iter()
            .find(|r| r.method == "POST" && r.target == REVIEWS)
            .expect("the review was posted");
        let sent: serde_json::Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(sent["event"], "REQUEST_CHANGES");
        assert_eq!(sent["body"], "Changes requested");
        let msg = app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
        assert!(!msg.contains("requires a body") && !msg.contains("422"), "{msg}");
    }

    /// The Gitea fake models the real rule: an empty-bodied REQUEST_CHANGES is a 422 (so the test
    /// above proves the adapter's body, not a fake that accepts anything), the same with a body or
    /// another event is accepted.
    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_b_the_gitea_fake_refuses_an_empty_change_request() {
        use std::io::{Read, Write};
        let dir = tempfile::tempdir().unwrap();
        let (_root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let addr = server.url().trim_start_matches("http://").to_string();
        let post = |body: &str| -> u16 {
            let mut s = std::net::TcpStream::connect(&addr).unwrap();
            write!(
                s,
                "POST {REVIEWS} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut text = String::new();
            s.read_to_string(&mut text).unwrap();
            text.split_whitespace().nth(1).unwrap().parse().unwrap()
        };
        // The key order serde_json writes the adapter's body in.
        assert_eq!(
            post(r#"{"body":"","comments":[],"commit_id":"h","event":"REQUEST_CHANGES"}"#),
            422
        );
        assert_eq!(
            post(r#"{"body":"","comments":[{"path":"a"}],"commit_id":"h","event":"REQUEST_CHANGES"}"#),
            422,
            "line comments are no body"
        );
        assert_eq!(
            post(r#"{"body":"x","comments":[],"commit_id":"h","event":"REQUEST_CHANGES"}"#),
            200
        );
        assert_eq!(
            post(r#"{"body":"","comments":[],"commit_id":"h","event":"APPROVED"}"#),
            200
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_b_gitlab_shows_changes_requested_and_fetches_the_merge_request_once() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let extra = vec![
            (
                format!("{API}/merge_requests/7/approvals"),
                Reply::json(r#"{"approved_by":[{"user":{"username":"sam"}}]}"#),
            ),
            (
                format!("{API}/merge_requests/7/reviewers"),
                Reply::json(
                    r#"[{"user":{"username":"bob"},"state":"requested_changes","created_at":"2026-01-01T00:00:00Z"},
                        {"user":{"username":"ann"},"state":"unreviewed","created_at":"2026-01-01T00:00:00Z"}]"#,
                ),
            ),
        ];
        let server = server_with_routes(&main, &head, Reply::json("{}"), "bob", "remark", extra);
        let mut app = boot_gitlab(&root, &server, None).await;
        let text = screen(&mut app);
        assert!(text.contains("✓ approved by sam"), "{text}");
        assert!(text.contains("✗ changes requested by bob"), "{text}");
        assert!(!text.contains("by sam, ann") && !text.contains("by ann"), "{text}");
        let gets = |path: &str| {
            let all = server.requests();
            all.iter().filter(|r| r.method == "GET" && r.target == path).count()
        };
        let lookups = gets(&format!(
            "{API}/merge_requests?state=opened&source_branch=feature&per_page=100&page=1"
        ));
        assert!(lookups >= 1);
        assert_eq!(
            gets(&format!("{API}/merge_requests/7")),
            lookups,
            "one MR request per lookup"
        );
    }
}
