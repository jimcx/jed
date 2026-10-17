//! End-to-end tests for the Tools view and logs milestone (`tools`). So far tools-a, the log
//! store: a language server's stderr, lifecycle (start, `initialize`, crash) and
//! `window/logMessage` / `showMessage` land in its ring and in `<state dir>/logs/tools/<name>.log`
//! (with time, pid and root); its protocol trace goes there while on; jed's own `tracing` events,
//! at the filter's level, land in "jed" and aren't written again under `logs/tools`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::buffer::Buffer;
use crate::logs::{Level, Logs, Tool};
use crate::lsp::State;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::syntax::Language;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

/// `<dir>/proj` with an `a.cpp`, and `<dir>/state` (the trust store, and the state dir when
/// `state` is set).
fn open_app(dir: &Path, fake: &Arc<FakeLauncher>, logs: Logs, state: bool) -> (App, PathBuf) {
    let base = workspace::normalize(dir);
    let root = base.join("proj");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.cpp"), "int a;\n").unwrap();
    let trust = TrustStore::load(base.join("state/trust.toml"));
    let mut app = App::new(Workspace::load(root.clone()), trust, None, true);
    app.lsp.set_launcher(fake.clone());
    // As `main` does: the store its `tracing` layer feeds.
    app.logs = logs;
    if state {
        app.state_dir = Some(base.join("state"));
    }
    app.test_start();
    (app, root)
}

fn clangd() -> Tool {
    Tool::Server("clangd".into())
}

fn has(app: &App, tool: &Tool, text: &str) -> bool {
    app.logs.texts(tool).iter().any(|t| t == text)
}

fn level_of(app: &App, tool: &Tool, text: &str) -> Option<Level> {
    app.logs
        .lines(tool)
        .into_iter()
        .find(|l| l.text == text)
        .map(|l| l.level)
}

