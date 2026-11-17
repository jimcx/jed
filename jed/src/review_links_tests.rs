//! End-to-end tests for review_links-a, Open on Forge and Copy Link: the commands open or copy
//! the link of the selected CONVERSATIONS row's thread or comment, else of the merge request;
//! the forge's URL is only used when it is `https://` (or `http://` when the host's entry allows
//! it) on the review's own host, and anything else (another host, plain http, no URL) says "The
//! forge sent no usable link" and neither opens nor copies. The pure check is unit-tested in
//! `forge/url.rs`.

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
