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
