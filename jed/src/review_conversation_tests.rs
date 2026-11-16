//! End-to-end tests for review_conversation-a, the conversation tab (read-only): a click or Enter
//! on the panel's summary and Enter on a merge request comment's CONVERSATIONS row open the tab
//! `!7 Conversation` (one per merge request, reused) with the description first, then every
//! comment in full and wrapped to the width; the row's Enter reveals its comment; a lookup's new
//! comments appear and another merge request retitles the tab; copy takes the caret's block; the
//! switcher mark alone switches merge request. The Claude tool's `body` is tested in
//! `mr_review_tests/claude.rs` (`review_conversation_claude_*`).

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::command::Command;
use crate::forge::Comment;
use crate::mr_review_tests::threads::loaded_app;
use crate::review::Content;

fn draw(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect()
}

fn comment(id: &str, author: &str, body: &str) -> Comment {
    Comment {
        id: id.into(),
        author: author.into(),
        body: body.into(),
        created_at: "2026-01-02T10:00:00Z".into(),
        updated_at: String::new(),
        url: String::new(),
    }
}

fn review(app: &mut App) -> &mut crate::review::Review {
    let Content::Loaded(r) = &mut app.review.content else {
        panic!("a loaded review");
    };
    r
}

/// As if a lookup finished with new data.
fn changed(app: &mut App) {
    app.review.generation += 1;
}

async fn open(app: &mut App) {
    app.test_execute(Command::ReviewOpenConversation);
    assert!(app.settle(|a| a.diffs_settled()).await);
}

fn tab(app: &App) -> &crate::diff_view::DiffTab {
    app.group()
        .active_diff()
        .filter(|d| d.conv.is_some())
        .expect("the conversation tab")
}

