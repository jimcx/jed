//! End-to-end tests for review_links-a, Open on Forge and Copy Link: the commands open or copy
//! the link of the selected CONVERSATIONS row's thread or comment, else of the merge request;
//! the forge's URL is only used when it is `https://` (or `http://` when the host's entry allows
//! it) on the review's own host, and anything else (another host, plain http, no URL) says "The
//! forge sent no usable link" and neither opens nor copies. The pure check is unit-tested in
//! `forge/url.rs`.
//!
//! review_links-b, Withdraw My Review (`review_links_b_*`): on the Gitea fake, the command asks first,
//! then dismisses the user's review and refreshes; Cancel sends nothing; with no verdict of the
//! user's it says so; a refusal's message is shown; the panel's menu lists the item only then.

use crate::app::App;
use crate::command::Command;
use crate::forge::Comment;
use crate::mr_review_tests::threads::loaded_app;
use crate::review::Content;

const REFUSED: &str = "The forge sent no usable link";

fn review(app: &mut App) -> &mut crate::review::Review {
    let Content::Loaded(r) = &mut app.review.content else {
        panic!("a loaded review");
    };
    r
}

fn comment(id: &str, url: &str) -> Comment {
    Comment {
        id: id.into(),
        author: "bob".into(),
        body: "hello".into(),
        created_at: "2026-01-02T10:00:00Z".into(),
        updated_at: String::new(),
        url: url.into(),
    }
}

/// Focuses the CONVERSATIONS list with row `i` selected.
fn select_row(app: &mut App, i: usize) {
    let len = review(app).conv_len();
    app.ui.focus = crate::app::Focus::SidePanel;
    app.review.view.conv.focused = true;
    app.review.view.conv.select(len, i);
}

