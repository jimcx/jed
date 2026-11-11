//! End-to-end tests for Claude's language server queries (`claude_lsp`), through a real MCP client
//! (`claude::tests::run_mcp` over a `claude::Socket`), the real socket and app, and the fake
//! server (real rust-analyzer for one). Covers the done-when:
//!
//! - `claude_lsp_definition_references_and_hover_leave_the_user_alone`: definition by `line` +
//!   `symbol` with its line text, references in two files (one not open) sorted with `count`,
//!   hover text; the selection, the active tab and the back / forward entries stay
//! - `claude_lsp_a_symbol_not_on_the_line_and_a_dirty_buffer`: the quoted-line error, `dirty: true`
//! - `claude_lsp_requests_wait_ask_again_time_out_and_retry`: a cursor move and a tab switch don't
//!   cancel, an edit answers "ask again", a silent server times out with `$/cancelRequest`,
//!   ContentModified is asked again
//! - `claude_lsp_a_file_without_a_tab_opens_in_the_background_and_causes_are_named`: `opened: true`,
//!   no server for the language, a missing capability
//! - `claude_lsp_document_symbols_both_shapes_and_workspace_symbols`
//! - `claude_lsp_real_rust_analyzer_definition_references_and_workspace_symbol` (skipped without it)
//! - `claude_lsp_the_mcp_log_shows_each_call_and_its_answer_size`

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::app::App;
use crate::claude::{self, Socket};
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::lsp::{Launcher, ProcessLauncher};
use crate::milestone9_tests::{REAL_SERVERS, location, open, open_synced, project, put_cursor, settle_for};
use crate::syntax::Language;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

const A: &str = "int foo() { return 1; }\nint main() {\treturn foo() + foo(); }\n";

struct Env {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    app: App,
    fake: Arc<FakeLauncher>,
    root: PathBuf,
    socket: PathBuf,
}

/// An app on a project with a listening socket and servers started by `launcher`; with
/// `open_a`, `a.cpp` open and synced to `fake`.
async fn env_with(files: &[(&str, &str)], launcher: Arc<dyn Launcher>, fake: Arc<FakeLauncher>, open_a: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let sockets = tempfile::tempdir().unwrap();
    let root = project(dir.path(), files);
    let trust = TrustStore::load(root.parent().unwrap().join("state/trust.toml"));
    let mut app = App::new(Workspace::load(root.clone()), trust, None, true);
    app.lsp.set_launcher(launcher);
    let sock_dir = workspace::normalize(sockets.path()).join("s");
    let socket = crate::ipc::socket_path(&sock_dir, std::process::id());
    app.ipc_dir = Some(sock_dir);
    app.test_start();
    app.test_plain_whitespace();
    assert!(app.settle(|a| a.tasks.ipc.is_some()).await, "the socket is listening");
    if open_a {
        open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    }
    Env {
        _dirs: (dir, sockets),
        app,
        fake,
        root,
        socket,
    }
}

async fn env() -> Env {
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    env_with(
        &[("a.cpp", A), ("b.cpp", "int bar;\n"), ("n.txt", "plain\n")],
        fake.clone(),
        fake,
        true,
    )
    .await
}

/// The scripted client: `initialize`, then each call in order, on its own thread.
fn mcp(socket: &Path, calls: Vec<(&'static str, Value)>) -> JoinHandle<Vec<Value>> {
    let socket = socket.to_path_buf();
    std::thread::spawn(move || {
        let mut jed = Socket::new(Some(socket.into_os_string()), Some("99".into()));
        let mut lines = vec![json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})];
        for (i, (name, args)) in calls.into_iter().enumerate() {
            lines.push(claude::tests::call(i as u64 + 2, name, args));
        }
        claude::tests::run_mcp(&lines, &mut jed)
    })
}

/// A tool result: its JSON (`Ok`) or its error text.
fn answer(out: &Value) -> Result<Value, String> {
    let text = out["result"]["content"][0]["text"].as_str().expect("a text result");
    if out["result"]["isError"] == true {
        Err(text.to_string())
    } else {
        Ok(serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}")))
    }
}

/// The worker's output (initialize's answer first), once the app has run it.
async fn finish(app: &mut App, worker: JoinHandle<Vec<Value>>) -> Vec<Value> {
    assert!(app.settle(|_| worker.is_finished()).await, "no answer");
    worker.join().unwrap()
}

