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
