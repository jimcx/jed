//! End-to-end tests for the LSP semantic tokens task, subtask a (the client side, no drawing),
//! against the fake server: the capability and legend are read at initialize; a file's first
//! `textDocument/semanticTokens/full` goes out at once after `didOpen`; the next request after an
//! edit (about 150 ms later) is `full/delta` with the last `resultId`, and its edits are applied
//! to the server's previous array; an answer for an older version is still taken, its tokens
//! mapped through the edits made since; `workspace/semanticTokens/refresh` asks again; a server
//! that goes away takes its tokens with it, and a language change (the document reopened, here
//! on the same server) asks for a whole new set. A diagnostics publish for a newer version
//! doesn't lose the version a semantic tokens request is out for. Decode, delta and mapping units, and the 60k-token
//! budget, are in `semantic::tests`; the client capabilities in `lsp::server::tests`.
//!
//! Subtask b (drawing): drawn cells take a token's theme colour over tree-sitter's (only its own
//! cells, still after typing before it with no new answer), a token with no theme key keeps
//! tree-sitter's; `semantic_highlighting = off` draws pure tree-sitter, asks for nothing and drops
//! the tokens; clangd's `inactiveRegions` (opted into at initialize) dim the foreground, over a
//! selection too (its background kept). `semantic_tokens_done_when_clangd` checks the done-when
//! against a real clangd (skipped without one). The style table's units are in `ui::semantic`.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use crossterm::event::{KeyCode, KeyModifiers as M};

use crate::app::App;
use crate::command::Command;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::milestone9_tests::{open_app, open_synced, project, publish};
use crate::semantic::{Kind, Token};

const FULL: &str = "textDocument/semanticTokens/full";
const DELTA: &str = "textDocument/semanticTokens/full/delta";
const TEXT: &str = "int foo;\nbar();\n";

async fn tokens_app(dir: &std::path::Path, scripted: Option<Value>) -> (App, Arc<FakeLauncher>, PathBuf) {
    let root = project(dir, &[("a.cpp", TEXT)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        semantic_tokens: true,
        ..Default::default()
    });
    if let Some(v) = scripted {
        fake.respond(FULL, v);
    }
    let mut app = open_app(&root, fake.clone());
    let path = root.join("a.cpp");
    open_synced(&mut app, &fake, &path).await;
    (app, fake, path)
}

fn tokens(app: &App) -> Vec<(usize, usize, Kind, u32)> {
    app.active_buffer()
        .unwrap()
        .semantic
        .tokens()
        .iter()
        .map(|t: &Token| (t.start.0, t.end.0, t.kind, t.mods))
        .collect()
}