/// Runs the calls and returns their answers.
async fn calls(env: &mut Env, calls: Vec<(&'static str, Value)>) -> Vec<Result<Value, String>> {
    let worker = mcp(&env.socket, calls);
    finish(&mut env.app, worker).await[1..].iter().map(answer).collect()
}

/// What the user sees: the cursor, the active buffer, the back / forward entries and index.
fn user_state(app: &App) -> (usize, crate::buffer::BufferId, usize, usize) {
    let tab = app.group().active_tab().unwrap();
    (
        tab.view.selections.primary().head.0,
        tab.buffer,
        app.nav.history.entries().len(),
        app.nav.history.index(),
    )
}

fn path(env: &Env, name: &str) -> String {
    env.root.join(name).to_string_lossy().into_owned()
}

fn flush(app: &App) {
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_lsp_definition_references_and_hover_leave_the_user_alone() {
    let mut e = env().await;
    let (a, b) = (e.root.join("a.cpp"), e.root.join("b.cpp"));
    e.fake
        .respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
    e.fake.respond(
        "textDocument/references",
        json!([location(&b, 0, 4, 7), location(&a, 1, 20, 23), location(&a, 0, 4, 7)]),
    );
    e.fake.respond(
        "textDocument/hover",
        json!({"contents": {"kind": "markdown", "value": "```cpp\nint foo()\n```"}}),
    );
    put_cursor(&mut e.app, 3);
    let before = user_state(&e.app);
    let (pa, tabs) = (path(&e, "a.cpp"), e.app.group().tabs.len());
    let out = calls(
        &mut e,
        vec![
            ("find_definition", json!({"path": pa, "line": 2, "symbol": "foo"})),
            ("find_references", json!({"path": pa, "line": 1, "symbol": "foo"})),
            ("get_hover", json!({"path": pa, "line": 1, "symbol": "foo"})),
        ],
    )
    .await;
    let def = out[0].clone().unwrap();
    assert_eq!(
        def["locations"],
        json!([{"path": "a.cpp", "line": 1, "col": 5, "end_line": 1, "end_col": 8, "text": "int foo() { return 1; }"}])
    );
    let refs = out[1].clone().unwrap();
    let places: Vec<(&str, u64)> = refs["locations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["path"].as_str().unwrap(), l["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(places, [("a.cpp", 1), ("a.cpp", 2), ("b.cpp", 1)], "sorted, two files");
    assert_eq!(refs["count"], 3);
    assert_eq!(refs["dirty"], false, "a clean buffer");
    assert_eq!(
        refs["locations"][2]["text"], "int bar;",
        "b.cpp isn't open: its line read from disk"
    );
    assert_eq!(out[2].clone().unwrap()["contents"], "```cpp\nint foo()\n```");
    assert_eq!(user_state(&e.app), before, "cursor, tab, back / forward untouched");
    assert_eq!(e.app.group().tabs.len(), tabs, "references don't open b.cpp");
    assert!(e.app.message.is_none(), "nothing notified");
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_lsp_a_symbol_not_on_the_line_and_a_dirty_buffer() {
    let mut e = env().await;
    let a = e.root.join("a.cpp");
    let pa = path(&e, "a.cpp");
    let out = calls(
        &mut e,
        vec![("find_definition", json!({"path": pa, "line": 1, "symbol": "main"}))],
    )
    .await;
    let err = out[0].clone().unwrap_err();
    assert!(
        err.contains("`main` is not on line 1") && err.contains("int foo() { return 1; }"),
        "{err}"
    );
    assert!(e.fake.requests("textDocument/definition").is_empty(), "nothing asked");
    put_cursor(&mut e.app, 0);
    e.app.type_text("// c\n");
    e.fake
        .respond("textDocument/definition", json!([location(&a, 1, 0, 3)]));
    let out = calls(
        &mut e,
        vec![("find_definition", json!({"path": pa, "line": 2, "symbol": "int"}))],
    )
    .await;
    let d = out[0].clone().unwrap();
    assert_eq!(d["dirty"], true);
    assert_eq!(
        d["locations"][0]["text"], "int foo() { return 1; }",
        "the buffer's lines"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_lsp_requests_wait_ask_again_time_out_and_retry() {
    let mut e = env().await;
    let (a, b) = (e.root.join("a.cpp"), e.root.join("b.cpp"));
    let pa = path(&e, "a.cpp");
    let one = |name: &'static str| vec![(name, json!({"path": pa.clone(), "line": 1, "symbol": "foo"}))];

    // An edit answers "ask again" and cancels.
    e.fake.respond("textDocument/references", json!([]));
    e.fake.hold("textDocument/references", true);
    let worker = mcp(&e.socket, one("find_references"));
    assert!(
        e.app
            .settle(|_| e.fake.requests("textDocument/references").len() == 1)
            .await
    );
    put_cursor(&mut e.app, 0);
    e.app.type_text("x");
    let out = finish(&mut e.app, worker).await;
    let err = answer(&out[1]).unwrap_err();
    assert!(err.contains("ask again"), "{err}");
    assert!(e.app.settle(|_| !e.fake.cancelled().is_empty()).await, "cancelled");
    e.fake.hold("textDocument/references", false);

    // A cursor move and a tab switch don't cancel the request.
    let cancelled = e.fake.cancelled().len();
    e.fake
        .respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
    e.fake.hold("textDocument/definition", true);
    let worker = mcp(&e.socket, one("find_definition"));
    assert!(
        e.app
            .settle(|_| e.fake.requests("textDocument/definition").len() == 1)
            .await
    );
    put_cursor(&mut e.app, 10);
    open(&mut e.app, &b);
    e.app.pump();
    assert_eq!(e.fake.cancelled().len(), cancelled, "{:?}", e.fake.cancelled());
    flush(&e.app);
    let out = finish(&mut e.app, worker).await;
    let d = answer(&out[1]).unwrap();
    assert_eq!(d["count"], 1);
    assert_eq!(d["locations"][0]["path"], "a.cpp", "the real answer, not an error");
    assert_eq!(
        e.fake.cancelled().len(),
        cancelled,
        "still not cancelled after the answer"
    );
    e.fake.hold("textDocument/definition", false);

    // A silent server: a timeout (shortened) and a `$/cancelRequest`.
    e.fake.respond("textDocument/hover", json!(null));
    e.fake.hold("textDocument/hover", true);
    // Generous enough that the request reaches the server first on a loaded machine.
    e.app.test_claude_lsp_times(Some(Duration::from_millis(2000)), None);
    let out = calls(&mut e, one("get_hover")).await;
    assert_eq!(e.fake.requests("textDocument/hover").len(), 1, "the request was sent");
    assert!(out[0].clone().unwrap_err().contains("timed out"), "{:?}", out[0]);
    assert!(
        e.app.settle(|_| e.fake.cancelled().len() > cancelled).await,
        "$/cancelRequest sent"
    );

    // ContentModified is asked again, then answered.
    e.app.test_claude_lsp_times(None, Some(Duration::from_millis(50)));
    e.fake.fail("textDocument/definition", -32801, "content modified");
    let asked = e.fake.requests("textDocument/definition").len();
    let worker = mcp(&e.socket, one("find_definition"));
    assert!(
        e.app
            .settle(|_| e.fake.requests("textDocument/definition").len() > asked)
            .await
    );
    assert!(!worker.is_finished(), "not answered with the routine error");
    e.fake.unfail("textDocument/definition");
    let out = finish(&mut e.app, worker).await;
    assert_eq!(answer(&out[1]).unwrap()["count"], 1);
    assert!(
        e.fake.requests("textDocument/definition").len() > asked + 1,
        "asked again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_lsp_a_file_without_a_tab_opens_in_the_background_and_causes_are_named() {
    let mut e = env().await;
    let a = e.root.join("a.cpp");
    e.fake
        .respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
    put_cursor(&mut e.app, 3);
    let before = user_state(&e.app);
    let tabs = e.app.group().tabs.len();
    let pb = path(&e, "b.cpp");
    let out = calls(
        &mut e,
        vec![("find_definition", json!({"path": pb, "line": 1, "symbol": "bar"}))],
    )
    .await;
    let d = out[0].clone().unwrap();
    assert_eq!(d["opened"], true);
    assert_eq!(d["locations"][0]["path"], "a.cpp");
    assert_eq!(e.app.group().tabs.len(), tabs + 1, "a background tab");
    assert!(e.app.buffers.find_path(&e.root.join("b.cpp")).is_some());
    assert_eq!(user_state(&e.app), before);

    // No server for the language.
    let pn = path(&e, "n.txt");
    let out = calls(&mut e, vec![("get_hover", json!({"path": pn, "line": 1}))]).await;
    let err = out[0].clone().unwrap_err();
    assert!(err.contains("no language server for Plain Text"), "{err}");

    // A server without the capability.
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        no_hover: true,
        ..Default::default()
    });
    let mut e = env_with(&[("a.cpp", A)], fake.clone(), fake, true).await;
    let pa = path(&e, "a.cpp");
    let out = calls(
        &mut e,
        vec![("get_hover", json!({"path": pa, "line": 1, "symbol": "foo"}))],
    )
    .await;
    let err = out[0].clone().unwrap_err();
    assert!(err.contains("does not support textDocument/hover"), "{err}");
}

fn rng(a: u32, b: u32, c: u32, d: u32) -> Value {
    json!({"start": {"line": a, "character": b}, "end": {"line": c, "character": d}})
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_lsp_document_symbols_both_shapes_and_workspace_symbols() {
    let mut e = env().await;
    let (pa, b) = (path(&e, "a.cpp"), e.root.join("b.cpp"));
    e.fake.respond(
        "textDocument/documentSymbol",
        json!([
            {"name": "Shape", "kind": 23, "range": rng(0, 0, 1, 10), "selectionRange": rng(0, 4, 0, 7),
             "children": [{"name": "foo", "kind": 6, "range": rng(1, 0, 1, 10), "selectionRange": rng(1, 4, 1, 8)}]},
            {"name": "main", "kind": 12, "range": rng(1, 0, 1, 40), "selectionRange": rng(1, 4, 1, 8)}
        ]),
    );
    let before = user_state(&e.app);
    let out = calls(&mut e, vec![("get_document_symbols", json!({"path": pa}))]).await;
    let d = out[0].clone().unwrap();
    assert_eq!(d["symbols"][0]["name"], "Shape");
    assert_eq!(d["symbols"][0]["children"][0]["name"], "foo");
    assert_eq!(d["symbols"][1]["name"], "main");
    assert_eq!(d["count"], 3);
    let uri = crate::lsp::uri::from_path(&e.root.join("a.cpp"));
    let loc = |a, b, c, d| json!({"uri": uri.as_str(), "range": rng(a, b, c, d)});
    e.fake.respond(
        "textDocument/documentSymbol",
        json!([
            {"name": "Shape", "kind": 23, "location": loc(0, 0, 1, 10)},
            {"name": "foo", "kind": 6, "containerName": "Shape", "location": loc(1, 0, 1, 10)},
            {"name": "main", "kind": 12, "location": loc(1, 12, 1, 40)}
        ]),
    );
    let out = calls(&mut e, vec![("get_document_symbols", json!({"path": pa}))]).await;
    let d = out[0].clone().unwrap();
    assert_eq!(d["symbols"][0]["children"][0]["name"], "foo", "nested by containerName");
    assert_eq!(d["symbols"].as_array().unwrap().len(), 2);

    let buri = crate::lsp::uri::from_path(&b);
    e.fake.respond(
        "workspace/symbol",
        json!([{"name": "Shape", "kind": 23, "containerName": "geom",
                "location": {"uri": buri.as_str(), "range": rng(0, 4, 0, 9)}}]),
    );
    let out = calls(&mut e, vec![("find_workspace_symbols", json!({"query": "Shape"}))]).await;
    assert_eq!(
        out[0].clone().unwrap()["symbols"],
        json!([{"name": "Shape", "kind": "struct", "container": "geom", "path": "b.cpp", "line": 1, "col": 5}])
    );
    assert_eq!(user_state(&e.app), before);
}

const CARGO_TOML: &str = "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";
const RA_MAIN: &str =
    "mod util;\n\nfn main() {\n    let t = util::total(&util::Point { xcoord: 1, ycoord: 2 });\n    let _ = t;\n}\n";
const RA_UTIL: &str = "/// A point.\npub struct Point {\n    pub xcoord: i32,\n    pub ycoord: i32,\n}\n\n/// Adds the coordinates.\npub fn total(p: &Point) -> i32 {\n    p.xcoord + p.ycoord\n}\n";

/// Asks `name` until `ok` accepts the answer (rust-analyzer answers nothing, or errors, while it
/// loads the crate), up to `secs`; the last answer is returned.
async fn retry(
    e: &mut Env,
    secs: u64,
    name: &'static str,
    args: Value,
    ok: impl Fn(&Value) -> bool,
) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let last = calls(e, vec![(name, args.clone())]).await.remove(0);
        if last.as_ref().is_ok_and(&ok) || Instant::now() >= deadline {
            return last;
        }
        settle_for(&mut e.app, 1, |_| false).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_lsp_real_rust_analyzer_definition_references_and_workspace_symbol() {
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let toolchain = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("rust-toolchain.toml")).unwrap();
    let probe = tempfile::tempdir().unwrap();
    let runs = std::process::Command::new("rust-analyzer")
        .arg("--version")
        .current_dir(probe.path())
        .output()
        .is_ok_and(|o| o.status.success());
    if !runs {
        eprintln!(
            "claude_lsp real rust-analyzer: skipped, rust-analyzer doesn't run (`rustup component add rust-analyzer`)"
        );
        return;
    }
    let started = Instant::now();
    let fake = FakeLauncher::new(Default::default());
    let mut e = env_with(
        &[
            ("Cargo.toml", CARGO_TOML),
            ("rust-toolchain.toml", &toolchain),
            ("src/main.rs", RA_MAIN),
            ("src/util.rs", RA_UTIL),
        ],
        Arc::new(ProcessLauncher),
        fake,
        false,
    )
    .await;
    let target = workspace::normalize(e._dirs.0.path()).join("target");
    e.app.lsp.builtins.rust_analyzer.init_options = Some(json!({ "cargo": { "targetDir": target } }));
    open(&mut e.app, &e.root.join("src/main.rs"));
    let main = path(&e, "src/main.rs");
    let util = path(&e, "src/util.rs");
    let before = user_state(&e.app);
    let state = |e: &Env| {
        format!(
            "message {:?}, progress {:?}",
            e.app.message.as_ref().map(|m| m.text.clone()),
            e.app.lsp.progress()
        )
    };

    let def = retry(
        &mut e,
        150,
        "find_definition",
        json!({"path": main, "line": 4, "symbol": "total"}),
        |d| d["locations"][0]["path"] == "src/util.rs",
    )
    .await;
    let def = def.unwrap_or_else(|err| panic!("definition: {err}; {}", state(&e)));
    assert_eq!(def["locations"][0]["line"], 8, "{def}; {}", state(&e));
    assert!(
        def["locations"][0]["text"].as_str().unwrap().contains("pub fn total"),
        "{def}"
    );

    let refs = retry(
        &mut e,
        150,
        "find_references",
        json!({"path": util, "line": 8, "symbol": "total"}),
        |d| d["count"].as_u64().unwrap_or(0) >= 2,
    )
    .await;
    let refs = refs.unwrap_or_else(|err| panic!("references: {err}; {}", state(&e)));
    let paths: Vec<&str> = refs["locations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["src/main.rs", "src/util.rs"], "{refs}");

    let syms = retry(&mut e, 150, "find_workspace_symbols", json!({"query": "Point"}), |d| {
        d["symbols"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["name"] == "Point"))
    })
    .await;
    let syms = syms.unwrap_or_else(|err| panic!("workspace symbols: {err}; {}", state(&e)));
    let point = syms["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Point")
        .unwrap();
    assert_eq!(
        (point["path"].as_str(), point["line"].as_u64()),
        (Some("src/util.rs"), Some(2)),
        "{syms}"
    );
    assert_eq!(user_state(&e.app), before);
    eprintln!("claude_lsp real rust-analyzer: ran in {:.1?}", started.elapsed());
    e.app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_lsp_the_mcp_log_shows_each_call_and_its_answer_size() {
    use crate::logs::Tool;
    let mut e = env().await;
    let a = e.root.join("a.cpp");
    e.fake
        .respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
    e.fake.respond("workspace/symbol", json!([]));
    let pa = path(&e, "a.cpp");
    let out = calls(
        &mut e,
        vec![
            ("find_definition", json!({"path": pa, "line": 2, "symbol": "foo"})),
            ("find_workspace_symbols", json!({"query": "zzz"})),
        ],
    )
    .await;
    assert!(out[0].is_ok() && out[1].is_ok(), "{out:?}");
    let pid = std::process::id();
    assert!(
        e.app
            .settle(|a| {
                let t = a.logs.texts(&Tool::Claude);
                t.iter()
                    .filter(|l| {
                        l.starts_with(&format!("← [mcp {pid}]")) && l.contains(" lines, ") && l.contains(" B, ")
                    })
                    .count()
                    >= 2
            })
            .await,
        "{:?}",
        e.app.logs.texts(&Tool::Claude)
    );
    let t = e.app.logs.texts(&Tool::Claude);
    let called = |name: &str| {
        t.iter()
            .any(|l| l.starts_with(&format!("→ [mcp {pid}] tools/call {name}")))
    };
    assert!(called("find_definition") && called("find_workspace_symbols"), "{t:?}");
}
