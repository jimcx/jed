//! End-to-end tests for the Diagnostics and quick fixes milestone (`fixes`). So far fixes-a,
//! diagnostics in hover: resting the mouse on a squiggle shows its diagnostics (most severe
//! first: the icon in its severity's colour, the message with its other lines, `source(code)`
//! dimmed; a rule between them) as soon as the rest ends, before the server answers; the
//! server's hover follows below a rule. With no `hoverProvider`, or a null answer, the popup
//! keeps the diagnostics alone. A squiggle on whitespace, under a line's last char (an empty
//! range at its end) or past an empty line's end is hovered too; plain whitespace and the cells
//! past a line's end aren't. Moving along the squiggle keeps the popup, moving off closes it, an
//! edit closes it, and a mark mapped through an edit is found at its new place. Show Hover at
//! the cursor shows the diagnostics touching it (a range ending there too) the same way; "No
//! hover information" only when there's neither. A failed answer leaves the diagnostics too.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::MouseEventKind as E;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};

use crate::app::App;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::milestone9_tests::{
    diag, draw, find_cells, idle, open_app, open_synced, popup_rows, project, publish, put_cursor, show_hover,
};
use crate::syntax::Language;

/// Five comment lines, then the squiggled lines (5–9).
const TEXT: &str = "// one\n// two\n// three\n// four\n// five\nint x = value;\nint y  = 2;\nint z = 3\n\nint w = 4;\n";

const UNDECLARED: &str = "use of undeclared identifier 'value'\nnote: did you mean 'valve'?";

fn terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(100, 30)).unwrap()
}

/// `a.cpp` open and synced with a fake server (`no_hover`: without `hoverProvider`), its
/// diagnostics published: on line 5 a warning over `x = value` and an error on `value` (listed
/// after it); on line 6 an info on the two spaces; an empty-range error at line 7's end; an
/// empty-range info on the empty line 8.
async fn fixes_app(dir: &Path, no_hover: bool) -> (App, Arc<FakeLauncher>, PathBuf) {
    let root = project(dir, &[("a.cpp", TEXT)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        no_hover,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    let path = root.join("a.cpp");
    open_synced(&mut app, &fake, &path).await;
    let with = |mut d: Value, source: &str, code: Option<&str>| {
        d["source"] = json!(source);
        if let Some(code) = code {
            d["code"] = json!(code);
        }
        d
    };
    let diags = json!([
        with(
            diag(5, 4, 13, 2, "unused variable 'x'"),
            "clang",
            Some("-Wunused-variable")
        ),
        with(diag(5, 8, 13, 1, UNDECLARED), "clang", Some("undeclared_var_use")),
        with(diag(6, 5, 7, 3, "extra whitespace"), "lint", None),
        with(
            diag(7, 9, 9, 1, "expected ';' after top level declarator"),
            "clang",
            None
        ),
        diag(8, 0, 0, 3, "empty line here"),
    ]);
    publish(&app, Language::Cpp, &path, None, diags);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().diagnostics.all().len() == 5)
            .await
    );
    (app, fake, root)
}

/// The screen cell of `col` on buffer line `line` (the view unscrolled).
fn cell(app: &App, line: u16, col: u16) -> (u16, u16) {
    let text = app.hits.group(app.groups.active).unwrap().text;
    (text.x + col, text.y + line)
}

/// Draws, moves the pointer to `at` and fires the resting timer: true when a rest started there
/// (the pointer is on something to hover).
fn rest(app: &mut App, term: &mut Terminal<TestBackend>, (x, y): (u16, u16)) -> bool {
    draw(term, app);
    app.mouse(E::Moved, x, y);
    let Some(deadline) = app.hover.rest.as_ref().map(|r| r.deadline) else {
        return false;
    };
    app.test_hover_tick(deadline);
    true
}

/// The popup's rows as drawn (rules as "---").
fn rows(term: &mut Terminal<TestBackend>, app: &mut App) -> Vec<String> {
    draw(term, app);
    popup_rows(term, app)
        .into_iter()
        .map(|r| {
            if !r.is_empty() && r.chars().all(|c| c == '─') {
                "---".into()
            } else {
                r
            }
        })
        .collect()
}

fn diagnostics_rows() -> Vec<&'static str> {
    vec![
        "⊗ use of undeclared identifier 'value'",
        "note: did you mean 'valve'?",
        "clang(undeclared_var_use)",
        "---",
        "⚠ unused variable 'x'",
        "clang(-Wunused-variable)",
    ]
}

/// The foreground of the first cell in the popup drawing `symbol`.
fn fg_of(term: &Terminal<TestBackend>, app: &App, symbol: &str) -> Option<ratatui::style::Color> {
    let r = app.hits.hover?;
    let buf = term.backend().buffer();
    (r.y..r.y + r.height)
        .flat_map(|y| (r.x..r.x + r.width).map(move |x| (x, y)))
        .find(|&p| buf[p].symbol() == symbol)
        .and_then(|p| buf[p].fg.into())
}