fn message(app: &App) -> String {
    app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

fn copied(app: &mut App) -> String {
    app.clipboard.read().unwrap_or_default()
}

async fn opens(app: &mut App, url: &str) {
    app.test_execute(Command::ReviewOpenOnForge);
    assert!(app.settle(|_| crate::url::opened(url)).await, "{url} opened");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_thread_opens_and_copies_its_first_comments_link() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    let url = "https://git.example.com/alice/project/pulls/7#issuecomment-thread";
    review(&mut app).threads[0].thread.comments[0].url = url.into();
    select_row(&mut app, 0);
    opens(&mut app, url).await;
    app.test_execute(Command::ReviewCopyLink);
    assert_eq!(copied(&mut app), url);
    assert_eq!(message(&app), "Link copied");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_conversation_comment_opens_and_copies_its_link() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    let url = "https://git.example.com/alice/project/pulls/7#issuecomment-conv";
    review(&mut app).conversation.push(comment("900", url));
    let row = review(&mut app).conv_len() - 1;
    select_row(&mut app, row);
    opens(&mut app, url).await;
    app.test_execute(Command::ReviewCopyLink);
    assert_eq!(copied(&mut app), url);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_merge_request_is_the_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    let url = "https://git.example.com/alice/project/pulls/7?fallback";
    review(&mut app).mr.url = url.into();
    app.review.view.conv.focused = false;
    opens(&mut app, url).await;
    app.test_execute(Command::ReviewCopyLink);
    assert_eq!(copied(&mut app), url);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_link_is_refused_unless_https_on_the_reviews_host() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    review(&mut app).allow_http = false;
    let bad = [
        "https://evil.example.org/refused-other-host",
        "http://git.example.com/refused-plain-http",
        "https://git.example.com@evil.example.org/refused-userinfo",
        "file:///etc/refused-passwd",
        "",
    ];
    app.review.view.conv.focused = false;
    for url in bad {
        review(&mut app).mr.url = url.into();
        app.message = None;
        app.test_execute(Command::ReviewOpenOnForge);
        assert_eq!(message(&app), REFUSED, "{url:?}");
        app.message = None;
        app.test_execute(Command::ReviewCopyLink);
        assert_eq!(message(&app), REFUSED, "{url:?}");
        assert_eq!(copied(&mut app), "", "{url:?} copied");
    }
    // Let a (wrongly) spawned open finish before checking nothing was opened.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    for url in bad.iter().filter(|u| !u.is_empty()) {
        assert!(!crate::url::opened(url), "{url} opened");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_host_entry_allowing_http_lets_a_plain_link_through() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    let url = "http://git.example.com/alice/project/pulls/7?allowed";
    review(&mut app).allow_http = true;
    review(&mut app).mr.url = url.into();
    app.review.view.conv.focused = false;
    opens(&mut app, url).await;
}

fn thread_url(id: &str) -> String {
    format!("https://git.example.com/alice/project/pulls/7#caret-thread-{id}")
}

/// Gives every thread's first comment its own link, and the merge request one.
fn link_everything(app: &mut App) -> String {
    let r = review(app);
    for row in &mut r.threads {
        let url = thread_url(&row.thread.id);
        row.thread.comments[0].url = url;
    }
    let mr = "https://git.example.com/alice/project/pulls/7#caret-mr".to_string();
    r.mr.url = mr.clone();
    mr
}

/// Runs Copy Link with the clipboard holding a sentinel, and returns what it holds after.
fn copy_link(app: &mut App) -> String {
    app.clipboard.write(crate::clipboard::Copied {
        text: "sentinel".into(),
        whole_line: false,
        pieces: Vec::new(),
    });
    app.test_execute(Command::ReviewCopyLink);
    copied(app)
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_the_thread_at_a_review_diffs_caret_is_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    let mr = link_everything(&mut app);
    crate::mr_review_tests::open_file(&mut app, "mod.txt").await;
    app.review.view.conv.focused = false;
    app.ui.focus = crate::app::Focus::Editor;
    crate::mr_review_tests::screen(&mut app);
    // No thread block under the caret: the merge request.
    app.group_mut().active_diff_mut().unwrap().cursor = 0;
    if app.group().active_diff().unwrap().thread_at_caret().is_none() {
        assert_eq!(copy_link(&mut app), mr);
    }
    // On a thread's block: that thread's link, whichever of the threads it is.
    let mut seen = 0;
    for _ in 0..3 {
        app.test_execute(Command::ReviewNextThread);
        let d = app.group().active_diff().unwrap();
        let Some(i) = d.thread_at_caret() else { continue };
        let id = d.threads.list()[i].id.clone();
        assert_eq!(copy_link(&mut app), thread_url(&id));
        seen += 1;
    }
    assert!(seen > 0, "the diff has thread blocks");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_a_thread_tabs_own_thread_is_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    link_everything(&mut app);
    let len = review(&mut app).conv_len();
    app.review.view.conv.focused = true;
    app.review.view.conv.select(len, 0);
    app.ui.focus = crate::app::Focus::SidePanel;
    app.press(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let id = app
        .group()
        .active_diff()
        .and_then(|d| d.thread.as_ref())
        .map(|t| t.id.clone())
        .expect("row 0 opened a thread tab");
    app.review.view.conv.focused = false;
    app.ui.focus = crate::app::Focus::Editor;
    assert_eq!(copy_link(&mut app), thread_url(&id));
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_the_conversation_tabs_caret_row_is_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    let mr = link_everything(&mut app);
    let first = "https://git.example.com/alice/project/pulls/7#c-first";
    let second = "https://git.example.com/alice/project/pulls/7#c-second";
    let r = review(&mut app);
    r.conversation.clear();
    r.conversation.push(comment("901", first));
    r.conversation.push(comment("902", second));
    app.test_execute(Command::ReviewOpenConversation);
    assert!(app.settle(|a| a.diffs_settled()).await);
    app.review.view.conv.focused = false;
    app.ui.focus = crate::app::Focus::Editor;
    let cursor = |app: &mut App, row: usize| app.group_mut().active_diff_mut().unwrap().cursor = row;
    // Row 0 is the description: the merge request; row k is conversation[k - 1].
    cursor(&mut app, 0);
    assert_eq!(copy_link(&mut app), mr);
    cursor(&mut app, 1);
    assert_eq!(copy_link(&mut app), first);
    cursor(&mut app, 2);
    assert_eq!(copy_link(&mut app), second);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_links_a_the_selected_row_beats_the_caret() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    link_everything(&mut app);
    let sel = "https://git.example.com/alice/project/pulls/7#c-sel";
    review(&mut app).conversation.clear();
    review(&mut app).conversation.push(comment("903", sel));
    app.test_execute(Command::ReviewOpenConversation);
    assert!(app.settle(|a| a.diffs_settled()).await);
    app.group_mut().active_diff_mut().unwrap().cursor = 0;
    let row = review(&mut app).conv_len() - 1;
    select_row(&mut app, row);
    assert_eq!(copy_link(&mut app), sel);
}

mod withdraw {
    use super::*;
    use crate::app::Prompt;
    use crate::forge::fake::Reply;
    use crate::mr_review_tests::threads::{loaded_app_reviews, loaded_app_with};
    use crossterm::event::{KeyCode, KeyModifiers};

    const REVIEWS: &str = r#"[{"id":1,"user":{"login":"bob"},"body":"","state":"COMMENT","comments_count":9,
        "submitted_at":"2026-01-02T10:00:00Z","html_url":"u"},
        {"id":5,"user":{"login":"Sam"},"body":"","state":"APPROVED",
        "submitted_at":"2026-01-03T10:00:00Z","html_url":"u"}]"#;
    const DISMISS: &str = "/api/v1/repos/alice/project/pulls/7/reviews/5/dismissals";

    fn dismissals(server: &crate::forge::fake::FakeServer) -> Vec<crate::forge::fake::Request> {
        server.requests().into_iter().filter(|r| r.target == DISMISS).collect()
    }

    fn reads(server: &crate::forge::fake::FakeServer) -> usize {
        server.requests().iter().filter(|r| r.method == "GET").count()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_links_b_withdraw_asks_then_dismisses_the_review_and_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let extra = vec![(format!("POST {DISMISS}"), Reply::json("{}"))];
        let (mut app, server) = loaded_app_with(dir.path(), None, Some(REVIEWS), None, extra).await;
        app.review.user = Some("sam".into());
        app.test_execute(Command::ReviewWithdraw);
        assert!(matches!(
            app.prompt,
            Some(Prompt::ConfirmWithdraw { approved: true, .. })
        ));
        // Declined: nothing is sent.
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.prompt.is_none());
        assert!(dismissals(&server).is_empty());
        let before = reads(&server);
        app.test_execute(Command::ReviewWithdraw);
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|_| !dismissals(&server).is_empty()).await);
        let sent = &dismissals(&server)[0];
        assert_eq!(sent.method, "POST");
        let body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(body["message"], crate::forge::WITHDRAW_MESSAGE);
        assert!(app.settle(|a| !a.review.writing && !a.review.loading).await);
        assert!(reads(&server) > before, "the merge request is read again");
        assert_eq!(message(&app), "Review withdrawn");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_links_b_a_refusal_shows_the_forges_message() {
        let dir = tempfile::tempdir().unwrap();
        let extra = vec![(format!("POST {DISMISS}"), Reply::status(403))];
        let (mut app, server) = loaded_app_with(dir.path(), None, Some(REVIEWS), None, extra).await;
        app.review.user = Some("sam".into());
        app.test_execute(Command::ReviewWithdraw);
        app.press(KeyCode::Char('w'), KeyModifiers::NONE);
        assert!(app.settle(|a| a.review.error.is_some()).await);
        assert_eq!(dismissals(&server).len(), 1);
        let err = app.review.error.clone().unwrap();
        assert!(err.contains("refused") && err.contains("403"), "{err}");
    }

    /// GitLab: the user's approval is unapproved after the prompt; their change request is refused
    /// before any prompt, and the panel menu does not offer it.
    #[tokio::test(flavor = "multi_thread")]
    async fn review_links_b_gitlab_unapproves_and_refuses_a_change_request_before_asking() {
        use crate::mr_review_tests::gitlab::{API, boot, server_with_routes};
        use crate::mr_review_tests::write::repo;
        let unapprove = format!("{API}/merge_requests/7/unapprove");
        let posts = |server: &crate::forge::fake::FakeServer| {
            server.requests().into_iter().filter(|r| r.target == unapprove).count()
        };
        let routes = || {
            vec![
                (
                    format!("{API}/merge_requests/7/approvals"),
                    Reply::json(r#"{"approved_by":[{"user":{"username":"sam"}}]}"#),
                ),
                (
                    format!("{API}/merge_requests/7/reviewers"),
                    Reply::json(
                        r#"[{"user":{"username":"bob"},"state":"requested_changes","created_at":"2026-01-01T00:00:00Z"}]"#,
                    ),
                ),
                (format!("POST {unapprove}"), Reply::json("{}")),
            ]
        };
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server_with_routes(&main, &head, Reply::json("{}"), "bob", "remark", routes());
        let mut app = boot(&root, &server, None).await;
        app.review.user = Some("bob".into());
        app.test_execute(Command::ReviewWithdraw);
        assert!(app.prompt.is_none(), "refused before asking");
        assert!(
            message(&app).contains("GitLab cannot withdraw a change request"),
            "{}",
            message(&app)
        );
        assert_eq!(posts(&server), 0);
        app.review.user = Some("sam".into());
        app.test_execute(Command::ReviewWithdraw);
        assert!(matches!(
            app.prompt,
            Some(Prompt::ConfirmWithdraw { approved: true, .. })
        ));
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(posts(&server), 0, "Cancel sends nothing");
        app.test_execute(Command::ReviewWithdraw);
        app.press(KeyCode::Char('w'), KeyModifiers::NONE);
        assert!(app.settle(|_| posts(&server) == 1).await);
        assert!(app.settle(|a| !a.review.writing).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_links_b_a_withdrawn_verdict_leaves_no_menu_item() {
        let dir = tempfile::tempdir().unwrap();
        let extra = vec![(format!("POST {DISMISS}"), Reply::json("{}"))];
        let (mut app, _server) = loaded_app_with(dir.path(), None, Some(REVIEWS), None, extra).await;
        app.review.user = Some("sam".into());
        app.test_execute(Command::ReviewWithdraw);
        assert!(app.prompt.is_some());
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        // The lookup after the withdraw no longer lists the approval.
        let mut review = app.review.review().unwrap().clone();
        review.approvals.retain(|a| a.author != "Sam");
        app.review.content = crate::review::Content::Loaded(Box::new(review));
        app.test_execute(Command::ReviewWithdraw);
        assert_eq!(message(&app), "You have no approval or change request to withdraw");
        assert!(app.prompt.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_links_b_without_a_verdict_of_the_users_nothing_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, server) = loaded_app_reviews(dir.path(), None, Some(REVIEWS)).await;
        app.test_execute(Command::ReviewWithdraw);
        assert_eq!(message(&app), "The forge hasn't said who you are yet");
        app.review.user = Some("bob".into());
        app.test_execute(Command::ReviewWithdraw);
        assert_eq!(message(&app), "You have no approval or change request to withdraw");
        assert!(app.prompt.is_none());
        assert!(dismissals(&server).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_links_b_the_panel_menu_lists_it_only_for_the_users_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _server) = loaded_app_reviews(dir.path(), None, Some(REVIEWS)).await;
        let hit = app.hits.review.clone().unwrap();
        let right_click = |app: &mut App| {
            app.context_menu = None;
            app.mouse(
                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Right),
                hit.summary.x + 2,
                hit.summary.y,
            );
            let menu = app.context_menu.as_ref().expect("a menu is open");
            menu.items.iter().any(|i| {
                matches!(
                    i,
                    crate::context_menu::MenuItem::Item { label, command: Command::ReviewWithdraw, .. }
                        if label == "Withdraw My Review"
                )
            })
        };
        app.review.user = Some("bob".into());
        assert!(!right_click(&mut app));
        app.review.user = Some("sam".into());
        assert!(right_click(&mut app));
    }
}
