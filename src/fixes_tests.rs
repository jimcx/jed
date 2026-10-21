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
//!
//! fixes-b, code actions: Cmd+. (Ctrl+. on Linux) asks for the cursor's range with that server's
//! own diagnostics there (as published, at their place now; another server's left out) and lists
//! the answer at the cursor: quick fixes first, the preferred marked and selected, disabled ones
//! greyed with their reason (Enter says why); Esc, the cursor moving or a click elsewhere close
//! it; an edit while asking drops the answer; no `codeActionProvider` says so. Enter applies an
//! edit as one undo step, resolves an action without one first (`codeAction/resolve`), runs a
//! command (`workspace/executeCommand`) whose server-sent `workspace/applyEdit` edits the buffer
//! and is answered `applied` (or not, with why). The lightbulb `💡` shows in the gutter on the
//! cursor line after a rest when actions exist, cached by revision, and a click on it opens the
//! list. The hover's diagnostics get a "Quick Fix…" row when the server has quick fixes there;
//! clicking it opens the list of those fixes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseEventKind as E};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};

use crate::app::App;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::milestone9_tests::{
    buffer_id, diag, draw, find_cells, idle, open_app, open_synced, popup_rows, project, publish, put_cursor,
    show_hover, uri,
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

// ---- fixes-b: code actions ----

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// Cmd+. (Ctrl+. on Linux).
fn quick_fix(app: &mut App) {
    app.press(K::Char('.'), cmd());
}

/// Where `needle` starts in `TEXT`.
fn at(needle: &str) -> usize {
    TEXT.find(needle).unwrap()
}

/// An LSP range on one line.
fn range(line: u32, from: u32, to: u32) -> Value {
    json!({ "start": { "line": line, "character": from }, "end": { "line": line, "character": to } })
}

/// A `WorkspaceEdit` of `path`: (line, from, to, new text) each.
fn workspace_edit(path: &Path, edits: &[(u32, u32, u32, &str)]) -> Value {
    let edits: Vec<Value> = edits
        .iter()
        .map(|&(line, from, to, text)| json!({ "range": range(line, from, to), "newText": text }))
        .collect();
    json!({ "changes": { uri(path): edits } })
}

/// `a.cpp` open and synced with a fake server (`options`), line 5's two diagnostics published
/// (a warning over `x = value`; an error on `value`, with `data`).
async fn actions_app(dir: &Path, options: FakeOptions) -> (App, Arc<FakeLauncher>, PathBuf) {
    let root = project(dir, &[("a.cpp", TEXT)]);
    let fake = FakeLauncher::new(FakeOptions { utf8: true, ..options });
    let mut app = open_app(&root, fake.clone());
    let path = root.join("a.cpp");
    open_synced(&mut app, &fake, &path).await;
    let mut warning = diag(5, 4, 13, 2, "unused variable 'x'");
    warning["source"] = json!("clang");
    warning["code"] = json!("-Wunused-variable");
    let mut error = diag(5, 8, 13, 1, UNDECLARED);
    error["source"] = json!("clang");
    error["code"] = json!("undeclared_var_use");
    error["data"] = json!({ "fix": 7 });
    publish(&app, Language::Cpp, &path, None, json!([warning, error]));
    assert!(
        app.settle(|a| a.active_buffer().unwrap().diagnostics.all().len() == 2)
            .await
    );
    (app, fake, path)
}

/// The `textDocument/codeAction` requests Cmd+. made (invoked, not the hover's quick fixes).
fn list_requests(fake: &FakeLauncher) -> Vec<Value> {
    fake.requests("textDocument/codeAction")
        .into_iter()
        .filter(|p| p["context"]["triggerKind"] == 1 && p["context"].get("only").is_none())
        .collect()
}

/// The lightbulb's requests (automatic).
fn bulb_requests(fake: &FakeLauncher) -> Vec<Value> {
    fake.requests("textDocument/codeAction")
        .into_iter()
        .filter(|p| p["context"]["triggerKind"] == 2)
        .collect()
}

/// The list's rows as drawn (trimmed).
fn list_rows(term: &mut Terminal<TestBackend>, app: &mut App) -> Vec<String> {
    draw(term, app);
    let r = app.hits.code_actions.expect("list drawn").rect;
    let buf = term.backend().buffer();
    (r.y + 1..r.y + r.height - 1)
        .map(|y| {
            (r.x + 1..r.x + r.width - 1)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

fn text_of(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

/// Cmd+. on `value`: the request carries the cursor's range and the server's own diagnostics
/// there (code, data and range as published); the list shows quick fixes first (the preferred
/// marked and selected), then refactorings (a disabled one greyed with its reason), then a bare
/// command. Enter on the disabled one only says why; Esc closes; moving the cursor closes it.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_code_actions_list() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _path) = actions_app(dir.path(), FakeOptions::default()).await;
    fake.respond(
        "textDocument/codeAction",
        json!([
            { "title": "Extract variable", "kind": "refactor.extract" },
            { "title": "Inline", "kind": "refactor.inline", "disabled": { "reason": "Nothing to inline" } },
            { "title": "Declare 'value'", "kind": "quickfix", "edit": { "changes": {} } },
            { "title": "Change to 'valve'", "kind": "quickfix", "isPreferred": true, "edit": { "changes": {} } },
            { "title": "Run lint", "command": "fake.lint" },
        ]),
    );
    let mut term = terminal();
    put_cursor(&mut app, at("value;") + 2);
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    let asked = list_requests(&fake);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["range"], range(5, 10, 10), "the cursor");
    let diags = asked[0]["context"]["diagnostics"].as_array().unwrap().clone();
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0]["code"], "undeclared_var_use", "most severe first");
    assert_eq!(diags[0]["data"], json!({ "fix": 7 }), "as published");
    assert_eq!(diags[0]["range"], range(5, 8, 13));
    assert_eq!(diags[1]["code"], "-Wunused-variable");
    assert_eq!(diags[1]["range"], range(5, 4, 13));

    assert_eq!(
        list_rows(&mut term, &mut app),
        [
            "★ Change to 'valve'",
            "Declare 'value'",
            "Extract variable",
            "Inline  (Nothing to inline)",
            "Run lint"
        ]
    );
    let r = app.hits.code_actions.unwrap().rect;
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    assert!(r.y > vy, "below the cursor's line");
    assert!(r.x < vx + 2 && r.x + r.width > vx + 2);
    let fg = |key: &str| app.theme.get(key).fg;
    let buf = term.backend().buffer();
    let (ix, iy) = find_cells(&term, "Inline").unwrap();
    assert_eq!(Some(buf[(ix, iy)].fg), fg("editorActionList.disabledForeground"));
    let (px, py) = find_cells(&term, "★").unwrap();
    assert_eq!(Some(buf[(px, py)].fg), fg("editorActionList.preferred"));
    assert_eq!(app.code_actions.popup.as_ref().unwrap().selected, 0, "the preferred");

    for _ in 0..3 {
        app.press(K::Down, M::NONE);
    }
    app.press(K::Enter, M::NONE);
    assert_eq!(app.message.as_ref().unwrap().text, "Nothing to inline");
    assert!(app.code_actions.popup.is_some(), "still open");
    assert!(app.settle(|a| a.pending_requests().is_empty()).await);
    assert_eq!(text_of(&app), TEXT, "nothing applied");
    app.press(K::Esc, M::NONE);
    assert!(app.code_actions.popup.is_none(), "Esc closes it");
    assert!(app.active_buffer().unwrap().diagnostics.all().len() == 2);

    // Moving the cursor closes it; so does a click elsewhere.
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    app.press(K::Right, M::NONE);
    assert!(app.code_actions.popup.is_none(), "the cursor moved");
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    draw(&mut term, &mut app);
    let (cx, cy) = cell(&app, 1, 2);
    app.mouse(E::Down(crossterm::event::MouseButton::Left), cx, cy);
    assert!(app.code_actions.popup.is_none(), "a click elsewhere");

    // Nothing there: VS Code's message.
    fake.respond("textDocument/codeAction", json!([]));
    quick_fix(&mut app);
    assert!(app.settle(|a| a.message.is_some()).await);
    assert_eq!(app.message.as_ref().unwrap().text, "No code actions available");
    assert!(app.code_actions.popup.is_none());
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;
}

/// Enter applies the selected action's edit (two edits in the file) as one undo step; the
/// server's copy follows.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_code_action_edit_is_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = actions_app(dir.path(), FakeOptions::default()).await;
    let edit = workspace_edit(&path, &[(5, 8, 13, "valve"), (0, 0, 0, "#include <v>\n")]);
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Change to 'valve'", "kind": "quickfix", "isPreferred": true, "edit": edit }]),
    );
    put_cursor(&mut app, at("value;"));
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    app.press(K::Enter, M::NONE);
    assert!(app.code_actions.popup.is_none());
    let fixed = format!("#include <v>\n{}", TEXT.replace("value;", "valve;"));
    assert_eq!(text_of(&app), fixed);
    assert!(fake.requests("codeAction/resolve").is_empty(), "it had its edit");
    let u = uri(&path);
    assert!(
        app.settle(|a| fake.document(&u).is_some_and(|d| d.text == text_of(a)))
            .await
    );
    app.press(K::Char('z'), cmd());
    assert_eq!(text_of(&app), TEXT, "one undo step");
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;
}