fn full_answer(id: &str) -> Value {
    // foo (variable, declaration) on line 0; bar (function) on line 1.
    json!({ "resultId": id, "data": [0, 4, 3, 0, 1, 1, 0, 3, 2, 0] })
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_full_request_after_open_then_delta_after_an_edit() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _) = tokens_app(dir.path(), Some(full_answer("1"))).await;
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 2)
            .await
    );
    assert_eq!(
        tokens(&app),
        vec![(4, 7, Kind::Variable, 1), (9, 12, Kind::Function, 0)]
    );
    assert_eq!(fake.requests(FULL).len(), 1);
    assert!(fake.requests(DELTA).is_empty());

    // Edit at the end: the next request is a delta against result "1" which turns `bar` into a class.
    fake.respond(
        DELTA,
        json!({ "resultId": "2", "edits": [{ "start": 5, "deleteCount": 5, "data": [1, 0, 3, 1, 0] }] }),
    );
    app.press_file_end();
    app.type_text("// c");
    assert!(app.settle(|_| !fake.requests(DELTA).is_empty()).await);
    assert_eq!(fake.requests(FULL).len(), 1, "no second full request");
    assert_eq!(fake.requests(DELTA)[0]["previousResultId"], json!("1"));
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().get(1).map(|t| t.kind) == Some(Kind::Class))
            .await
    );
    assert_eq!(tokens(&app)[0], (4, 7, Kind::Variable, 1));
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_a_stale_answer_still_applies_mapped_through_the_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", TEXT)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        semantic_tokens: true,
        ..Default::default()
    });
    fake.respond(FULL, full_answer("1"));
    fake.hold(FULL, true);
    let mut app = open_app(&root, fake.clone());
    let path = root.join("a.cpp");
    open_synced(&mut app, &fake, &path).await;
    assert!(app.settle(|_| !fake.requests(FULL).is_empty()).await);

    // Typed before the answer: the answer is for version 1.
    app.press_file_start();
    app.type_text("ab");
    assert!(
        app.settle(|a| a
            .buffers
            .iter()
            .next()
            .is_some_and(|(id, _)| a.lsp.version(id) == Some(2)))
            .await
    );
    // Diagnostics for version 2 come first (clangd publishes as it likes): version 1, which the
    // held request is about, must still be kept for its answer.
    publish(&app, crate::syntax::Language::Cpp, &path, Some(2), json!([]));
    fake.hold(FULL, false);
    app.lsp
        .server_for(crate::syntax::Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 2)
            .await
    );
    assert_eq!(
        tokens(&app),
        vec![(6, 9, Kind::Variable, 1), (11, 14, Kind::Function, 0)]
    );
    assert_eq!(
        fake.requests(FULL).len(),
        1,
        "the held answer was used, not a new full request"
    );
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_a_language_change_asks_for_a_new_full_set() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _) = tokens_app(dir.path(), Some(full_answer("1"))).await;
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 2)
            .await
    );
    // C++ to C: clangd serves both, so the server stays but the document is reopened. Its old
    // `resultId` means nothing now: a full request, not a delta.
    fake.respond(FULL, json!({ "resultId": "c1", "data": [1, 0, 3, 1, 0] }));
    app.test_execute(Command::ChangeLanguageMode);
    app.type_text("C");
    let (want, at) = {
        let p = app.palette.as_ref().unwrap();
        (p.rows.iter().position(|r| r.label == "C").expect("a C row"), p.selected)
    };
    let key = if want < at { KeyCode::Up } else { KeyCode::Down };
    for _ in 0..want.abs_diff(at) {
        app.press(key, M::NONE);
    }
    {
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows[p.selected].label, "C");
    }
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|_| fake.requests(FULL).len() == 2).await);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 1)
            .await
    );
    assert_eq!(tokens(&app), vec![(9, 12, Kind::Class, 0)]);
    assert!(fake.requests(DELTA).is_empty());
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_refresh_asks_again_and_a_gone_server_clears() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _) = tokens_app(dir.path(), Some(full_answer("1"))).await;
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 2)
            .await
    );

    fake.respond(
        DELTA,
        json!({ "resultId": "2", "edits": [{ "start": 0, "deleteCount": 5, "data": [] }] }),
    );
    // The server's request: answered `null` by the server task, then the tokens asked again.
    app.lsp
        .server_for(crate::syntax::Language::Cpp)
        .unwrap()
        .notify("test/refresh", Value::Null);
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 1)
            .await
    );
    assert_eq!(fake.requests(DELTA).len(), 1);
    assert_eq!(fake.answers().get("refresh"), Some(&Value::Null));

    app.lsp
        .server_for(crate::syntax::Language::Cpp)
        .unwrap()
        .notify("test/crash", Value::Null);
    assert!(app.settle(|a| a.active_buffer().unwrap().semantic.is_empty()).await);
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_nothing_is_asked_without_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", TEXT)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    app.press_file_end();
    app.type_text("x");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(app.settle(|_| true).await);
    assert!(fake.requests(FULL).is_empty());
    assert!(app.active_buffer().unwrap().semantic.is_empty());
    app.shutdown_servers().await;
}

// Subtask b: drawing, the setting, inactive regions.

use ratatui::style::Color;

use crate::config::settings::SettingsOverrides;
use crate::edit::SemanticHighlighting;
use crate::milestone9_tests::{draw, find_cells, terminal, uri};

