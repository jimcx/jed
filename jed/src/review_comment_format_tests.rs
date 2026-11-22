//! review_comment_format-a's "done when", through the command: with a comment box open on each
//! forge kind (against the fake servers of `mr_review_tests`), `Command::ReviewFormat(style)`
//! wraps, prefixes or inserts at the box's selection in that forge's form and keeps the selection
//! on the text, a second run toggles it off, one undo removes a format, and a style the forge
//! lacks notifies and changes nothing. The toolbar row and its clicks are subtask b's.

use std::path::Path;

use crate::app::App;
use crate::command::Command;
use crate::comment_format::{self, Style};
use crate::diff_caret::Caret;
use crate::forge::Kind;
use crate::forge::fake::Reply;
use crate::mr_review_tests::{draw_wide, github, gitlab, make_repo, open_file, suggest};

/// An app on the standard repository against a fake `kind` server, `mod.txt`'s review diff open.
async fn boot(dir: &Path, kind: Kind) -> App {
    match kind {
        Kind::Gitea => {
            let (root, server) = suggest::world(dir, false, |_, _| "[]".into());
            let app = suggest::boot(&root, &server).await;
            std::mem::forget(server);
            app
        }
        Kind::GitHub => {
            let (root, main) = make_repo(dir);
            let head = crate::git::test_repo::git(&root, &["rev-parse", "HEAD"])
                .trim()
                .to_string();
            let server = github::server(&main, &head);
            let mut app = github::boot_github(&root, &server).await;
            open_file(&mut app, "mod.txt").await;
            draw_wide(&mut app, 120);
            std::mem::forget(server);
            app
        }
        Kind::GitLab => {
            let (root, main) = make_repo(dir);
            let head = crate::git::test_repo::git(&root, &["rev-parse", "HEAD"])
                .trim()
                .to_string();
            let server = gitlab::server(&main, &head, Reply::json("{}"));
            let app = gitlab::boot(&root, &server, None).await;
            std::mem::forget(server);
            app
        }
    }
}

/// A comment box with `text` typed in.
fn open_box(app: &mut App, text: &str) {
    let d = app.group_mut().active_diff_mut().unwrap();
    d.go_to_side_line(true, 2);
    draw_wide(app, 120);
    app.group_mut().active_diff_mut().unwrap().caret = Caret {
        right: true,
        anchor: (1, 0),
        head: (2, 1),
        col: None,
    };
    app.test_execute(Command::ReviewComment);
    app.type_text(text);
    assert_eq!(app.test_composer_text().as_deref(), Some(text));
}

fn text(app: &App) -> String {
    app.test_composer_text().unwrap()
}