/// An action without an edit is resolved first (its JSON, `data` included, as the params); the
/// resolved edit is applied. One whose resolve brings only a command runs it
/// (`workspace/executeCommand`); the server's `workspace/applyEdit` then edits the buffer and
/// is answered `applied: true`; an edit that can't apply is answered `applied: false` with why.
/// A bare command runs without a resolve.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_code_action_resolve_command_and_apply_edit() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = actions_app(dir.path(), FakeOptions::default()).await;
    let lazy = json!({ "title": "Declare 'value'", "kind": "quickfix", "data": { "id": 3 } });
    fake.respond("textDocument/codeAction", json!([lazy]));
    let mut resolved = lazy.clone();
    resolved["edit"] = workspace_edit(&path, &[(5, 0, 0, "int value;\n")]);
    fake.respond("codeAction/resolve", resolved);
    put_cursor(&mut app, at("value;"));
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| text_of(a).contains("int value;\nint x")).await);
    assert_eq!(fake.requests("codeAction/resolve"), [lazy]);
    app.press(K::Char('z'), cmd());
    assert_eq!(text_of(&app), TEXT);

    // Resolved to a command: it runs, and the server's edit comes back as applyEdit.
    let fix = json!({
        "title": "Fix all", "kind": "quickfix",
        "command": { "title": "Fix all", "command": "fake.fixAll", "arguments": [{ "uri": uri(&path) }] },
    });
    fake.respond("textDocument/codeAction", json!([fix.clone()]));
    fake.respond("codeAction/resolve", fix);
    let apply = json!({ "label": "Fix all", "edit": workspace_edit(&path, &[(5, 8, 13, "valve")]) });
    fake.apply_edit_on("workspace/executeCommand", apply);
    fake.respond("workspace/executeCommand", Value::Null);
    put_cursor(&mut app, at("value;"));
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| text_of(a).contains("valve;")).await);
    assert_eq!(
        fake.requests("workspace/executeCommand"),
        [json!({ "command": "fake.fixAll", "arguments": [{ "uri": uri(&path) }] })]
    );
    assert!(app.settle(|_| fake.answers().contains_key("apply1")).await);
    assert_eq!(fake.answers()["apply1"], json!({ "applied": true }));
    assert_eq!(fake.requests("codeAction/resolve").len(), 2);

    // A bare command: no resolve; its applyEdit overlaps itself and isn't applied.
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Lint", "command": "fake.lint" }]),
    );
    let bad = workspace_edit(&path, &[(5, 0, 5, "a"), (5, 2, 8, "b")]);
    fake.apply_edit_on("workspace/executeCommand", json!({ "edit": bad }));
    let valve = text_of(&app).find("valve;").unwrap();
    put_cursor(&mut app, valve);
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    let before = text_of(&app);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|_| fake.answers().contains_key("apply2")).await);
    let answer = fake.answers()["apply2"].clone();
    assert_eq!(answer["applied"], false);
    assert_eq!(answer["failureReason"], "a.cpp: overlapping edits");
    assert_eq!(text_of(&app), before);
    assert_eq!(
        fake.requests("codeAction/resolve").len(),
        2,
        "a bare command isn't resolved"
    );
    assert_eq!(
        fake.requests("workspace/executeCommand")[1],
        json!({ "command": "fake.lint" })
    );
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Edit not applied: a.cpp: overlapping edits"
    );
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;
}