fn tabs(app: &App) -> usize {
    app.group()
        .tabs
        .iter()
        .filter(|t| t.diff().is_some_and(|d| d.conv.is_some()))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_the_tab_shows_the_description_and_comments_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau end";
    review(&mut app).mr.body = format!("First line of the description\n\n{long}");
    review(&mut app)
        .conversation
        .push(comment("100", "bob", &format!("Second\n{long} bobend")));
    changed(&mut app);
    open(&mut app).await;
    let l = draw(&mut app, 100, 40).join("\n");
    assert_eq!(tab(&app).title(), "!7 Conversation");
    assert!(l.contains("!7 Conversation"), "{l}");
    // The description first, headed by the author and the title, then the comments in order.
    let (desc, ann, bob) = (
        l.find("ann · Add review panel")
            .unwrap_or_else(|| panic!("description header in\n{l}")),
        l.find("Thanks for").expect("ann's comment"),
        l.find("bobend").expect("bob's comment's end"),
    );
    assert!(desc < ann && ann < bob, "{l}");
    assert!(
        l.contains("First line of the description") && l.contains("looking"),
        "{l}"
    );
    assert!(l.contains("tau end"), "the description's last words: {l}");
    assert!(!l.contains("Open File"), "{l}");
    // Reused: one tab per merge request.
    open(&mut app).await;
    assert_eq!(tabs(&app), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_a_comment_rows_enter_reveals_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    for i in 0..12 {
        let body = format!("Comment number {i}\nwith a second line\nand a third");
        review(&mut app)
            .conversation
            .push(comment(&format!("{}", 200 + i), "cy", &body));
    }
    changed(&mut app);
    draw(&mut app, 70, 30);
    let (threads, len) = {
        let r = review(&mut app);
        (r.threads.len(), r.conv_len())
    };
    // The last comment's row.
    app.ui.focus = crate::app::Focus::SidePanel;
    app.review.view.conv.focused = true;
    app.review.view.conv.select(len, len - 1);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let l = draw(&mut app, 100, 24).join("\n");
    assert!(l.contains("Comment number 11") && l.contains("and a third"), "{l}");
    assert!(!l.contains("Comment number 0\n"), "scrolled past the first: {l}");
    // The caret is on that comment's row (the description's is row 0).
    assert_eq!(tab(&app).cursor, len - threads);
    // Another comment: in view.
    app.ui.focus = crate::app::Focus::SidePanel;
    app.review.view.conv.focused = true;
    app.review.view.conv.select(len, threads + 3);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    let l = draw(&mut app, 100, 24).join("\n");
    assert!(l.contains("Comment number 3"), "{l}");
    assert_eq!(tabs(&app), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_it_follows_the_review() {
    use crate::forge::fake::Reply;
    let dir = tempfile::tempdir().unwrap();
    // The issue comments route answers in turn: the standard one (the app's first two lookups,
    // opening the panel and showing it), then with a second comment, so a real refresh brings it.
    let path = "/api/v1/repos/alice/project/issues/7/comments?page=1&limit=50".to_string();
    let ann = r#"[{"id":99,"user":{"login":"ann"},"body":"Thanks for\nlooking","created_at":"2026-01-01T10:00:00Z",
            "updated_at":"","html_url":"u99"}]"#;
    let extra = vec![
        (path.clone(), Reply::json(ann)),
        (
            path,
            Reply::json(
                r#"[{"id":99,"user":{"login":"ann"},"body":"Thanks for\nlooking","created_at":"2026-01-01T10:00:00Z",
            "updated_at":"","html_url":"u99"},
            {"id":98,"user":{"login":"dee"},"body":"A new remark","created_at":"2026-01-03T10:00:00Z",
            "updated_at":"","html_url":"u98"}]"#,
            ),
        ),
    ];
    let (mut app, _server) =
        crate::mr_review_tests::threads::loaded_app_with(dir.path(), None, None, None, extra).await;
    open(&mut app).await;
    assert_eq!(tab(&app).conv.as_ref().unwrap().rows, 2);
    assert!(!draw(&mut app, 80, 30).join("\n").contains("A new remark"));
    app.test_execute(Command::ReviewRefresh);
    assert!(
        app.settle(|a| !a.review.loading
            && a.group()
                .active_diff()
                .is_some_and(|d| d.conv.as_ref().is_some_and(|c| c.rows == 3) && a.diffs_settled()))
            .await
    );
    let l = draw(&mut app, 80, 30).join("\n");
    assert!(l.contains("A new remark"), "{l}");
    assert_eq!(tabs(&app), 1);
    // Another merge request retitles it (the lookup's result is poked: the fake forge serves one).
    review(&mut app).mr.number = 9;
    changed(&mut app);
    assert!(app.settle(|a| a.diffs_settled()).await);
    draw(&mut app, 80, 30);
    assert_eq!(tab(&app).title(), "!9 Conversation");
    assert_eq!(tabs(&app), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_the_summary_opens_it_and_a_row_of_the_panel_too() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    draw(&mut app, 70, 30);
    let hit = app.hits.review.clone().unwrap();
    // One merge request: no switcher mark; a click on the summary opens the conversation.
    assert_eq!(hit.switcher.width, 0);
    app.mouse(
        MouseEventKind::Down(MouseButton::Left),
        hit.summary.x + 2,
        hit.summary.y,
    );
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(tab(&app).title(), "!7 Conversation");
    // Enter on the summary row (Up from the FILES header selects it).
    app.group_mut().active = 0;
    app.review.view.files_header = true;
    app.ui.focus = crate::app::Focus::SidePanel;
    app.press(KeyCode::Up, KeyModifiers::NONE);
    assert!(app.review.view.summary);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(tab(&app).title(), "!7 Conversation");
    assert_eq!(tabs(&app), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_copy_takes_the_carets_block() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    open(&mut app).await;
    draw(&mut app, 80, 30);
    app.press(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(tab(&app).cursor, 1);
    app.test_execute(Command::Copy);
    let copied = app.clipboard.read().unwrap_or_default();
    assert!(copied.starts_with("ann:\nThanks for"), "{copied:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_stepping_down_brings_the_whole_block_into_view() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    for i in 0..6 {
        let body = format!("head {i}\nl1\nl2\nl3\nl4\ntail {i}");
        review(&mut app)
            .conversation
            .push(comment(&format!("{}", 400 + i), "cy", &body));
    }
    changed(&mut app);
    open(&mut app).await;
    for step in 0..7 {
        let l = draw(&mut app, 100, 18).join("\n");
        let cursor = tab(&app).cursor;
        assert_eq!(cursor, step);
        if cursor >= 2 {
            // The caret's block (row 1 is ann's comment) in full, its last line too.
            let i = cursor - 2;
            assert!(
                l.contains(&format!("head {i}")) && l.contains(&format!("tail {i}")),
                "{l}"
            );
        }
        app.press(KeyCode::Down, KeyModifiers::NONE);
    }
}

// review_conversation-b: times on comments, drawn with a fixed clock (`app.review.clock`).

/// 2026-03-12T10:20:30Z, the clock of these tests.
const NOW: i64 = 1_773_310_830;

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_thread_blocks_show_when_each_comment_was_written() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    crate::mr_review_tests::open_file(&mut app, "mod.txt").await;
    app.review.clock = Some(NOW);
    {
        let r = review(&mut app);
        let sam = r.threads.iter_mut().find(|t| t.thread.id == "11").unwrap();
        sam.thread.comments[0].created_at = "2026-03-12T08:20:30Z".into();
        sam.thread.comments[0].updated_at = "2026-03-12T08:25:30.500Z".into();
        let mut reply = comment("500", "dee", "A reply");
        reply.created_at = "2026-03-12T10:15:00+00:00".into();
        sam.thread.comments.push(reply);
        // Unparseable: no time, as before.
        let bob = r.threads.iter_mut().find(|t| t.thread.id == "12").unwrap();
        bob.thread.comments[0].created_at = "soon".into();
    }
    changed(&mut app);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let l = draw(&mut app, 120, 30);
    let row = |what: &str| {
        l.iter()
            .find(|r| r.contains(what))
            .unwrap_or_else(|| panic!("{what} in {l:#?}"))
    };
    assert!(row("┌ sam").contains("┌ sam · 2h ago · edited ─"), "{l:#?}");
    assert!(row("├ dee").contains("├ dee · 5m ago ─"), "{l:#?}");
    assert!(
        !row("┌ bob").contains("ago") && !row("┌ bob").contains("soon"),
        "{l:#?}"
    );
    // Later: the same comment, a day and a bit on, then (weeks on) a date; no redraw timer.
    app.review.clock = Some(NOW + 26 * 3600);
    let l = draw(&mut app, 120, 30);
    assert!(l.iter().any(|r| r.contains("┌ sam · 1d ago · edited")), "{l:#?}");
    app.review.clock = Some(NOW + 40 * 86_400);
    let l = draw(&mut app, 120, 30);
    assert!(l.iter().any(|r| r.contains("┌ sam · 12 Mar · edited")), "{l:#?}");
    // A narrow tab cuts `edited` first, then the time, never the name.
    let l = draw(&mut app, 50, 30);
    assert!(l.iter().any(|r| r.contains("┌ sam") && !r.contains("edited")), "{l:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_the_tab_shows_each_comments_time() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    app.review.clock = Some(NOW);
    let mut fresh = comment("101", "bob", "Fresh");
    fresh.created_at = "2026-03-12T10:20:00Z".into();
    let mut old = comment("102", "cy", "Ancient");
    old.created_at = "2025-12-31T23:00:00Z".into();
    old.updated_at = "2026-01-02T00:00:00Z".into();
    let mut garbage = comment("103", "dee", "Timeless");
    garbage.created_at = String::new();
    review(&mut app).conversation = vec![fresh, old, garbage];
    changed(&mut app);
    open(&mut app).await;
    let l = draw(&mut app, 100, 40);
    let row = |what: &str| {
        l.iter()
            .find(|r| r.contains(what))
            .unwrap_or_else(|| panic!("{what} in {l:#?}"))
    };
    // The description has no time of its own.
    assert!(!row("ann · Add review panel").contains("ago"), "{l:#?}");
    assert!(row("┌ bob").contains("┌ bob · just now ─"), "{l:#?}");
    assert!(row("┌ cy").contains("┌ cy · 31 Dec 2025 · edited ─"), "{l:#?}");
    assert!(!row("┌ dee").contains("·"), "{l:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_a_thread_tab_shows_the_time_too() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server) = loaded_app(dir.path(), None).await;
    app.review.clock = Some(NOW);
    {
        let r = review(&mut app);
        let sam = r.threads.iter_mut().find(|t| t.thread.id == "11").unwrap();
        sam.thread.comments[0].created_at = "2026-03-12T07:20:30Z".into();
        sam.thread.comments[0].updated_at = "2026-03-12T07:40:00Z".into();
    }
    changed(&mut app);
    // Row 2 is thread 11; Open Thread opens its tab.
    let len = app.review.review().unwrap().conv_len();
    app.review.view.conv.focused = true;
    app.review.view.conv.select(len, 2);
    app.test_execute(Command::ReviewOpenThread);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert!(app.group().active_diff().is_some_and(|d| d.thread.is_some()));
    let l = draw(&mut app, 120, 30);
    assert!(l.iter().any(|r| r.contains("┌ sam · 3h ago · edited ─")), "{l:#?}");
    app.review.clock = Some(NOW + 2 * 86_400);
    let l = draw(&mut app, 120, 30);
    assert!(l.iter().any(|r| r.contains("┌ sam · 2d ago · edited")), "{l:#?}");
}

// ---- review_conversation-c: writing merge request comments (Gitea end to end; the other
// forges' endpoints are the adapters' unit tests) ----

const ISSUES_POST: &str = "POST /api/v1/repos/alice/project/issues/7/comments";

fn sam_comment(app: &mut App) {
    app.review.user = Some("sam".into());
    review(app).conversation.push(comment("300", "sam", "my words"));
    changed(app);
}

fn sent(server: &crate::forge::fake::FakeServer, method: &str) -> Vec<crate::forge::fake::Request> {
    server.requests().into_iter().filter(|r| r.method == method).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_comment_on_merge_request_posts_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let extra = vec![(ISSUES_POST.to_string(), crate::forge::fake::Reply::json("{}"))];
    let (mut app, server) = crate::mr_review_tests::threads::loaded_app_with(dir.path(), None, None, None, extra).await;
    app.test_execute(Command::ReviewCommentOnMergeRequest);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(app.test_composer_text().as_deref(), Some(""));
    assert_eq!(tabs(&app), 1);
    let l = draw(&mut app, 100, 30).join("\n");
    assert!(l.contains("Comment on the merge request (posted at once)"), "{l}");
    // The box is after the description and ann's comment.
    assert!(l.find("looking").unwrap() < l.find("posted at once").unwrap(), "{l}");
    app.type_text("Looks good to me");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(app.settle(|a| a.test_composer_text().is_none()).await);
    let posts = sent(&server, "POST");
    let post = posts
        .iter()
        .find(|r| r.target.ends_with("issues/7/comments"))
        .expect("a post");
    assert_eq!(post.body, r#"{"body":"Looks good to me"}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_a_refused_post_keeps_the_box_and_its_text() {
    let dir = tempfile::tempdir().unwrap();
    let extra = vec![(
        ISSUES_POST.to_string(),
        crate::forge::fake::Reply::status(500).with_body("{}"),
    )];
    let (mut app, server) = crate::mr_review_tests::threads::loaded_app_with(dir.path(), None, None, None, extra).await;
    app.test_execute(Command::ReviewCommentOnMergeRequest);
    assert!(app.settle(|a| a.diffs_settled()).await);
    app.type_text("keep me");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(
        app.settle(|a| !a.review.writing && !sent(&server, "POST").is_empty())
            .await
    );
    assert_eq!(app.test_composer_text().as_deref(), Some("keep me"));
    assert!(app.review.error.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_own_comments_have_edit_and_delete_buttons() {
    let dir = tempfile::tempdir().unwrap();
    let extra = vec![
        (
            "PATCH /api/v1/repos/alice/project/issues/comments/300".to_string(),
            crate::forge::fake::Reply::json("{}"),
        ),
        (
            "DELETE /api/v1/repos/alice/project/issues/comments/300".to_string(),
            crate::forge::fake::Reply::status(204),
        ),
    ];
    let (mut app, server) = crate::mr_review_tests::threads::loaded_app_with(dir.path(), None, None, None, extra).await;
    sam_comment(&mut app);
    // A review summary of the user's is read-only, and so are others' comments.
    review(&mut app)
        .conversation
        .push(comment("review-5", "sam", "summary"));
    changed(&mut app);
    open(&mut app).await;
    let rows = draw(&mut app, 100, 40);
    assert_eq!(rows.iter().filter(|r| r.contains("[Edit]")).count(), 1, "{rows:#?}");
    let at = |rows: &[String], label: &str| {
        rows.iter()
            .enumerate()
            .find_map(|(y, r)| r.find(label).map(|b| (r[..b].chars().count() as u16, y as u16)))
            .unwrap()
    };
    // Edit: the box holds the comment's text; the forge gets the new one.
    let (x, y) = at(&rows, "[Edit]");
    app.mouse(MouseEventKind::Down(MouseButton::Left), x + 2, y);
    assert_eq!(app.test_composer_text().as_deref(), Some("my words"));
    app.type_text(" and more");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(app.settle(|a| a.test_composer_text().is_none()).await);
    assert_eq!(sent(&server, "PATCH")[0].body, r#"{"body":"my words and more"}"#);
    // Delete confirms first.
    let rows = draw(&mut app, 100, 40);
    let (x, y) = at(&rows, "[Delete]");
    app.mouse(MouseEventKind::Down(MouseButton::Left), x + 2, y);
    assert!(matches!(
        app.prompt,
        Some(crate::app::Prompt::ConfirmDeleteComment { conversation: true, .. })
    ));
    assert!(sent(&server, "DELETE").is_empty());
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|_a| !sent(&server, "DELETE").is_empty()).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_the_box_is_guarded_kept_over_a_refresh_and_cancelled_by_esc() {
    let dir = tempfile::tempdir().unwrap();
    let extra = vec![(ISSUES_POST.to_string(), crate::forge::fake::Reply::json("{}"))];
    let (mut app, server) = crate::mr_review_tests::threads::loaded_app_with(dir.path(), None, None, None, extra).await;
    app.test_execute(Command::ReviewCommentOnMergeRequest);
    assert!(app.settle(|a| a.diffs_settled()).await);
    // An empty box posts nothing.
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(sent(&server, "POST").is_empty());
    assert_eq!(app.test_composer_text().as_deref(), Some(""));
    app.type_text("draft words");
    // A second command while the box is open keeps the text (no new box).
    app.test_execute(Command::ReviewCommentOnMergeRequest);
    assert_eq!(app.test_composer_text().as_deref(), Some("draft words"));
    // A refresh that adds a comment re-syncs the blocks: the box stays, still last.
    review(&mut app)
        .conversation
        .push(comment("301", "bob", "late arrival"));
    changed(&mut app);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let l = draw(&mut app, 100, 40).join("\n");
    assert_eq!(app.test_composer_text().as_deref(), Some("draft words"));
    assert!(l.contains("late arrival") && l.contains("draft words"), "{l}");
    assert!(l.find("late arrival").unwrap() < l.find("draft words").unwrap(), "{l}");
    // Esc cancels without posting.
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.test_composer_text().is_none());
    assert!(sent(&server, "POST").is_empty());
    let l = draw(&mut app, 100, 40).join("\n");
    assert!(!l.contains("posted at once"), "{l}");
}

fn click_label(app: &mut App, label: &str) {
    let rows = draw(app, 100, 40);
    let (y, b) = rows
        .iter()
        .enumerate()
        .find_map(|(y, r)| r.find(label).map(|b| (y as u16, b)))
        .unwrap();
    let x = rows[y as usize][..b].chars().count() as u16 + 2;
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_a_refused_edit_keeps_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let extra = vec![(
        "PATCH /api/v1/repos/alice/project/issues/comments/300".to_string(),
        crate::forge::fake::Reply::status(500).with_body("{}"),
    )];
    let (mut app, server) = crate::mr_review_tests::threads::loaded_app_with(dir.path(), None, None, None, extra).await;
    sam_comment(&mut app);
    open(&mut app).await;
    click_label(&mut app, "[Edit]");
    app.type_text("!");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(
        app.settle(|a| !a.review.writing && !sent(&server, "PATCH").is_empty())
            .await
    );
    assert_eq!(app.test_composer_text().as_deref(), Some("my words!"));
    assert!(app.review.error.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn review_conversation_github_edit_and_delete_go_to_the_issue_comment_endpoints() {
    use crate::forge::fake::Reply;
    use crate::mr_review_tests::github::{boot_github, server_plus};
    let dir = tempfile::tempdir().unwrap();
    let (root, main) = crate::mr_review_tests::make_repo(dir.path());
    let head = crate::git::test_repo::git(&root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    let api = "/api/v3/repos/alice/project";
    let extra = vec![
        (format!("POST {api}/issues/7/comments"), Reply::json("{}")),
        (format!("PATCH {api}/issues/comments/300"), Reply::json("{}")),
        (format!("DELETE {api}/issues/comments/300"), Reply::status(204)),
    ];
    let server = server_plus(&main, &head, true, false, extra);
    let mut app = boot_github(&root, &server).await;
    app.test_execute(Command::ReviewCommentOnMergeRequest);
    assert!(app.settle(|a| a.diffs_settled()).await);
    app.type_text("hi there");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(app.settle(|a| a.test_composer_text().is_none()).await);
    let post = sent(&server, "POST")
        .into_iter()
        .find(|r| r.target.ends_with("/issues/7/comments"))
        .expect("a post");
    assert_eq!(post.body, r#"{"body":"hi there"}"#);
    app.review.user = Some("sam".into());
    review(&mut app).conversation.push(comment("300", "sam", "my words"));
    changed(&mut app);
    assert!(app.settle(|a| a.diffs_settled()).await);
    click_label(&mut app, "[Edit]");
    app.type_text("!");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(app.settle(|a| a.test_composer_text().is_none()).await);
    let patch = &sent(&server, "PATCH")[0];
    assert!(patch.target.ends_with("/issues/comments/300"), "{}", patch.target);
    assert_eq!(patch.body, r#"{"body":"my words!"}"#);
    click_label(&mut app, "[Delete]");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|_a| !sent(&server, "DELETE").is_empty()).await);
    assert!(sent(&server, "DELETE")[0].target.ends_with("/issues/comments/300"));
}

/// review_conversation-d: Submit Review's overall body.
mod summary {
    use std::fs;

    use super::*;
    use crate::forge::fake::Reply;
    use crate::mr_review_tests::idle;
    use crate::mr_review_tests::write::{
        REVIEWS, boot, comment as draft, json, repo, requests, review_post, server, store_text,
    };

    fn send(app: &mut App) {
        app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_conversation_submit_asks_for_a_summary_and_posts_it_with_the_review() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, Some(state.clone())).await;
        draft(&mut app, 1, 2, "Fix this range");
        assert!(app.settle(|_| store_text(&state).contains("Fix this range")).await);
        app.test_execute(Command::ReviewSubmitApprove);
        assert_eq!(app.test_composer_text().as_deref(), Some(""));
        assert!(
            requests(&server, "POST").iter().all(|r| r.target != REVIEWS),
            "nothing sent yet"
        );
        assert!(app.settle(|a| a.diffs_settled()).await);
        let l = draw(&mut app, 100, 30).join("\n");
        assert!(l.contains("Review summary (optional)"), "{l}");
        app.type_text("Nice work overall");
        send(&mut app);
        assert!(
            app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
                .await
        );
        idle(&mut app).await;
        let v = json(&review_post(&server).unwrap());
        assert_eq!(
            (v["event"].as_str(), v["body"].as_str()),
            (Some("APPROVED"), Some("Nice work overall"))
        );
        assert_eq!(v["comments"].as_array().unwrap().len(), 1);
        assert!(
            app.test_composer_text().is_none(),
            "the box closes once the forge agrees"
        );
        assert!(app.settle(|_| !store_text(&state).contains("Nice work")).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_conversation_an_empty_box_submits_as_before_and_a_summary_alone_is_a_review() {
        let dir = tempfile::tempdir().unwrap();
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, None).await;
        draft(&mut app, 1, 2, "x");
        app.test_execute(Command::ReviewSubmitRequestChanges);
        send(&mut app);
        assert!(
            app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
                .await
        );
        let v = json(&review_post(&server).unwrap());
        assert_eq!(v["body"].as_str(), Some("Changes requested"), "the forge's fallback");
        idle(&mut app).await;
        // No drafts: an empty Comment review is refused (the box stays), a summary is not.
        app.test_execute(Command::ReviewSubmitComment);
        send(&mut app);
        assert!(app.message.as_ref().unwrap().text.contains("No drafts"));
        assert_eq!(app.test_composer_text().as_deref(), Some(""));
        app.type_text("Just a note");
        send(&mut app);
        assert!(
            app.settle(|a| !a.review.writing && a.test_composer_text().is_none())
                .await
        );
        let posts: Vec<_> = requests(&server, "POST")
            .into_iter()
            .filter(|r| r.target == REVIEWS)
            .collect();
        assert_eq!(posts.len(), 2);
        let v = json(&posts[1]);
        assert_eq!(
            (v["event"].as_str(), v["body"].as_str()),
            (Some("COMMENT"), Some("Just a note"))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_conversation_esc_sends_nothing_and_keeps_the_typed_summary() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, head, main) = repo(dir.path());
        let server = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server, Some(state.clone())).await;
        draft(&mut app, 1, 2, "x");
        app.test_execute(Command::ReviewSubmitComment);
        app.type_text("half a thought");
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.test_composer_text().is_none());
        assert!(review_post(&server).is_none());
        assert!(app.settle(|_| store_text(&state).contains("half a thought")).await);
        app.test_execute(Command::ReviewSubmitApprove);
        assert_eq!(app.test_composer_text().as_deref(), Some("half a thought"));
        // Emptying it and Esc forgets it.
        app.press(KeyCode::Char('a'), KeyModifiers::CONTROL);
        app.press(KeyCode::Backspace, KeyModifiers::NONE);
        app.press(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.settle(|_| !store_text(&state).contains("half a thought")).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn review_conversation_a_failed_submit_keeps_the_box_and_the_summary_over_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let (root, head, main) = repo(dir.path());
        let failing = Reply::status(500).with_body(r#"{"message":"boom"}"#);
        let server1 = server(&main, &head, &head, failing);
        let mut app = boot(&root, &server1, Some(state.clone())).await;
        draft(&mut app, 1, 2, "Keep me");
        app.test_execute(Command::ReviewSubmitComment);
        app.type_text("Please read");
        send(&mut app);
        assert!(
            app.settle(|a| !a.review.writing && !a.review.drafts.errors.is_empty())
                .await
        );
        assert_eq!(app.test_composer_text().as_deref(), Some("Please read"));
        assert!(app.settle(|_| store_text(&state).contains("Please read")).await);
        assert!(
            fs::read_to_string(state.join("review-drafts.json"))
                .unwrap()
                .contains("Keep me")
        );
        drop(app);
        let server2 = server(&main, &head, &head, Reply::json("{}"));
        let mut app = boot(&root, &server2, Some(state.clone())).await;
        app.test_execute(Command::ReviewSubmitComment);
        assert_eq!(app.test_composer_text().as_deref(), Some("Please read"));
        send(&mut app);
        assert!(
            app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
                .await
        );
        let v = json(&review_post(&server2).unwrap());
        assert_eq!(v["body"].as_str(), Some("Please read"));
        assert!(app.settle(|_| !store_text(&state).contains("Please read")).await);
    }
}