/// A frame's foreground at the first place `needle` is drawn.
fn fg_of(term: &ratatui::Terminal<ratatui::backend::TestBackend>, needle: &str) -> Option<Color> {
    let (x, y) = find_cells(term, needle).unwrap_or_else(|| panic!("{needle:?} drawn"));
    term.backend().buffer()[(x, y)].fg.into()
}

/// The foreground of each cell `needle` is drawn in.
fn fgs(term: &ratatui::Terminal<ratatui::backend::TestBackend>, needle: &str) -> Vec<Option<Color>> {
    (0..needle.chars().count() as i32)
        .map(|i| cell_fg(term, needle, i))
        .collect()
}

/// The foreground `dx` cells right of where `needle` is first drawn.
fn cell_fg(term: &ratatui::Terminal<ratatui::backend::TestBackend>, needle: &str, dx: i32) -> Option<Color> {
    let (x, y) = find_cells(term, needle).unwrap_or_else(|| panic!("{needle:?} drawn"));
    term.backend().buffer()[((x as i32 + dx) as u16, y)].fg.into()
}

/// `fg` as the editor dims it in compiled-out code.
fn dimmed(app: &App, fg: Option<Color>) -> Option<Color> {
    app.theme.dim(ratatui::style::Style::new().fg(fg.unwrap())).fg
}

fn fg_style(app: &App, key: &str) -> Option<Color> {
    app.theme.get(key).fg
}

