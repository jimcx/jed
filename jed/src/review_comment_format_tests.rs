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