/// The lightbulb: after the cursor rests, an automatic request; `💡` in the gutter on the
/// cursor line while that place has actions. Answers are cached by revision (back to a place
/// asked about: no request); an edit asks again; a place without actions shows none. A click on
/// it opens the list.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_lightbulb() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _path) = actions_app(dir.path(), FakeOptions::default()).await;
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Change to 'valve'", "kind": "quickfix" }]),
    );
    let mut term = terminal();
    put_cursor(&mut app, at("value;") + 1);
    app.press(K::Right, M::NONE);
    assert!(app.bulb_line().is_none(), "not before the rest");
    assert!(app.settle(|a| a.bulb_line().is_some()).await);
    assert_eq!(app.bulb_line().unwrap().1, 5);
    let asked = bulb_requests(&fake);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["range"], range(5, 10, 10));
    assert_eq!(asked[0]["context"]["diagnostics"].as_array().unwrap().len(), 2);
    draw(&mut term, &mut app);
    let (bx, by) = find_cells(&term, "💡").expect("drawn");
    let (_, line5) = cell(&app, 5, 0);
    assert_eq!(by, line5, "on the cursor line");
    assert!(bx < app.hits.group(app.groups.active).unwrap().text.x, "in the gutter");

    // Line 6 has none; back on line 5 the cached answer shows it again without asking.
    fake.respond("textDocument/codeAction", json!([]));
    app.press(K::Down, M::NONE);
    assert!(app.bulb_line().is_none());
    assert!(app.settle(|_| bulb_requests(&fake).len() == 2).await);
    assert!(app.settle(|a| a.code_actions.asking.is_none()).await);
    draw(&mut term, &mut app);
    assert!(find_cells(&term, "💡").is_none());
    app.press(K::Up, M::NONE);
    assert_eq!(app.bulb_line().map(|b| b.1), Some(5), "cached");
    // Past a whole rest, with the timers firing (`settle`): still no new request.
    let until = std::time::Instant::now() + crate::code_action::BULB_DELAY * 2;
    assert!(app.settle(|_| std::time::Instant::now() >= until).await);
    assert_eq!(bulb_requests(&fake).len(), 2, "not asked again");
    draw(&mut term, &mut app);
    let (_, by) = find_cells(&term, "💡").expect("drawn from the cache");
    assert_eq!(by, line5);

    // An edit: a new revision, asked again (now none there).
    app.type_text("q");
    assert!(app.bulb_line().is_none(), "hidden at once");
    assert!(app.settle(|_| bulb_requests(&fake).len() == 3).await);
    assert!(app.settle(|a| a.code_actions.asking.is_none()).await);
    assert!(app.bulb_line().is_none());

    // Actions again (after a publish, which forgets the cache): a click on the bulb opens the list.
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Change to 'valve'", "kind": "quickfix" }]),
    );
    app.press(K::Backspace, M::NONE);
    assert!(app.settle(|a| a.bulb_line().is_some()).await);
    draw(&mut term, &mut app);
    let (bx, by) = find_cells(&term, "💡").unwrap();
    app.mouse(E::Down(crossterm::event::MouseButton::Left), bx, by);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    assert_eq!(list_rows(&mut term, &mut app), ["Change to 'valve'"]);
    app.shutdown_servers().await;
}