async fn flush(logs: &Logs) {
    let logs = logs.clone();
    let done = tokio::task::spawn_blocking(move || logs.flush(Duration::from_secs(5)))
        .await
        .unwrap();
    assert!(done, "tool logs written");
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_language_server_lines_land_in_its_ring_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), true);
    assert!(app.settle(|a| !a.session.loading()).await);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await
    );
    let tool = clangd();
    // Lifecycle, the fake's stderr at launch and its `logMessage` while initializing.
    let start = format!("Starting: clangd (in {})", root.display());
    assert!(
        app.settle(|a| has(a, &tool, "fake server starting") && has(a, &tool, "fake ready"))
            .await,
        "{:?}",
        app.logs.texts(&tool)
    );
    assert_eq!(app.logs.texts(&tool)[0], start);
    assert!(has(&app, &tool, "Initialized: fake 1, positions in UTF-16"));
    assert_eq!(level_of(&app, &tool, "fake ready"), Some(Level::Info));

    let server = app.lsp.server_for(Language::Cpp).unwrap();
    server.notify("test/stderr", json!({ "text": "warming up\nstill warming" }));
    server.notify("test/log", json!({ "type": 1, "message": "index broken" }));
    server.notify("test/log", json!({ "type": 4, "message": "verbose detail" }));
    server.notify("test/show", json!({ "type": 2, "message": "check your config" }));
    assert!(
        app.settle(|a| {
            has(a, &tool, "still warming")
                && has(a, &tool, "verbose detail")
                && has(a, &tool, "Message: check your config")
        })
        .await,
        "{:?}",
        app.logs.texts(&tool)
    );
    assert!(has(&app, &tool, "warming up"));
    assert_eq!(level_of(&app, &tool, "index broken"), Some(Level::Error));
    assert_eq!(level_of(&app, &tool, "verbose detail"), Some(Level::Debug));
    assert_eq!(level_of(&app, &tool, "Message: check your config"), Some(Level::Warn));
    assert_eq!(app.logs.errors(&tool), 1);

    // A crash: its reason as an error line.
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/crash", json!(null));
    assert!(
        app.settle(|a| matches!(a.lsp.server_for(Language::Cpp).map(|s| &s.state), Some(State::Failed)))
            .await
    );
    assert!(
        app.settle(|a| has(a, &tool, "Crashed: exited")).await,
        "{:?}",
        app.logs.texts(&tool)
    );
    assert_eq!(level_of(&app, &tool, "Crashed: exited"), Some(Level::Error));

    // The same lines on disk, each with time, pid and root; nothing for "jed".
    flush(&app.logs).await;
    let tools = dir.path().join("state/logs/tools");
    let text = fs::read_to_string(tools.join("clangd.log")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), app.logs.texts(&tool).len(), "{text}");
    let tag = format!(" UTC [{} {}] ", std::process::id(), root.display());
    for line in &lines {
        assert!(line.contains(&tag), "{line}");
    }
    assert!(lines[0].ends_with(&format!("INFO  {start}")), "{}", lines[0]);
    assert!(lines.iter().any(|l| l.ends_with("ERROR index broken")));
    assert!(lines.iter().any(|l| l.ends_with("ERROR Crashed: exited")));
    assert!(!tools.join("jed.log").exists());
    assert!(app.message.is_some(), "the crash is reported");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_protocol_trace_goes_to_the_log_while_on() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await
    );
    let tool = clangd();
    let traced = |a: &App| {
        a.logs
            .texts(&tool)
            .into_iter()
            .filter(|t| t.starts_with(['→', '←']))
            .count()
    };
    assert_eq!(traced(&app), 0, "off by default");

    let server = app.lsp.server_for(Language::Cpp).unwrap();
    assert!(!server.trace());
    server.set_trace(true);
    let id = server.request("test/echo", json!({ "ping": 1 }));
    assert!(app.settle(|a| traced(a) >= 2).await);
    let texts = app.logs.texts(&tool);
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("→ ") && t.contains("\"test/echo\"") && t.contains(&format!("\"id\":{id}"))),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t.starts_with("← ") && t.contains("\"ping\":1")));
    assert!(
        app.logs
            .lines(&tool)
            .iter()
            .filter(|l| l.text.starts_with(['→', '←']))
            .all(|l| l.level == Level::Trace)
    );

    let server = app.lsp.server_for(Language::Cpp).unwrap();
    server.set_trace(false);
    let before = traced(&app);
    server.request("test/echo", json!({ "ping": 2 }));
    tokio::time::sleep(Duration::from_millis(200)).await;
    app.pump();
    assert_eq!(traced(&app), before, "off again");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_jed_events_land_in_jed_at_the_log_level() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let logs = Logs::default();
    // `logging::init`'s stack, on this thread (the file's lines to nowhere).
    let (subscriber, handle) = crate::logging::subscriber(EnvFilter::new("info"), std::io::sink, logs.clone());
    // While only one subscriber exists, tracing-core caches a callsite's interest from the
    // subscriber of whichever thread hits it first: another test's thread (none) would disable
    // the app's callsites for this one too. A second one alive makes it ask every subscriber.
    let _second = tracing::Dispatch::new(tracing_subscriber::registry());
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    let (mut app, root) = open_app(dir.path(), &fake, logs, true);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    // `Manager::sync` (on this, the UI thread) logs the start at info, the request at debug.
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await
    );
    let jed = app.logs.texts(&Tool::Jed);
    assert!(
        jed.contains(&"jed::lsp: starting language server server=clangd language=cpp".to_string()),
        "{jed:?}"
    );
    let buffer = app.buffers.find_path(&root.join("a.cpp")).unwrap();
    let sent = app.lsp.request_at(
        &mut app.buffers,
        buffer,
        crate::buffer::pos::ByteIdx(0),
        "textDocument/hover",
        |_| true,
        json!({}),
    );
    assert!(sent.is_some());
    assert!(
        !app.logs
            .texts(&Tool::Jed)
            .iter()
            .any(|t| t.starts_with("jed::lsp: request")),
        "debug is below the filter"
    );
    assert!(app.logs.lines(&Tool::Jed).iter().all(|l| l.level <= Level::Info));

    // "Set Log Level" changes what "jed" gets too.
    crate::logging::set_filter(&handle, "debug").unwrap();
    assert!(
        app.logs
            .texts(&Tool::Jed)
            .contains(&"jed::logging: log filter changed filter=debug".to_string())
    );
    let sent = app.lsp.request_at(
        &mut app.buffers,
        buffer,
        crate::buffer::pos::ByteIdx(0),
        "textDocument/hover",
        |_| true,
        json!({}),
    );
    let id = sent.unwrap().id;
    let request = format!("jed::lsp: request server=clangd method=textDocument/hover id={id}");
    let jed = app.logs.texts(&Tool::Jed);
    assert!(jed.iter().any(|t| t.starts_with(&request)), "{jed:?}");
    assert_eq!(app.logs.lines(&Tool::Jed).last().map(|l| l.level), Some(Level::Debug));

    // "jed" isn't written under `logs/tools` (it's the daily log file already).
    flush(&app.logs).await;
    let tools = dir.path().join("state/logs/tools");
    let names: Vec<String> = fs::read_dir(&tools)
        .map(|d| d.map(|e| e.unwrap().file_name().into_string().unwrap()).collect())
        .unwrap_or_default();
    assert!(names.iter().all(|n| n.starts_with("clangd.log")), "{names:?}");
    app.shutdown_servers().await;
}

