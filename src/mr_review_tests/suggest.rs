//! mr_review-g, suggestions: the fence drawn with an [Apply Suggestion] button, Apply as one undo
//! step that refuses when the lines are out of date, Insert Suggestion in the comment box.

use super::*;
use crate::diff_caret::Caret;
use crossterm::event::{MouseButton, MouseEventKind};

const REVIEWS: &str = "/api/v1/repos/alice/project/pulls/7/reviews";

fn lines(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect()
}

fn message(app: &App) -> String {
    app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

/// A fake forge with these comments (a JSON array); the merge request's head is `head`.
fn server(main: &str, head: &str, comments: &str) -> FakeServer {
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
        (format!("{PULLS}/7/reviews/1/comments"), Reply::json(comments)),
        (
            "/api/v1/repos/alice/project/issues/7/comments?page=1&limit=50".into(),
            Reply::json("[]"),
        ),
        ("/api/v1/user".into(), Reply::json(r#"{"login":"sam"}"#)),
        (format!("POST {REVIEWS}"), Reply::json("{}")),
    ])
}

/// The repository (mod.txt CRLF when asked: committed so, so the thread places), and the server
/// with `comments(head, main)`.
fn world(dir: &Path, crlf: bool, comments: impl Fn(&str, &str) -> String) -> (PathBuf, FakeServer) {
    let (root, main) = make_repo(dir);
    if crlf {
        fs::write(root.join("mod.txt"), "m1\r\nm2 changed\r\nm3\r\n").unwrap();
        commit_all(&root, "crlf");
    }
    let head = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let server = server(&main, &head, &comments(&head, &main));
    (root, server)
}

/// bob's thread on `mod.txt` line 3 (`m3`) with `body` (JSON-escaped).
fn on_line3(body: &'static str) -> impl Fn(&str, &str) -> String {
    move |head, _| format!("[{}]", threads::rc(11, "bob", body, "mod.txt", 3, head, "+m3", false))
}

async fn boot(root: &Path, server: &FakeServer) -> App {
    let mut app = open_app(root, None, config_for(server), true);
    assert!(app.settle(loaded).await);
    assert!(app.settle(|a| a.git.status.is_some()).await);
    app.test_execute(Command::ShowReview);
    idle(&mut app).await;
    assert!(app.settle(|a| a.review.drafts.store.is_some()).await);
    screen(&mut app);
    open_file(&mut app, "mod.txt").await;
    draw_wide(&mut app, 120);
    app
}

fn select(app: &mut App, right: bool, first: usize, last: usize) {
    app.group_mut().active_diff_mut().unwrap().go_to_side_line(right, last);
    draw_wide(app, 120);
    app.group_mut().active_diff_mut().unwrap().caret = Caret {
        right,
        anchor: (first, 0),
        head: (last, 1),
        col: None,
    };
}

fn text(app: &App, root: &Path) -> String {
    let id = app.buffers.find_path(&root.join("mod.txt")).expect("mod.txt is open");
    app.buffers.get(id).unwrap().rope.to_string()
}

/// One undo of the file's buffer.
fn undo(app: &mut App, root: &Path) {
    let id = app.buffers.find_path(&root.join("mod.txt")).unwrap();
    assert!(app.buffers.get_mut(id).unwrap().undo().is_some());
}

/// Apply on line 3, then give the background work time to answer.
async fn apply(app: &mut App) {
    select(app, true, 2, 2);
    app.test_execute(Command::ReviewApplySuggestion);
    for _ in 0..40 {
        app.test_turn(Duration::from_millis(25)).await;
    }
}

const ONE: &str = "Use this\\n```suggestion\\nm3 fixed\\nm4\\n```";

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_a_suggestion_is_drawn_as_code_with_an_apply_button() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3(ONE));
    let mut app = boot(&root, &server).await;
    let l = lines(&mut app, 120, 30);
    let code = l
        .iter()
        .position(|x| x.contains("│ m3 fixed"))
        .unwrap_or_else(|| panic!("{l:#?}"));
    assert!(l[code + 1].contains("│ m4"), "{l:#?}");
    let row = l
        .iter()
        .position(|x| x.contains("[Reply] [Resolve] [Apply Suggestion]"))
        .unwrap();
    assert!(row > code);
    // The code lines carry the inserted lines' background.
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let x = l[code].find("m3 fixed").unwrap();
    let x = l[code][..x].chars().count() as u16;
    let want = app.theme.get("diffEditor.insertedLineBackground").bg;
    assert_eq!(term.backend().buffer()[(x, code as u16)].style().bg, want);
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_apply_replaces_the_lines_in_one_undo_step_and_touches_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3(ONE));
    let mut app = boot(&root, &server).await;
    // The button: a click on [Apply Suggestion].
    let l = lines(&mut app, 120, 30);
    let row = l.iter().position(|x| x.contains("[Apply Suggestion]")).unwrap();
    let col = l[row].split("[Apply").next().unwrap().chars().count() as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), col + 2, row as u16);
    assert!(
        app.settle(|a| a.buffers.find_path(&root.join("mod.txt")).is_some_and(|id| a
            .buffers
            .get(id)
            .unwrap()
            .is_dirty()))
            .await
    );
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3 fixed\nm4\n");
    assert_eq!(
        fs::read_to_string(root.join("mod.txt")).unwrap(),
        "m1\nm2 changed\nm3\n"
    );
    assert!(message(&app).contains("applied"), "{}", message(&app));
    assert!(app.review.review().unwrap().threads.iter().all(|t| !t.thread.resolved));
    assert!(
        !server
            .requests()
            .iter()
            .any(|r| r.method == "POST" || r.method == "PATCH"),
        "nothing is sent to the forge"
    );
    undo(&mut app, &root);
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3\n", "one undo restores it");
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_an_edited_line_makes_the_suggestion_out_of_date_and_a_second_apply_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3(ONE));
    let mut app = boot(&root, &server).await;
    app.test_open_file(root.join("mod.txt"));
    assert!(
        app.settle(|a| a.buffers.find_path(&root.join("mod.txt")).is_some())
            .await
    );
    app.test_activate_tab(0);
    // Typed over line 3 in the diff: the buffer no longer reads as the comment's commit.
    select(&mut app, true, 2, 2);
    app.type_text("X");
    assert_eq!(text(&app, &root), "m1\nm2 changed\nX3\n");
    apply(&mut app).await;
    assert!(message(&app).contains("out of date"), "{}", message(&app));
    assert_eq!(text(&app, &root), "m1\nm2 changed\nX3\n");
    // Undone, it applies; applied twice, the second finds the lines changed.
    undo(&mut app, &root);
    apply(&mut app).await;
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3 fixed\nm4\n");
    app.message = None;
    apply(&mut app).await;
    assert!(message(&app).contains("out of date"), "{}", message(&app));
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3 fixed\nm4\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_an_empty_suggestion_over_a_range_deletes_the_lines() {
    let dir = tempfile::tempdir().unwrap();
    let body = "```suggestion\\n```\\n\\n<!-- jed:range=2-3 -->";
    let (root, server) = world(dir.path(), false, on_line3(body));
    let mut app = boot(&root, &server).await;
    apply(&mut app).await;
    assert_eq!(text(&app, &root), "m1\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_a_crlf_file_keeps_its_line_endings() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), true, on_line3(ONE));
    let mut app = boot(&root, &server).await;
    apply(&mut app).await;
    assert_eq!(text(&app, &root), "m1\r\nm2 changed\r\nm3 fixed\r\nm4\r\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_a_deleted_lines_thread_has_no_apply_and_the_command_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let comments = |_: &str, main: &str| format!("[{}]", threads::rc(12, "bob", ONE, "mod.txt", 2, main, "-m2", false));
    let (root, server) = world(dir.path(), false, comments);
    let mut app = boot(&root, &server).await;
    let l = lines(&mut app, 120, 30).join("\n");
    assert!(l.contains("[Reply]") && !l.contains("[Apply Suggestion]"), "{l}");
    select(&mut app, false, 1, 1);
    app.test_execute(Command::ReviewApplySuggestion);
    assert!(message(&app).contains("new side"), "{}", message(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_insert_suggestion_fills_the_selected_lines_and_the_draft_is_submitted() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, |_, _| "[]".into());
    let mut app = boot(&root, &server).await;
    app.test_execute(Command::ReviewInsertSuggestion);
    assert!(message(&app).contains("comment box"), "{}", message(&app));
    select(&mut app, true, 1, 2);
    app.test_execute(Command::ReviewComment);
    app.test_execute(Command::ReviewInsertSuggestion);
    assert_eq!(
        app.test_composer_text().as_deref(),
        Some("```suggestion\nm2 changed\nm3\n```")
    );
    // The caret is inside the fence, on the first line.
    app.type_text("X");
    assert_eq!(
        app.test_composer_text().as_deref(),
        Some("```suggestion\nXm2 changed\nm3\n```")
    );
    let l = lines(&mut app, 120, 30).join("\n");
    assert!(l.contains("[Insert Suggestion]"), "{l}");
    app.press(KeyCode::Enter, KeyModifiers::CONTROL);
    app.test_execute(Command::ReviewSubmitComment);
    assert!(
        app.settle(|a| a.review.draft_rows().is_empty() && !a.review.writing)
            .await
    );
    let post = server
        .requests()
        .into_iter()
        .find(|r| r.method == "POST" && r.target == REVIEWS)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&post.body).unwrap();
    assert_eq!(
        v["comments"][0]["body"],
        "```suggestion\nXm2 changed\nm3\n```\n\n<!-- jed:range=2-3 -->"
    );
    // A left-side box refuses.
    select(&mut app, false, 1, 1);
    app.test_execute(Command::ReviewComment);
    app.test_execute(Command::ReviewInsertSuggestion);
    assert!(message(&app).contains("new side"), "{}", message(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_the_undo_key_on_the_diff_tab_undoes_the_apply() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3(ONE));
    let mut app = boot(&root, &server).await;
    apply(&mut app).await;
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3 fixed\nm4\n");
    // The diff reloads, then the user presses the real undo key with the review diff active.
    for _ in 0..20 {
        app.test_turn(Duration::from_millis(25)).await;
    }
    draw_wide(&mut app, 120);
    assert!(app.group().active_diff().is_some());
    app.press(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3\n", "{}", message(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn mr_review_the_undo_key_works_with_the_caret_on_the_left_side() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3(ONE));
    let mut app = boot(&root, &server).await;
    apply(&mut app).await;
    select(&mut app, false, 0, 0);
    app.press(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3\n", "{}", message(&app));
}

/// The Done-when walk's suggestion part: Apply is one undo step, and a second Apply (or one over
/// an edited line) is refused as out of date.
#[tokio::test(flavor = "multi_thread")]
async fn mr_review_done_when_a_suggestion_applies_as_one_undo_step_and_refuses_when_out_of_date() {
    let dir = tempfile::tempdir().unwrap();
    let (root, server) = world(dir.path(), false, on_line3(ONE));
    let mut app = boot(&root, &server).await;
    apply(&mut app).await;
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3 fixed\nm4\n");
    undo(&mut app, &root);
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3\n", "one undo step");
    apply(&mut app).await;
    app.message = None;
    apply(&mut app).await;
    assert!(message(&app).contains("out of date"), "{}", message(&app));
    assert_eq!(text(&app, &root), "m1\nm2 changed\nm3 fixed\nm4\n");
}
