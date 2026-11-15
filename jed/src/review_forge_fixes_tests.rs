//! Code review forge fixes, subtask a (suggestions mean what the forge means), driven through the
//! app against the fake GitLab server: a plain ` ```suggestion ` fence in a multi-line GitLab
//! comment replaces only the comment's last line when applied, `suggestion:-N+M` its range; a
//! reply or an edit from jed on GitLab is posted with the plain fence spelled out (`-N+0`), so
//! GitLab's page means what jed's box showed. Subtask c: the panel picks the same-repository merge
//! request, not a fork's on the same branch name.

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

/// Subtask c: GitLab's fake lists a same-project merge request (7) and a fork's (8, higher) on the
/// branch `feature`; the panel shows 7.
mod forks {
    use crate::forge::fake::Reply;
    use crate::git::test_repo::git;
    use crate::mr_review_tests::gitlab::boot;
    use crate::mr_review_tests::gitlab::server;
    use crate::mr_review_tests::make_repo;

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_c_the_panel_picks_the_same_repository_merge_request() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let srv = server(&main, &head, Reply::status(204));
        let app = boot(&root, &srv, None).await;
        let r = app.review.review().unwrap();
        assert_eq!(r.mr.number, 7);
        assert_eq!(r.mr.head_repo.as_deref(), Some("alice/project"));
    }
}