/// Resting on `value`: its diagnostics (the error first though published second, then the
/// warning over `x = value`) show at once, while the server's answer is held; when it comes it
/// follows below a rule. The icons take their severity's colour, the source the dimmed one.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_hover_shows_diagnostics_then_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = fixes_app(dir.path(), false).await;
    fake.respond("textDocument/hover", json!({ "contents": "about `value`" }));
    fake.hold("textDocument/hover", true);
    let mut term = terminal();
    draw(&mut term, &mut app);
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    assert!(rest(&mut app, &mut term, (vx + 1, vy)));
    let popup = app
        .hover
        .popup
        .clone()
        .expect("shown when the rest ends, before any answer");
    let value = TEXT.find("value;").unwrap();
    assert_eq!(popup.anchor.0, value, "drawn from the word, not the warning's start");
    assert_eq!(
        (popup.range.0.0, popup.range.1.0),
        (value - 4, value + 5),
        "the word and both squiggles"
    );
    draw(&mut term, &mut app);
    let r = app.hits.hover.unwrap();
    assert_eq!(r.x, vx, "the popup starts at `value`, not at `x`");
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    let fg = |key: &str| app.theme.get(key).fg;
    assert_eq!(fg_of(&term, &app, "⊗"), fg("problemsErrorIcon.foreground"));
    assert_eq!(fg_of(&term, &app, "⚠"), fg("problemsWarningIcon.foreground"));
    assert_eq!(fg_of(&term, &app, "("), fg("editorHoverWidget.source"));
    assert_ne!(fg("editorHoverWidget.source"), fg("editorHoverWidget.foreground"));
    assert_eq!(
        fg_of(&term, &app, "u"),
        fg("editorHoverWidget.foreground"),
        "the message"
    );

    // The request still went out; its answer is appended.
    assert!(app.settle(|_| !fake.requests("textDocument/hover").is_empty()).await);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(
        app.settle(|a| a.hover.popup.as_ref().is_some_and(|p| p.lines.len() > 7))
            .await
    );
    let mut want = diagnostics_rows();
    want.extend(["---", "about value"]);
    assert_eq!(rows(&mut term, &mut app), want);
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;
}

/// Without `hoverProvider` the popup is the diagnostics alone (no request, no "No hover
/// information"); a server answering null leaves them alone too. Show Hover at the cursor shows
/// the diagnostics touching it (both ranges end right after `value`); off any, with no answer,
/// it says "No hover information".
#[tokio::test(flavor = "multi_thread")]
async fn fixes_hover_diagnostics_only() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = fixes_app(dir.path(), true).await;
    let mut term = terminal();
    draw(&mut term, &mut app);
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    assert!(rest(&mut app, &mut term, (vx, vy)));
    idle(&mut app).await;
    assert!(fake.requests("textDocument/hover").is_empty(), "not asked");
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    assert!(app.message.is_none());

    // The command, the cursor right after `value`.
    let after_value = TEXT.find("value;").unwrap() + "value".len();
    put_cursor(&mut app, after_value);
    show_hover(&mut app);
    assert!(app.hover.popup.as_ref().is_some_and(|p| !p.mouse));
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    assert!(app.message.is_none());
    put_cursor(&mut app, TEXT.find("// two").unwrap());
    show_hover(&mut app);
    assert!(app.hover.popup.is_none());
    assert_eq!(app.message.as_ref().unwrap().text, "No hover information");
    app.shutdown_servers().await;

    // A server answering null: the diagnostics stay, no message; to the command as well.
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = fixes_app(dir.path(), false).await;
    fake.respond("textDocument/hover", Value::Null);
    draw(&mut term, &mut app);
    assert!(rest(&mut app, &mut term, (vx, vy)));
    assert!(app.settle(|_| fake.requests("textDocument/hover").len() == 1).await);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await, "answered");
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    put_cursor(&mut app, after_value);
    show_hover(&mut app);
    assert!(app.settle(|_| fake.requests("textDocument/hover").len() == 2).await);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await, "answered");
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    assert!(app.message.is_none(), "{:?}", app.message.as_ref().map(|m| &m.text));
    app.shutdown_servers().await;
}