/// The hover's diagnostics ask for their quick fixes (`only: quickfix`, the diagnostics shown);
/// with some, a "Quick Fix…" row ends the diagnostics section (before the server's hover);
/// clicking it opens the list of those fixes (not the refactoring) at the diagnostic, and
/// Enter applies one. Without fixes there's no row.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_hover_quick_fix_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = actions_app(dir.path(), FakeOptions::default()).await;
    fake.respond("textDocument/hover", json!({ "contents": "about `value`" }));
    fake.hold("textDocument/hover", true);
    let edit = workspace_edit(&path, &[(5, 8, 13, "valve")]);
    fake.respond(
        "textDocument/codeAction",
        json!([
            { "title": "Extract", "kind": "refactor.extract" },
            { "title": "Change to 'valve'", "kind": "quickfix", "edit": edit },
        ]),
    );
    let mut term = terminal();
    draw(&mut term, &mut app);
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    assert!(rest(&mut app, &mut term, (vx + 1, vy)));
    let mut want = diagnostics_rows();
    want.push("Quick Fix…");
    assert!(
        app.settle(|a| a.hover.popup.as_ref().is_some_and(|p| p.lines.len() == want.len()))
            .await
    );
    assert_eq!(rows(&mut term, &mut app), want);
    let asked: Vec<Value> = fake
        .requests("textDocument/codeAction")
        .into_iter()
        .filter(|p| p["context"]["only"] == json!(["quickfix"]))
        .collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["range"], range(5, 4, 13), "the diagnostics shown");
    assert_eq!(asked[0]["context"]["diagnostics"].as_array().unwrap().len(), 2);
    let (lx, ly) = find_cells(&term, "Quick").unwrap();
    assert_eq!(
        Some(term.backend().buffer()[(lx, ly)].fg),
        app.theme.get("textLink.foreground").fg
    );

    // The server's hover after it, the row kept.
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(
        app.settle(|a| a.hover.popup.as_ref().is_some_and(|p| p.lines.len() > want.len()))
            .await
    );
    let mut with_answer = want.clone();
    with_answer.extend(["---", "about value"]);
    assert_eq!(rows(&mut term, &mut app), with_answer);

    // A click on it: the list of the fixes, at the diagnostics.
    draw(&mut term, &mut app);
    let (lx, ly) = find_cells(&term, "Quick").unwrap();
    app.mouse(E::Down(crossterm::event::MouseButton::Left), lx, ly);
    assert!(app.hover.popup.is_none());
    assert_eq!(list_rows(&mut term, &mut app), ["Change to 'valve'"]);
    assert_eq!(app.code_actions.popup.as_ref().unwrap().anchor.0, at("x = value"));
    app.press(K::Enter, M::NONE);
    assert_eq!(text_of(&app), TEXT.replace("value;", "valve;"));
    app.shutdown_servers().await;

    // No quick fixes: no row.
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _path) = actions_app(dir.path(), FakeOptions::default()).await;
    fake.respond("textDocument/hover", Value::Null);
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Extract", "kind": "refactor.extract" }]),
    );
    draw(&mut term, &mut app);
    assert!(rest(&mut app, &mut term, (vx + 1, vy)));
    assert!(app.settle(|a| a.pending_requests().is_empty()).await);
    assert_eq!(rows(&mut term, &mut app), diagnostics_rows());
    app.shutdown_servers().await;
}