/// A real process that dies before answering `initialize`, writing to stderr after its output
/// closed: its pid, the lines read after the crash and the crash itself are in its log.
#[tokio::test(flavor = "multi_thread")]
async fn tools_an_early_crash_keeps_its_last_words() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), true);
    app.lsp.set_launcher(Arc::new(crate::lsp::ProcessLauncher));
    let script = "exec 1>&-; sleep 0.1; echo 'first words' >&2; echo 'last words' >&2; exit 3";
    app.lsp.builtins.clangd = crate::lsp::ServerConfig::new("sh", &["-c", script]);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    let tool = Tool::Server("sh".into());
    assert!(
        app.settle(|a| a.logs.texts(&tool).iter().any(|t| t.starts_with("Crashed: ")))
            .await,
        "{:?}",
        app.logs.texts(&tool)
    );
    let texts = app.logs.texts(&tool);
    assert_eq!(
        texts[0],
        format!(
            "Starting: sh -c {} (in {})",
            crate::recent::shell_quote(script),
            root.display()
        )
    );
    let pid = texts[1].strip_prefix("Started, pid ").expect("the pid line");
    assert!(pid.parse::<u32>().is_ok(), "{pid}");
    let words = texts
        .iter()
        .position(|t| t == "first words")
        .expect("stderr after the crash");
    assert_eq!(texts[words + 1], "last words");
    let crash = texts.last().unwrap();
    assert!(
        crash.starts_with("Crashed: exited during initialize: last words") && crash.contains("3"),
        "{crash}"
    );
    assert_eq!(level_of(&app, &tool, crash), Some(Level::Error));
    flush(&app.logs).await;
    let file = fs::read_to_string(dir.path().join("state/logs/tools/sh.log")).unwrap();
    assert!(file.contains("INFO  last words\n"), "{file}");
    app.shutdown_servers().await;
}

/// A command that isn't there, and a server refusing `initialize`.
#[tokio::test(flavor = "multi_thread")]
async fn tools_start_failures_are_logged() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions {
        missing: vec!["clangd".into()],
        ..FakeOptions::default()
    });
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    let tool = clangd();
    assert!(app.settle(|a| has(a, &tool, "Can't start: clangd not found")).await);
    assert_eq!(
        level_of(&app, &tool, "Can't start: clangd not found"),
        Some(Level::Error)
    );
    assert!(!app.logs.texts(&tool).iter().any(|t| t.starts_with("Started")));
    app.shutdown_servers().await;

    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions {
        init_error: true,
        ..FakeOptions::default()
    });
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    let failed = "Failed to initialize: fake can't start";
    assert!(
        app.settle(|a| has(a, &tool, failed)).await,
        "{:?}",
        app.logs.texts(&tool)
    );
    assert_eq!(level_of(&app, &tool, failed), Some(Level::Error));
    assert!(!app.logs.texts(&tool).iter().any(|t| t.starts_with("Initialized")));
    app.shutdown_servers().await;

    // A file that isn't executable: the spawn's error.
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    fs::write(root.join("not-a-program"), "").unwrap();
    app.lsp.set_launcher(Arc::new(crate::lsp::ProcessLauncher));
    app.lsp.builtins.clangd = crate::lsp::ServerConfig::new("./not-a-program", &[]);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    let tool = Tool::Server("not-a-program".into());
    let spawn_error = |a: &App| {
        a.logs
            .texts(&tool)
            .iter()
            .any(|t| t.starts_with("Can't start: ") && t.contains("ermission denied"))
    };
    assert!(app.settle(spawn_error).await, "{:?}", app.logs.texts(&tool));
    app.shutdown_servers().await;
}

/// Quitting stops the server and writes its last line ("Stopped") before jed exits.
#[tokio::test(flavor = "multi_thread")]
async fn tools_quit_writes_the_last_lines() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), true);
    assert!(app.settle(|a| !a.session.loading()).await);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await
    );
    app.test_execute(crate::command::Command::Quit);
    app.shutdown().await;
    assert!(
        has(&app, &clangd(), "Stopped (killed)"),
        "{:?}",
        app.logs.texts(&clangd())
    );
    // No flush here: the quit's.
    let file = fs::read_to_string(dir.path().join("state/logs/tools/clangd.log")).unwrap();
    assert!(file.ends_with("INFO  Stopped (killed)\n"), "{file}");
}

/// A log folder that can't be written: one warning, the lines kept in memory.
#[tokio::test(flavor = "multi_thread")]
async fn tools_a_write_error_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let base = workspace::normalize(dir.path());
    // A file where `state/logs` should be.
    fs::create_dir_all(base.join("state")).unwrap();
    fs::write(base.join("state/logs"), "").unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), true);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    let warned = |a: &App| {
        a.message
            .as_ref()
            .is_some_and(|m| m.text.contains("Can't write the clangd log"))
    };
    assert!(app.settle(warned).await, "{:?}", app.message.as_ref().map(|m| &m.text));
    app.message = None;
    let server = app.lsp.server_for(Language::Cpp).unwrap();
    server.notify("test/stderr", json!({ "text": "more" }));
    assert!(app.settle(|a| has(a, &clangd(), "more")).await);
    flush(&app.logs).await;
    app.pump();
    assert!(
        app.message.is_none(),
        "once: {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    app.shutdown_servers().await;
}