/// Subtask d: Insert Suggestion in a thread tab's reply box reads the anchored lines through the
/// tab's base (and refuses without code); on GitHub a draft on a line outside the diff's hunks is
/// held back (named in PENDING, the composer says so), the others post; on Gitea it posts.
mod placement {
    use crate::app::App;
    use crate::command::Command;
    use crate::diff_caret::Caret;
    use crate::forge::fake::FakeServer;
    use crate::git::test_repo::git;
    use crate::mr_review_tests::github::{boot_github, server as github_server};
    use crate::mr_review_tests::suggest::{boot, server as gitea_server};
    use crate::mr_review_tests::{draw_wide, make_repo, open_file, threads};
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    fn screen(app: &mut App) -> String {
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        term.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..40)
            .map(|y| (0..140).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn select(app: &mut App, line: usize) {
        app.group_mut().active_diff_mut().unwrap().go_to_side_line(true, line);
        draw_wide(app, 120);
        app.group_mut().active_diff_mut().unwrap().caret = Caret {
            right: true,
            anchor: (line - 1, 0),
            head: (line - 1, 1),
            col: None,
        };
    }

    fn message(app: &App) -> String {
        app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    /// A reply box in the thread tab of bob's thread on `renamed.txt` line 7 (`seven`, so the
    /// snippet starts at line 4: the tab's base is 3).
    pub(super) async fn thread_reply(code: bool) -> (App, FakeServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        // Without code: the anchor commit is not in the clone and there is no hunk.
        let commit = if code {
            head.as_str()
        } else {
            "0123456789012345678901234567890123456789"
        };
        let comments = format!(
            "[{}]",
            threads::rc(31, "bob", "Why", "renamed.txt", 7, commit, "+seven", false)
        );
        let server = gitea_server(&main, &head, &comments);
        let mut app = boot(&root, &server).await;
        let len = app.review.review().unwrap().conv_len();
        app.review.view.conv.focused = true;
        app.review.view.conv.select(len, 0);
        app.ui.focus = crate::app::Focus::SidePanel;
        app.test_execute(Command::ReviewOpenThread);
        assert!(app.settle(|a| a.diffs_settled()).await);
        assert!(app.group().active_diff().unwrap().thread.is_some());
        app.review.view.conv.focused = false;
        app.test_execute(Command::ReviewReply);
        assert!(app.test_composer_text().is_some(), "{}", message(&app));
        (app, server, dir)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_d_a_thread_tabs_reply_box_inserts_the_anchored_lines() {
        let (mut app, _server, _dir) = thread_reply(true).await;
        app.test_execute(Command::ReviewInsertSuggestion);
        assert_eq!(
            app.test_composer_text().as_deref(),
            Some("```suggestion\nseven\n```"),
            "{}",
            message(&app)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_d_a_thread_without_code_refuses_to_insert() {
        let (mut app, _server, _dir) = thread_reply(false).await;
        app.test_execute(Command::ReviewInsertSuggestion);
        assert_eq!(app.test_composer_text().as_deref(), Some(""));
        assert!(message(&app).contains("code isn't available"), "{}", message(&app));
    }

    fn posted(server: &FakeServer) -> Option<serde_json::Value> {
        server
            .requests()
            .into_iter()
            .find(|r| r.method == "POST" && r.target.ends_with("/pulls/7/reviews"))
            .map(|r| serde_json::from_str(&r.body).unwrap())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_d_github_holds_back_a_draft_outside_the_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = github_server(&main, &head);
        let mut app = boot_github(&root, &server).await;
        open_file(&mut app, "renamed.txt").await;
        draw_wide(&mut app, 120);
        select(&mut app, 1);
        app.test_execute(Command::ReviewComment);
        let text = screen(&mut app);
        assert!(
            text.contains("not in the merge request's diff"),
            "the box says so: {text}"
        );
        app.type_text("outside");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        select(&mut app, 6);
        app.test_execute(Command::ReviewComment);
        let text = screen(&mut app);
        assert!(!text.contains("not in the merge request's diff"), "{text}");
        app.type_text("inside");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        let rows = app.review.draft_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].held, Some("not in the merge request's diff"));
        assert_eq!(rows[1].held, None);
        app.test_execute(Command::ReviewSubmitComment);
        assert!(
            app.settle(|a| a.review.draft_rows().len() == 1 && !a.review.writing)
                .await
        );
        let v = posted(&server).expect("the review was posted");
        assert_eq!(v["comments"].as_array().unwrap().len(), 1);
        assert_eq!(
            (v["comments"][0]["body"].as_str(), v["comments"][0]["line"].as_u64()),
            (Some("inside"), Some(6))
        );
        let rows = app.review.draft_rows();
        assert_eq!(rows[0].draft.body, "outside");
        assert_eq!(rows[0].held, Some("not in the merge request's diff"));
        let text = screen(&mut app);
        assert!(text.contains("held back"), "PENDING names it: {text}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_d_a_drifted_draft_outside_the_diff_names_the_drift() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = github_server(&main, &head);
        let mut app = boot_github(&root, &server).await;
        open_file(&mut app, "renamed.txt").await;
        draw_wide(&mut app, 120);
        select(&mut app, 1);
        app.test_execute(Command::ReviewComment);
        app.type_text("outside");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        let rows = app.review.draft_rows();
        assert_eq!(rows[0].held, Some("not in the merge request's diff"));
        let id = rows[0].draft.id.clone();
        app.review.drafts.live.insert(id, Some("line changed".into()));
        assert_eq!(app.review.draft_rows()[0].held, Some("line changed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_d_gitea_posts_a_draft_outside_the_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = gitea_server(&main, &head, "[]");
        let mut app = boot(&root, &server).await;
        open_file(&mut app, "renamed.txt").await;
        draw_wide(&mut app, 120);
        select(&mut app, 1);
        app.test_execute(Command::ReviewComment);
        let text = screen(&mut app);
        assert!(!text.contains("not in the merge request's diff"), "{text}");
        app.type_text("outside");
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(app.review.draft_rows()[0].held, None);
        app.test_execute(Command::ReviewSubmitComment);
        assert!(
            app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
                .await
        );
        let v = posted(&server).expect("the review was posted");
        assert_eq!(v["comments"][0]["body"], "outside");
    }
}

/// Subtask e: the 60 s refresh also runs while a review diff or thread tab is active; GitHub's
/// cut-off thread comments are said in the Review tool log; a tokenless lookup stops before any
/// request (`mr_review_tests` covers the empty state).
mod small_differences {
    use crate::app::App;
    use crate::git::test_repo::git;
    use crate::logs::Tool;
    use crate::mr_review_tests::github::{boot_github, server_with_long_thread};
    use crate::mr_review_tests::{make_repo, open_file};
    use std::time::Instant;

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_e_a_review_tab_keeps_the_refresh_going_with_the_panel_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = server_with_long_thread(&main, &head);
        let mut app: App = boot_github(&root, &server).await;
        open_file(&mut app, "mod.txt").await;
        assert!(app.group().active_diff().unwrap().pair.is_review());
        app.ui.side_panel = crate::layout::SidePanel::Explorer;
        app.review.next_poll = Some(Instant::now());
        assert!(app.test_review_deadline().is_some(), "a review tab is active");
        // Another tab in the only group: no poll with the panel hidden.
        app.test_execute(crate::command::Command::NewUntitledFile);
        assert!(app.group().active_diff().is_none());
        assert_eq!(app.test_review_deadline(), None, "no review tab, no panel: no poll");
        app.test_execute(crate::command::Command::PrevTab);
        assert!(app.test_review_deadline().is_some());
        assert!(app.test_next_timer_is_review().await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_e_only_a_review_diff_or_the_panel_counts_for_the_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = server_with_long_thread(&main, &head);
        let mut app: App = boot_github(&root, &server).await;
        app.ui.side_panel = crate::layout::SidePanel::Explorer;
        app.review.next_poll = Some(Instant::now());
        // A plain (non-review) diff tab is not a reason to poll.
        app.open_diff(root.join("mod.txt"), crate::diff_view::Pair::COMPARE);
        assert!(!app.group().active_diff().unwrap().pair.is_review());
        assert_eq!(app.test_review_deadline(), None);
        // The panel on screen is.
        app.ui.side_panel = crate::layout::SidePanel::Review;
        app.ui.side_visible = true;
        assert!(app.test_review_deadline().is_some());
        app.ui.side_visible = false;
        assert_eq!(app.test_review_deadline(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_e_a_thread_tab_keeps_the_refresh_going_with_the_panel_hidden() {
        let (mut app, _server, _dir) = super::placement::thread_reply(true).await;
        let d = app.group().active_diff().unwrap();
        assert!(
            d.thread.is_some() && !d.pair.is_review(),
            "a thread tab, not a review diff"
        );
        app.ui.side_panel = crate::layout::SidePanel::Explorer;
        app.review.next_poll = Some(Instant::now());
        assert!(app.test_review_deadline().is_some(), "a thread tab is active");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_e_a_review_tab_in_another_group_keeps_the_refresh_going() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = server_with_long_thread(&main, &head);
        let mut app: App = boot_github(&root, &server).await;
        app.test_execute(crate::command::Command::SplitRight);
        open_file(&mut app, "mod.txt").await;
        app.ui.side_panel = crate::layout::SidePanel::Explorer;
        app.review.next_poll = Some(Instant::now());
        // The review tab is in one group; focus the one without it.
        if app.group().active_diff().is_some() {
            app.test_execute(crate::command::Command::FocusPreviousGroup);
        }
        assert!(
            app.group().active_diff().is_none(),
            "the focused group has no review tab"
        );
        assert!(
            app.test_review_deadline().is_some(),
            "the other group shows a review tab"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_forge_fixes_e_github_says_in_the_tool_log_that_a_thread_was_cut() {
        let dir = tempfile::tempdir().unwrap();
        let (root, main) = make_repo(dir.path());
        let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
        let server = server_with_long_thread(&main, &head);
        let app = boot_github(&root, &server).await;
        let log = app.logs.texts(&Tool::Review);
        assert!(log.iter().any(|l| l.contains("more than 100 comments")), "{log:?}");
    }
}