/// Only the buffer's server's diagnostics go to it, at their place now (after an edit before
/// them): another server's on the same file stay out.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_code_actions_send_only_that_servers_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = actions_app(dir.path(), FakeOptions::default()).await;
    let py = path.with_file_name("b.py");
    std::fs::write(&py, "x = 1\n").unwrap();
    app.test_open(crate::buffer::Buffer::open(&py).unwrap());
    assert!(
        app.settle(|a| a
            .lsp
            .server_for(Language::Python)
            .is_some_and(|s| s.encoding().is_some()))
            .await
    );
    // ty's diagnostic on a.cpp.
    let mut other = diag(5, 8, 13, 1, "ty says so");
    other["source"] = json!("ty");
    publish(&app, Language::Python, &path, None, json!([other]));
    let a = buffer_id(&app, &path);
    assert!(
        app.settle(|app| app.buffers.get(a).unwrap().diagnostics.all().len() == 3)
            .await
    );
    app.press(K::Char('w'), cmd());
    assert!(
        app.settle(|app| app.active_buffer().is_some_and(|b| b.path == path))
            .await
    );

    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Fix", "kind": "quickfix" }]),
    );
    put_cursor(&mut app, at("int x"));
    app.handle_test_event(crossterm::event::Event::Paste("ab".into()));
    put_cursor(&mut app, at("value;") + 4);
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    let asked = list_requests(&fake);
    assert_eq!(asked.len(), 1);
    let diags = asked[0]["context"]["diagnostics"].as_array().unwrap().clone();
    let sources: Vec<&str> = diags.iter().map(|d| d["source"].as_str().unwrap()).collect();
    assert_eq!(sources, ["clang", "clang"], "not ty's");
    assert_eq!(diags[0]["range"], range(5, 10, 15), "moved by the paste");
    app.shutdown_servers().await;
}

