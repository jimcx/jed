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