/// A squiggle on whitespace, an empty range at a line's end (drawn under its last char) and one
/// on an empty line (the cell past its end) are hovered; plain whitespace, the cell past a
/// line's end and past an empty line's first cell aren't.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_hover_on_whitespace_and_line_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = fixes_app(dir.path(), false).await;
    fake.respond("textDocument/hover", Value::Null);
    let mut term = terminal();
    draw(&mut term, &mut app);

    // `int y  = 2;`: the two spaces are squiggled, the one after `int` isn't.
    let at = cell(&app, 6, 5);
    assert!(rest(&mut app, &mut term, at));
    assert_eq!(rows(&mut term, &mut app), ["ⓘ extra whitespace", "lint"]);
    let (sx, sy) = cell(&app, 6, 6);
    app.mouse(E::Moved, sx, sy);
    assert!(app.hover.popup.is_some(), "the second space: still on it");
    let at = cell(&app, 6, 3);
    assert!(!rest(&mut app, &mut term, at), "plain whitespace");
    assert!(app.hover.popup.is_none(), "moved off");

    // `int z = 3`: an empty range at its end, drawn under the `3`.
    let at = cell(&app, 7, 8);
    assert!(rest(&mut app, &mut term, at));
    assert_eq!(
        rows(&mut term, &mut app),
        ["⊗ expected ';' after top level declarator", "clang"]
    );
    let at = cell(&app, 7, 9);
    assert!(!rest(&mut app, &mut term, at), "past the line's end");
    assert!(app.hover.popup.is_none());

    // The empty line: its first cell, not the next.
    let at = cell(&app, 8, 0);
    assert!(rest(&mut app, &mut term, at));
    assert_eq!(rows(&mut term, &mut app), ["ⓘ empty line here"]);
    let at = cell(&app, 8, 1);
    assert!(!rest(&mut app, &mut term, at));
    assert!(app.hover.popup.is_none());
    // Nor the empty line's first cell without a squiggle (none after the text either).
    let at = cell(&app, 10, 0);
    assert!(!rest(&mut app, &mut term, at));

    // The command at the end of `int z = 3` finds the empty range there.
    put_cursor(&mut app, TEXT.find("3\n").unwrap() + 1);
    show_hover(&mut app);
    assert_eq!(
        rows(&mut term, &mut app),
        ["⊗ expected ';' after top level declarator", "clang"]
    );
    app.shutdown_servers().await;
}

/// Resting on `x` shows the warning over `x = value`; moving along it to `value` keeps the
/// popup, moving off it closes it. An edit closes it; the marks follow the edit, and resting on
/// the new place finds them there.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_hover_follows_the_squiggle_and_edits() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = fixes_app(dir.path(), false).await;
    fake.respond("textDocument/hover", Value::Null);
    let mut term = terminal();
    draw(&mut term, &mut app);
    let x = cell(&app, 5, 4);
    assert!(rest(&mut app, &mut term, x));
    assert_eq!(
        rows(&mut term, &mut app),
        ["⚠ unused variable 'x'", "clang(-Wunused-variable)"]
    );
    let popup = app.hover.popup.clone().unwrap();
    let line5 = TEXT.find("int x").unwrap();
    assert_eq!(
        (popup.range.0.0, popup.range.1.0),
        (line5 + 4, line5 + 13),
        "the squiggle"
    );
    assert_eq!(popup.anchor.0, line5 + 4, "anchored at the word");
    for col in [6, 8, 12] {
        let (cx, cy) = cell(&app, 5, col);
        app.mouse(E::Moved, cx, cy);
        assert!(app.hover.popup.is_some(), "col {col}: along the squiggle");
    }
    let (cx, cy) = cell(&app, 5, 1);
    app.mouse(E::Moved, cx, cy);
    assert!(app.hover.popup.is_none(), "off it");

    // An edit closes it.
    assert!(rest(&mut app, &mut term, x));
    assert!(app.hover.popup.is_some());
    // A paste, not a key: a key press closes the popup by itself.
    put_cursor(&mut app, line5);
    app.handle_test_event(crossterm::event::Event::Paste("ab".into()));
    assert!(app.hover.popup.is_none(), "the edit closed it");
    app.pump();

    // `abint x = value;`: `value` moved two columns on, and its error with it.
    draw(&mut term, &mut app);
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    assert_eq!(cell(&app, 5, 10), (vx, vy));
    assert!(rest(&mut app, &mut term, (vx + 4, vy)));
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    // Where `value` started: now the `=`, under the warning only.
    let (cx, cy) = cell(&app, 4, 1);
    app.mouse(E::Moved, cx, cy);
    assert!(app.hover.popup.is_none(), "off it");
    let at = cell(&app, 5, 8);
    assert!(rest(&mut app, &mut term, at));
    assert_eq!(
        rows(&mut term, &mut app),
        ["⚠ unused variable 'x'", "clang(-Wunused-variable)"]
    );
    app.shutdown_servers().await;
}

/// A failed hover answer (not a routine error) leaves the diagnostics, without a message; to
/// the command as well. Off any squiggle, the failed command says nothing either (as before).
#[tokio::test(flavor = "multi_thread")]
async fn fixes_hover_failed_answer_keeps_the_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = fixes_app(dir.path(), false).await;
    fake.fail("textDocument/hover", -32603, "internal error");
    let mut term = terminal();
    draw(&mut term, &mut app);
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    assert!(rest(&mut app, &mut term, (vx, vy)));
    assert!(app.settle(|_| fake.requests("textDocument/hover").len() == 1).await);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await, "answered");
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    assert!(app.message.is_none());

    put_cursor(&mut app, TEXT.find("value;").unwrap() + 2);
    show_hover(&mut app);
    assert!(app.settle(|_| fake.requests("textDocument/hover").len() == 2).await);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await, "answered");
    assert!(app.hover.popup.as_ref().is_some_and(|p| !p.mouse));
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    assert!(app.message.is_none());
    app.shutdown_servers().await;
}