/// No `codeActionProvider`: Cmd+. says so and asks nothing; no lightbulb rest starts.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_code_actions_without_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let options = FakeOptions {
        no_code_actions: true,
        ..Default::default()
    };
    let (mut app, fake, _path) = actions_app(dir.path(), options).await;
    put_cursor(&mut app, at("value;"));
    app.press(K::Right, M::NONE);
    assert!(app.code_actions.rest.is_none(), "no lightbulb");
    quick_fix(&mut app);
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "No language server offers code actions here"
    );
    idle(&mut app).await;
    assert!(fake.requests("textDocument/codeAction").is_empty());
    assert!(app.code_actions.popup.is_none());
    app.shutdown_servers().await;
}

/// An edit while the answer is out cancels the request; its answer (when the server sends one
/// anyway) is dropped: no list.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_code_actions_stale_answer_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _path) = actions_app(dir.path(), FakeOptions::default()).await;
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Fix", "kind": "quickfix" }]),
    );
    fake.hold("textDocument/codeAction", true);
    put_cursor(&mut app, at("value;"));
    quick_fix(&mut app);
    assert!(app.settle(|_| list_requests(&fake).len() == 1).await);
    assert_eq!(
        app.pending_requests()
            .iter()
            .filter(|(k, _)| *k == "code actions")
            .count(),
        1
    );
    app.type_text("z");
    assert!(
        app.pending_requests().iter().all(|(k, _)| *k != "code actions"),
        "cancelled"
    );
    assert!(app.settle(|_| !fake.cancelled().is_empty()).await);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert!(app.code_actions.popup.is_none());
    app.shutdown_servers().await;
}

/// Moving off while the lightbulb's request is out cancels it; coming straight back (before
/// another rest) asks again, not waiting on the cancelled request forever.
#[tokio::test(flavor = "multi_thread")]
async fn fixes_lightbulb_asks_again_after_a_cancelled_request() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _path) = actions_app(dir.path(), FakeOptions::default()).await;
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Change to 'valve'", "kind": "quickfix" }]),
    );
    fake.hold("textDocument/codeAction", true);
    put_cursor(&mut app, at("value;") + 1);
    app.press(K::Right, M::NONE);
    assert!(app.settle(|_| bulb_requests(&fake).len() == 1).await);
    app.press(K::Right, M::NONE);
    app.press(K::Left, M::NONE);
    assert!(app.settle(|_| !fake.cancelled().is_empty()).await);
    fake.hold("textDocument/codeAction", false);
    assert!(app.settle(|a| a.bulb_line().is_some()).await, "asked again");
    let asked = bulb_requests(&fake);
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1]["range"], range(5, 10, 10));
    app.shutdown_servers().await;
}