/// `foo` is a constructor (class + constructorOrDestructor), `bar` a plain function.
fn ctor_answer() -> Value {
    json!({ "resultId": "1", "data": [0, 4, 3, 1, 2, 1, 0, 3, 2, 0] })
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_are_drawn_over_tree_sitter_and_off_returns_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _) = tokens_app(dir.path(), Some(ctor_answer())).await;
    assert!(
        app.settle(|a| {
            let b = a.active_buffer().unwrap();
            b.semantic.tokens().len() == 2 && b.syntax.tree().is_some()
        })
        .await
    );
    let mut term = terminal();
    // Tree-sitter's colours first, with the setting off (the tokens are still in the buffer).
    app.settings.semantic_highlighting = SemanticHighlighting::Off;
    draw(&mut term, &mut app);
    let foo_off = fg_of(&term, "foo");
    let function = fg_style(&app, "function");
    assert!(function.is_some());
    assert_eq!(fgs(&term, "bar"), vec![function; 3], "tree-sitter colours the call");
    app.settings.semantic_highlighting = SemanticHighlighting::On;
    draw(&mut term, &mut app);
    let ctor = fg_style(&app, "semantic.constructor");
    assert!(ctor.is_some());
    assert_ne!(foo_off, ctor);
    assert_eq!(fgs(&term, "foo"), vec![ctor; 3], "the constructor has its colour");
    assert_ne!(cell_fg(&term, "foo", -1), ctor, "only the token's cells");
    assert_ne!(cell_fg(&term, "foo;", 3), ctor, "only the token's cells");
    assert_eq!(
        fgs(&term, "bar"),
        vec![function; 3],
        "a function (no theme key) keeps tree-sitter's colour"
    );
    app.settings.semantic_highlighting = SemanticHighlighting::Off;
    draw(&mut term, &mut app);
    assert_eq!(fgs(&term, "foo"), vec![foo_off; 3], "off is tree-sitter's colour");

    // Typing before the token: it moves with the text, with no answer yet (no flicker).
    app.settings.semantic_highlighting = SemanticHighlighting::On;
    fake.hold(DELTA, true);
    app.press_file_start();
    app.type_text("  ");
    draw(&mut term, &mut app);
    assert_eq!(fgs(&term, "foo"), vec![ctor; 3], "the colour follows the edit");
    assert_ne!(cell_fg(&term, "foo", -1), ctor);
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_off_asks_for_nothing_and_clears() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", TEXT)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        semantic_tokens: true,
        ..Default::default()
    });
    fake.respond(FULL, ctor_answer());
    let mut app = open_app(&root, fake.clone());
    let o: SettingsOverrides = toml::from_str("semantic_highlighting = \"off\"").unwrap();
    o.apply(&mut app.settings);
    assert_eq!(app.settings.semantic_highlighting, SemanticHighlighting::Off);
    open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(app.settle(|_| true).await);
    assert!(fake.requests(FULL).is_empty());
    // On again: asked at once.
    app.settings.semantic_highlighting = SemanticHighlighting::On;
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 2)
            .await
    );
    app.settings.semantic_highlighting = SemanticHighlighting::Off;
    assert!(app.settle(|a| a.active_buffer().unwrap().semantic.is_empty()).await);
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_inactive_regions_are_dimmed() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, path) = tokens_app(dir.path(), Some(ctor_answer())).await;
    let init = fake.requests("initialize");
    assert_eq!(
        init[0]["capabilities"]["textDocument"]["inactiveRegionsCapabilities"]["inactiveRegions"],
        json!(true)
    );
    assert!(
        app.settle(|a| a.active_buffer().unwrap().semantic.tokens().len() == 2)
            .await
    );
    // Line 1 (`bar();`) is compiled out.
    app.lsp.server_for(crate::syntax::Language::Cpp).unwrap().notify(
        "test/inactive",
        json!({
            "textDocument": { "uri": uri(&path) },
            "regions": [{ "start": { "line": 1, "character": 0 }, "end": { "line": 2, "character": 0 } }],
        }),
    );
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().semantic.inactive().is_empty())
            .await
    );
    let mut term = terminal();
    // The same frame without the dimming: the setting off.
    app.settings.semantic_highlighting = SemanticHighlighting::Off;
    draw(&mut term, &mut app);
    let plain = fgs(&term, "bar();");
    let foo_plain = fg_of(&term, "foo");
    app.settings.semantic_highlighting = SemanticHighlighting::On;
    // Dimmed: each cell keeps its own colour, blended into the background.
    draw(&mut term, &mut app);
    let dim: Vec<_> = plain.iter().map(|&fg| dimmed(&app, fg)).collect();
    assert_eq!(fgs(&term, "bar();"), dim);
    assert!(
        dim.iter().zip(&plain).all(|(d, p)| d != p),
        "the dimming changes the colour"
    );
    assert_ne!(
        fg_of(&term, "foo"),
        dimmed(&app, foo_plain),
        "outside the region: not dimmed"
    );
    // Over a selection: the foreground is dimmed, the selection's background kept.
    app.test_execute(Command::SelectAll);
    draw(&mut term, &mut app);
    let (x, y) = find_cells(&term, "bar").unwrap();
    let cell = &term.backend().buffer()[(x, y)];
    assert_eq!(Option::<Color>::from(cell.fg), dim[0]);
    assert_eq!(Some(cell.bg), app.theme.get("editor.selection").bg);
    app.settings.semantic_highlighting = SemanticHighlighting::Off;
    draw(&mut term, &mut app);
    assert_eq!(fg_of(&term, "bar"), plain[0], "off draws no dimming");
    app.shutdown_servers().await;
}

const CLANGD_CPP: &str = "#define LIMIT 10\n\
struct Widget {\n  Widget(int size);\n  int count;\n};\n\
Widget::Widget(int size) : count(size) {}\n\
const int kMax = 5;\n\
int plain(int p) { int v = p; return v + kMax + LIMIT; }\n\
#if 0\nint dead() { return 1; }\n#endif\n";