fn message(app: &App) -> String {
    app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

const KINDS: [Kind; 3] = [Kind::Gitea, Kind::GitHub, Kind::GitLab];

/// What `style` makes of "hello world" with "world" selected, and the selection after.
fn expected(style: Style) -> (&'static str, &'static str) {
    match style {
        Style::Bold => ("hello **world**", "world"),
        Style::Italic => ("hello _world_", "world"),
        Style::Underline => ("hello <ins>world</ins>", "world"),
        Style::Strike => ("hello ~~world~~", "world"),
        Style::Code => ("hello `world`", "world"),
        Style::Link => ("hello [world](url)", "url"),
        Style::Quote => ("> hello world", "world"),
        Style::Bullets => ("- hello world", "world"),
        Style::Numbers => ("1. hello world", "world"),
        Style::Tasks => ("- [ ] hello world", "world"),
        Style::Heading => ("### hello world", "world"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_every_style_on_every_forge_and_one_undo() {
    for kind in KINDS {
        let dir = tempfile::tempdir().unwrap();
        let mut app = boot(dir.path(), kind).await;
        open_box(&mut app, "hello world");
        for style in comment_format::styles(kind) {
            app.test_composer_select(6, 11);
            app.test_execute(Command::ReviewFormat(*style));
            let (want, selected) = expected(*style);
            assert_eq!(text(&app), want, "{kind:?} {style:?}");
            assert_eq!(app.test_composer_selected(), selected, "{kind:?} {style:?}");
            // One undo step removes it.
            app.test_execute(Command::Undo);
            assert_eq!(text(&app), "hello world", "{kind:?} {style:?} undo");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_a_second_run_toggles_off_and_lines_and_carets_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    open_box(&mut app, "one\ntwo");
    app.test_composer_select(0, 7);
    app.test_execute(Command::ReviewFormat(Style::Numbers));
    assert_eq!(text(&app), "1. one\n2. two");
    app.test_execute(Command::ReviewFormat(Style::Numbers));
    assert_eq!(text(&app), "one\ntwo");
    app.test_composer_select(0, 3);
    app.test_execute(Command::ReviewFormat(Style::Bold));
    app.test_execute(Command::ReviewFormat(Style::Bold));
    assert_eq!(text(&app), "one\ntwo");
    // A caret gets the empty pair, and typing goes between.
    app.test_composer_select(3, 3);
    app.test_execute(Command::ReviewFormat(Style::Code));
    app.type_text("x");
    assert_eq!(text(&app), "one`x`\ntwo");
    // Several lines of code are fenced.
    let len = text(&app).len();
    app.test_composer_select(0, len);
    app.test_execute(Command::ReviewFormat(Style::Code));
    assert_eq!(text(&app), "```\none`x`\ntwo\n```");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_without_a_box_or_for_an_unsupported_style_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::Gitea).await;
    app.test_execute(Command::ReviewFormat(Style::Bold));
    assert!(message(&app).contains("comment box"), "{}", message(&app));
    open_box(&mut app, "hello world");
    app.test_composer_select(6, 11);
    app.test_format_unsupported(Kind::Gitea, Style::Underline);
    assert_eq!(message(&app), "Gitea has no underline");
    assert_eq!(text(&app), "hello world");
    assert_eq!(app.test_composer_selected(), "world");
}

#[test]
fn review_comment_format_every_style_is_a_palette_command() {
    for s in Style::ALL {
        let c = Command::ReviewFormat(s);
        assert!(Command::all().contains(&c));
        assert_eq!(Command::from_name(&c.name()), Some(c));
        assert!(c.title().starts_with("Code Review: "));
    }
}

/// The drawn screen, one string.
fn screen(app: &mut App) -> String {
    use ratatui::{Terminal, backend::TestBackend};
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_each_format_is_its_own_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    open_box(&mut app, "hello world");
    app.test_composer_select(6, 11);
    app.test_execute(Command::ReviewFormat(Style::Bold));
    app.test_execute(Command::ReviewFormat(Style::Italic));
    assert_eq!(text(&app), "hello **_world_**");
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), "hello **world**");
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), "hello world");
    // Typing before a format doesn't join it either.
    app.test_composer_select(11, 11);
    app.type_text("!");
    app.test_composer_select(0, 5);
    app.test_execute(Command::ReviewFormat(Style::Bold));
    app.test_execute(Command::Undo);
    assert_eq!(text(&app), "hello world!");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_the_drawn_box_follows_the_formatted_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    open_box(&mut app, "hello world");
    assert!(!screen(&mut app).contains("```"));
    app.test_composer_select(0, 5);
    app.test_execute(Command::ReviewFormat(Style::Quote));
    assert!(screen(&mut app).contains("> hello world"));
    app.test_composer_select(0, 0);
    app.type_text("a\nb");
    let len = text(&app).len();
    app.test_composer_select(0, len);
    app.test_execute(Command::ReviewFormat(Style::Code));
    let s = screen(&mut app);
    assert_eq!(s.matches("```").count(), 2, "{s}");
}

// ---- review_comment_format-b: the toolbar row, through the drawn hit map ----

use crossterm::event::{MouseButton, MouseEventKind};

/// The drawn screen at `w` columns (rows) and the terminal's cursor cell.
fn draw_at(app: &mut App, w: u16) -> (Vec<String>, (u16, u16)) {
    use ratatui::{Terminal, backend::TestBackend};
    let mut term = Terminal::new(TestBackend::new(w, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let cur = term.get_cursor_position().unwrap();
    let buf = term.backend().buffer().clone();
    let rows = (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect();
    (rows, (cur.x, cur.y))
}

/// The toolbar's screen row and text.
fn toolbar(rows: &[String]) -> (usize, String) {
    let y = rows
        .iter()
        .position(|r| r.contains("[B]"))
        .unwrap_or_else(|| panic!("a toolbar row in {rows:#?}"));
    (y, rows[y].clone())
}

fn click(app: &mut App, x: usize, y: usize) {
    app.mouse(MouseEventKind::Down(MouseButton::Left), x as u16, y as u16);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x as u16, y as u16);
}

/// Clicks the `[label]` button on the drawn toolbar.
fn click_tool(app: &mut App, w: u16, label: &str) {
    let (rows, _) = draw_at(app, w);
    let (y, row) = toolbar(&rows);
    let at = row
        .find(&format!("[{label}]"))
        .unwrap_or_else(|| panic!("no [{label}] in {row}"));
    click(app, row[..at].chars().count() + 1, y);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_the_toolbar_lists_the_forges_styles_in_order() {
    for kind in KINDS {
        let dir = tempfile::tempdir().unwrap();
        let mut app = boot(dir.path(), kind).await;
        open_box(&mut app, "hello world");
        // Wide enough for every label in a half-width box.
        let (rows, _) = draw_at(&mut app, 240);
        let (_, row) = toolbar(&rows);
        let want: Vec<String> = comment_format::styles(kind)
            .iter()
            .map(|s| format!("[{}]", s.label()))
            .collect();
        let want = want.join(" ");
        let at = row.find("│ [B]").unwrap() + "│ ".len();
        let got: String = row[at..].chars().take(want.chars().count()).collect();
        assert_eq!(got, want, "{kind:?}");
        // The block carries the review's forge kind, so a per-forge difference reaches the toolbar.
        let d = app.group_mut().active_diff_mut().unwrap();
        let carried = d.threads.list().iter().find_map(|t| match t.role {
            crate::review_blocks::Role::Composer { kind, .. } => Some(kind),
            _ => None,
        });
        assert_eq!(carried, Some(kind));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_clicking_each_label_applies_its_style_and_toggles_off() {
    for kind in KINDS {
        let dir = tempfile::tempdir().unwrap();
        let mut app = boot(dir.path(), kind).await;
        open_box(&mut app, "hello world");
        for style in comment_format::styles(kind) {
            app.test_composer_select(6, 11);
            click_tool(&mut app, 120, style.label());
            let (want, selected) = expected(*style);
            assert_eq!(text(&app), want, "{kind:?} {style:?}");
            assert_eq!(app.test_composer_selected(), selected, "{kind:?} {style:?}");
            // One undo removes it.
            app.test_execute(Command::Undo);
            assert_eq!(text(&app), "hello world", "{kind:?} {style:?} undo");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    open_box(&mut app, "hello world");
    app.test_composer_select(6, 11);
    click_tool(&mut app, 120, "B");
    assert_eq!(text(&app), "hello **world**");
    click_tool(&mut app, 120, "B");
    assert_eq!(text(&app), "hello world");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_a_narrow_box_drops_labels_from_the_right() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    open_box(&mut app, "hello world");
    let (rows, _) = draw_at(&mut app, 56);
    let (_, row) = toolbar(&rows);
    assert!(row.contains("[B]") && row.contains("[S]"), "{row}");
    assert!(!row.contains("[H]"), "{row}");
    // Every drawn label is whole: the row's brackets pair up, ending on the last label that fits.
    let tools = &row[row.find("│ [B]").unwrap() + "│ ".len()..];
    let drawn: Vec<&str> = tools
        .split_whitespace()
        .take_while(|t| t.starts_with('[') && t.ends_with(']'))
        .collect();
    let all: Vec<String> = comment_format::styles(Kind::GitHub)
        .iter()
        .map(|s| format!("[{}]", s.label()))
        .collect();
    assert!(!drawn.is_empty() && drawn.len() < all.len(), "{row}");
    assert_eq!(drawn, all[..drawn.len()], "{row}");
    app.test_composer_select(6, 11);
    click_tool(&mut app, 56, "I");
    assert_eq!(text(&app), "hello _world_");
    // A click past the last label does nothing.
    let (rows, _) = draw_at(&mut app, 56);
    let (y, row) = toolbar(&rows);
    let end = row.trim_end().chars().count();
    let before = text(&app);
    click(&mut app, end + 1, y);
    assert_eq!(text(&app), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_text_rows_still_take_clicks_and_the_caret_is_drawn_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    open_box(&mut app, "one\ntwo");
    let (rows, _) = draw_at(&mut app, 120);
    let y = rows.iter().position(|r| r.contains("│ two")).unwrap();
    let x = rows[y].chars().position(|c| c == 't').unwrap();
    click(&mut app, x + 2, y);
    let (_, cur) = draw_at(&mut app, 120);
    assert_eq!(cur, ((x + 2) as u16, y as u16));
    app.type_text("X");
    assert_eq!(text(&app), "one\ntwXo");
    // The first text row too.
    click(&mut app, x + 1, y - 1);
    app.type_text("Y");
    assert_eq!(text(&app), "oYne\ntwXo");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_the_toolbar_is_on_the_conversation_box_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    app.test_execute(Command::ReviewCommentOnMergeRequest);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert!(app.test_composer_text().is_some());
    app.type_text("hello");
    let (rows, _) = draw_at(&mut app, 120);
    toolbar(&rows);
    app.test_composer_select(0, 5);
    click_tool(&mut app, 120, "B");
    assert_eq!(text(&app), "**hello**");
}

#[tokio::test(flavor = "multi_thread")]
async fn review_comment_format_the_toolbar_is_on_the_summary_and_edit_boxes_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = boot(dir.path(), Kind::GitHub).await;
    open_box(&mut app, "hello world");
    // The edit box: the saved draft opened again.
    app.press_bound(Command::ReviewSaveComment);
    assert!(app.test_composer_text().is_none());
    draw_wide(&mut app, 120);
    app.test_execute(Command::ReviewEditDraft);
    assert_eq!(app.test_composer_text().as_deref(), Some("hello world"));
    app.test_composer_select(6, 11);
    click_tool(&mut app, 120, "B");
    assert_eq!(text(&app), "hello **world**");
    app.press_bound(Command::ReviewCancelComment);
    // The summary box.
    app.test_execute(Command::ReviewSubmitComment);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(app.test_composer_text().as_deref(), Some(""));
    app.type_text("done");
    app.test_composer_select(0, 4);
    click_tool(&mut app, 120, "I");
    assert_eq!(text(&app), "_done_");
}