/// A code action's edit over two files, one not open: the open one is edited at once (one undo
/// step), the other loaded into a tab and edited there (its file on disk untouched).
#[tokio::test(flavor = "multi_thread")]
async fn fixes_code_action_edits_a_file_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = actions_app(dir.path(), FakeOptions::default()).await;
    let other = path.with_file_name("b.cpp");
    std::fs::write(&other, "int value = 0;\n").unwrap();
    let mut edit = workspace_edit(&path, &[(5, 8, 13, "valve")]);
    edit["changes"][uri(&other)] = json!([{ "range": range(0, 4, 9), "newText": "valve" }]);
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Rename everywhere", "kind": "quickfix", "edit": edit }]),
    );
    put_cursor(&mut app, at("value;"));
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    app.press(K::Enter, M::NONE);
    let a = buffer_id(&app, &path);
    assert_eq!(
        app.buffers.get(a).unwrap().rope.to_string(),
        TEXT.replace("value;", "valve;")
    );
    assert!(
        app.settle(|app| app
            .buffers
            .find_path(&other)
            .is_some_and(|b| app.buffers.get(b).unwrap().rope == "int valve = 0;\n"))
            .await
    );
    assert_eq!(
        std::fs::read_to_string(&other).unwrap(),
        "int value = 0;\n",
        "not saved"
    );
    // The edited buffer's undo is one step.
    let tab = app.group().tabs.iter().position(|t| t.buffer() == Some(a)).unwrap();
    app.group_mut().active = tab;
    app.press(K::Char('z'), cmd());
    assert_eq!(app.buffers.get(a).unwrap().rope.to_string(), TEXT);
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;
}

/// A server's `workspace/applyEdit` while the list is open edits the buffer, is answered
/// `applied: true` and closes the list (its actions were for the text before).
#[tokio::test(flavor = "multi_thread")]
async fn fixes_apply_edit_while_the_list_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = actions_app(dir.path(), FakeOptions::default()).await;
    let fix = workspace_edit(&path, &[(5, 8, 13, "valve")]);
    fake.respond(
        "textDocument/codeAction",
        json!([{ "title": "Change to 'valve'", "kind": "quickfix", "edit": fix }]),
    );
    put_cursor(&mut app, at("value;"));
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    let server = app.lsp.server_for(Language::Cpp).unwrap().id;
    app.test_lsp_event(crate::lsp::Event::ServerRequest {
        server,
        id: crate::lsp::rpc::Id::Str("pushed".into()),
        method: "workspace/applyEdit".into(),
        // Elsewhere: the list's fix would still apply cleanly if it weren't closed.
        params: json!({ "edit": workspace_edit(&path, &[(9, 8, 9, "5")]) }),
    });
    assert!(app.code_actions.popup.is_none(), "closed by the edit");
    assert_eq!(text_of(&app), TEXT.replace("w = 4", "w = 5"));
    assert!(app.settle(|_| fake.answers().contains_key("pushed")).await);
    assert_eq!(fake.answers()["pushed"], json!({ "applied": true }));
    // Enter now is the editor's: a new line, not the stale fix.
    app.press(K::Enter, M::NONE);
    assert!(!text_of(&app).contains("valve"));
    app.shutdown_servers().await;
}

/// An edit while `codeAction/resolve` is out cancels it; its answer, if it comes, isn't applied
/// (its positions are for the text before).
#[tokio::test(flavor = "multi_thread")]
async fn fixes_resolve_answer_after_an_edit_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = actions_app(dir.path(), FakeOptions::default()).await;
    let lazy = json!({ "title": "Declare 'value'", "kind": "quickfix", "data": { "id": 3 } });
    fake.respond("textDocument/codeAction", json!([lazy]));
    let mut resolved = lazy.clone();
    resolved["edit"] = workspace_edit(&path, &[(5, 0, 0, "int value;\n")]);
    fake.respond("codeAction/resolve", resolved);
    fake.hold("codeAction/resolve", true);
    put_cursor(&mut app, at("value;"));
    quick_fix(&mut app);
    assert!(app.settle(|a| a.code_actions.popup.is_some()).await);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|_| fake.requests("codeAction/resolve").len() == 1).await);
    app.type_text("q");
    assert!(app.settle(|_| !fake.cancelled().is_empty()).await);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert_eq!(text_of(&app), TEXT.replace("value;", "qvalue;"), "only the typing");
    app.shutdown_servers().await;
}