/// Real clangd: constructors, macros, parameters, fields and `const` values arrive with their own
/// kinds, a plain function is plain, the `#if 0` block is dimmed. Skipped without clangd.
#[tokio::test(flavor = "multi_thread")]
async fn semantic_tokens_done_when_clangd() {
    use crate::lsp::ProcessLauncher;
    use crate::milestone9_tests::{REAL_SERVERS, settle_for, which};
    use crate::semantic::mods;
    if which("clangd").is_none() {
        eprintln!("semantic_tokens_done_when_clangd: skipped, clangd isn't on PATH");
        return;
    }
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", CLANGD_CPP)]);
    let path = root.join("a.cpp");
    let mut app = crate::milestone9_tests::open_app_watched(&root, Arc::new(ProcessLauncher));
    app.test_open(crate::buffer::Buffer::open(&path).unwrap());
    let ready = |a: &App| {
        let s = &a.active_buffer().unwrap().semantic;
        s.tokens().len() > 10 && !s.inactive().is_empty() && a.active_buffer().unwrap().syntax.tree().is_some()
    };
    assert!(
        settle_for(&mut app, 60, ready).await,
        "clangd's tokens and inactive regions"
    );
    let find = |app: &App, at: usize| {
        app.active_buffer()
            .unwrap()
            .semantic
            .tokens()
            .iter()
            .find(|t| t.start.0 == at)
            .map(|t| (t.kind, t.mods))
    };
    let at = |needle: &str, nth: usize| CLANGD_CPP.match_indices(needle).nth(nth).unwrap().0;
    let ctor = find(&app, at("Widget::Widget", 0) + 8).expect("constructor token");
    assert_eq!(ctor.0, Kind::Class);
    assert_ne!(ctor.1 & mods::CONSTRUCTOR_OR_DESTRUCTOR, 0);
    assert_eq!(find(&app, at("LIMIT", 1)).map(|t| t.0), Some(Kind::Macro));
    assert_eq!(find(&app, at("size", 1)).map(|t| t.0), Some(Kind::Parameter));
    assert_eq!(find(&app, at("count", 0)).map(|t| t.0), Some(Kind::Property));
    let k = find(&app, at("kMax", 1)).expect("const value");
    assert_ne!(k.1 & mods::READONLY, 0);
    assert_eq!(find(&app, at("plain", 0)).map(|t| t.0), Some(Kind::Function));
    let dead = at("int dead", 0);
    let (from, to) = (crate::buffer::pos::ByteIdx(dead), crate::buffer::pos::ByteIdx(dead + 3));
    assert_eq!(
        app.active_buffer()
            .unwrap()
            .semantic
            .overlapping_inactive(from, to)
            .len(),
        1,
        "the #if 0 body is inactive"
    );
    // Drawn: each in its own colour, apart from tree-sitter's plain function and variable, which
    // clangd's plain `function` / `variable` tokens (no theme key) leave as they are.
    let drawn = |term: &ratatui::Terminal<ratatui::backend::TestBackend>| {
        [
            cell_fg(term, "::Widget(", 2),
            cell_fg(term, "LIMIT;", 0),
            cell_fg(term, "size) {", 0),
            cell_fg(term, "count(size", 0),
            cell_fg(term, "kMax +", 0),
            cell_fg(term, "plain(", 0),
            cell_fg(term, "v +", 0),
        ]
    };
    let mut term = terminal();
    draw(&mut term, &mut app);
    let on = drawn(&term);
    let key = |k: &str| fg_style(&app, k);
    let (function, variable) = (key("function"), key("variable"));
    assert_eq!(
        on,
        [
            key("semantic.constructor"),
            key("semantic.macro"),
            key("semantic.parameter"),
            key("semantic.property"),
            key("semantic.readonly"),
            function,
            variable,
        ]
    );
    // A parameter is the variable's light blue (not grey), the others are coloured apart.
    assert_eq!(on[2], variable);
    for (i, fg) in on[..5].iter().enumerate().filter(|(i, _)| *i != 2) {
        assert!(
            *fg != function && *fg != variable,
            "token {i} is coloured apart: {on:?}"
        );
    }
    let dead_on = fg_of(&term, "int dead");
    // Off: tree-sitter's colours, the `#if 0` body too.
    app.settings.semantic_highlighting = SemanticHighlighting::Off;
    draw(&mut term, &mut app);
    let off = drawn(&term);
    // Compiled-out code keeps its syntax colours, dimmed.
    let dead_off = fg_of(&term, "int dead");
    assert_eq!(dead_on, dimmed(&app, dead_off));
    assert_ne!(dead_on, dead_off);
    assert_eq!(off[0], function, "tree-sitter's constructor is a function: {off:?}");
    assert_eq!(off[4], variable);
    assert_eq!(off[5..], on[5..]);
    app.shutdown_servers().await;
}
