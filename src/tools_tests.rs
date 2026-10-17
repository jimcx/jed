//! End-to-end tests for the Tools view and logs milestone (`tools`). So far tools-a, the log
//! store: a language server's stderr, lifecycle (start, `initialize`, crash) and
//! `window/logMessage` / `showMessage` land in its ring and in `<state dir>/logs/tools/<name>.log`
//! (with time, pid and root); its protocol trace goes there while on; jed's own `tracing` events,
//! at the filter's level, land in "jed" and aren't written again under `logs/tools`.
//!
//! tools-b, log tabs: "Tools: Open Log" (a picker) opens a tool's log as a read-only tab
//! (`<tool> (log)`) that keeps up with new lines, follows the tail from the last line only,
//! refuses edits / undo / save, is never dirty, works with find, copy and splits, is reused
//! when opened again, drops what the ring drops, empties with "Tools: Clear Log", takes many
//! lines in one turn as one change, and is never restored by a session; new lines wake the loop
//! (only while a log tab is open, a turn per frame rather than per line); file commands (git,
//! history, diffs, Go Back) pass a log tab by, and it starts no language server.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::buffer::Buffer;
use crate::buffer::pos;
use crate::logs::{Level, Logs, Tool};
use crate::lsp::State;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::palette::Mode;
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
    // Wait for the answer itself: other traced messages (a `didChange`, …) may come first.
    let answered = |a: &App| {
        a.logs
            .texts(&tool)
            .iter()
            .any(|t| t.starts_with("← ") && t.contains("\"ping\":1"))
    };
    assert!(app.settle(answered).await);
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

// ---- tools-b: read-only log tabs ----

fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

fn demo() -> Tool {
    Tool::Server("demo".into())
}

/// Pushes `line 0` … `line n-1` (from `from`) to `tool`'s log.
fn push_lines(app: &App, tool: &Tool, from: usize, n: usize) {
    for i in from..from + n {
        app.logs.push(tool, Level::Info, &format!("line {i}"));
    }
}

/// "Tools: Open Log" through the palette, picking `name`: returns the picker's labels.
fn open_log(app: &mut App, name: &str) -> Vec<String> {
    app.press(KeyCode::Char('p'), cmd() | KeyModifiers::SHIFT);
    app.type_text("Tools: Open Log");
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, "Tools: Open Log");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    let p = app.palette.as_ref().expect("the log picker");
    assert_eq!(p.mode(), Mode::Logs);
    let labels = p.rows.iter().map(|r| r.label.clone()).collect();
    app.type_text(name);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, name);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.palette.is_none());
    labels
}

fn run_command(app: &mut App, title: &str) {
    app.press(KeyCode::Char('p'), cmd() | KeyModifiers::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
}

/// The active log tab's lines, without their time and level.
fn tab_lines(app: &App) -> Vec<String> {
    app.active_buffer()
        .unwrap()
        .rope
        .to_string()
        .lines()
        .map(|l| l.get(19..).unwrap_or_default().to_string())
        .collect()
}

fn tab_has(app: &App, text: &str) -> bool {
    app.active_buffer()
        .is_some_and(|b| b.log.is_some() && b.rope.to_string().lines().any(|l| l.get(19..) == Some(text)))
}

/// The active view's primary cursor line and top line.
fn cursor_and_top(app: &App) -> (usize, usize) {
    let tab = app.group().active_tab().unwrap();
    let buf = app.buffers.get(tab.buffer).unwrap();
    let head = tab.view.selections.primary().head;
    (pos::line_of(&buf.rope, head), tab.view.top_line())
}

fn draw(app: &mut App, term: &mut Terminal<TestBackend>) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

/// A language server's log opens from the palette (which lists "jed" and the server) as a
/// read-only tab titled `clangd (log)`, and keeps up with its stderr and `logMessage`s.
#[tokio::test(flavor = "multi_thread")]
async fn tools_open_log_shows_a_servers_lines() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    assert!(app.settle(|a| has(a, &clangd(), "fake ready")).await);
    let labels = open_log(&mut app, "clangd");
    assert_eq!(labels, ["jed", "clangd"]);
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.name(), "clangd (log)");
    assert!(buf.read_only.is_some() && !buf.is_dirty());
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    assert!(tab_has(&app, "fake server starting"), "{:?}", tab_lines(&app));
    assert!(tab_has(&app, "fake ready"));
    // A line's time and level come first.
    let first = app.active_buffer().unwrap().rope.to_string();
    assert!(first.lines().any(|l| l.ends_with("INFO  fake ready")), "{first}");

    let server = app.lsp.server_for(Language::Cpp).unwrap();
    server.notify("test/stderr", json!({ "text": "indexing 3/25" }));
    server.notify("test/log", json!({ "type": 1, "message": "index broken" }));
    assert!(
        app.settle(|a| tab_has(a, "indexing 3/25") && tab_has(a, "index broken"))
            .await,
        "{:?}",
        tab_lines(&app)
    );
    let text = app.active_buffer().unwrap().rope.to_string();
    assert!(text.lines().any(|l| l.ends_with("ERROR index broken")), "{text}");
    // The tab's tab bar title.
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    draw(&mut app, &mut term);
    let top: String = (0..80)
        .map(|x| term.backend().buffer()[(x, 0)].symbol().to_string())
        .collect();
    assert!(top.contains("clangd (log)"), "{top}");
    app.shutdown_servers().await;
}

/// jed's own `tracing` events show in the "jed" log tab as they happen.
#[tokio::test(flavor = "multi_thread")]
async fn tools_jed_events_show_in_the_jed_log_tab() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let logs = Logs::default();
    let (subscriber, _handle) = crate::logging::subscriber(EnvFilter::new("info"), std::io::sink, logs.clone());
    // See `tools_jed_events_land_in_jed_at_the_log_level`.
    let _second = tracing::Dispatch::new(tracing_subscriber::registry());
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    let (mut app, _root) = open_app(dir.path(), &fake, logs, false);
    tracing::info!(target: "jed::test", "before the tab");
    open_log(&mut app, "jed");
    assert!(tab_has(&app, "jed::test: before the tab"), "{:?}", tab_lines(&app));
    tracing::info!(target: "jed::test", n = 2, "after the tab");
    assert!(
        app.settle(|a| tab_has(a, "jed::test: after the tab n=2")).await,
        "{:?}",
        tab_lines(&app)
    );
    // New lines wake the loop (`AppEvent::LogsChanged`, which sets the rearm deadline), at
    // most once per `LOG_FRAME`: the timer rearms the store.
    assert!(app.settle(|a| a.log_tabs.deadline().is_none()).await);
    tracing::info!(target: "jed::test", "wake up");
    assert!(app.settle(|a| a.log_tabs.deadline().is_some()).await);
    assert!(app.settle(|a| a.log_tabs.deadline().is_none()).await, "rearmed");
    assert!(tab_has(&app, "jed::test: wake up"));
}

/// The cursor on the last line follows the tail (cursor and scroll); moved up, it stays on its
/// text while lines arrive.
#[tokio::test(flavor = "multi_thread")]
async fn tools_the_tail_is_followed_only_from_the_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = open_app(dir.path(), &fake, Logs::default(), false);
    push_lines(&app, &demo(), 0, 50);
    open_log(&mut app, "demo");
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    draw(&mut app, &mut term);
    let height = app.group().active_tab().unwrap().view.height;
    // On the empty line after the last one, the bottom of the view.
    assert_eq!(cursor_and_top(&app), (50, 51 - height));

    push_lines(&app, &demo(), 50, 10);
    assert!(app.settle(|a| tab_lines(a).len() == 60).await);
    draw(&mut app, &mut term);
    assert_eq!(cursor_and_top(&app), (60, 61 - height), "followed");

    // On the last log line (not the empty line after it): no longer following.
    app.press(KeyCode::Up, KeyModifiers::NONE);
    draw(&mut app, &mut term);
    let before = cursor_and_top(&app);
    assert_eq!(before.0, 59);
    push_lines(&app, &demo(), 60, 5);
    assert!(app.settle(|a| tab_lines(a).len() == 65).await);
    draw(&mut app, &mut term);
    assert_eq!(cursor_and_top(&app), before, "stays put");

    // Up a few more lines: still not.
    for _ in 0..2 {
        app.press(KeyCode::Up, KeyModifiers::NONE);
    }
    draw(&mut app, &mut term);
    let before = cursor_and_top(&app);
    assert_eq!(before.0, 57);
    push_lines(&app, &demo(), 65, 5);
    assert!(app.settle(|a| tab_lines(a).len() == 70).await);
    draw(&mut app, &mut term);
    assert_eq!(cursor_and_top(&app), before, "stays put");
    assert_eq!(tab_lines(&app)[57], "line 57");

    // Back to the end (Cmd+End): following again.
    app.test_execute(crate::command::Command::Move {
        motion: crate::command::Motion::DocEnd,
        select: false,
    });
    push_lines(&app, &demo(), 70, 5);
    assert!(app.settle(|a| tab_lines(a).len() == 75).await);
    draw(&mut app, &mut term);
    assert_eq!(cursor_and_top(&app), (75, 76 - height));
}

/// Typing, paste, cut, Enter and undo are refused (the read-only message), nothing is dirty,
/// Save writes nothing; copy and select all work.
#[tokio::test(flavor = "multi_thread")]
async fn tools_a_log_tab_is_read_only_and_never_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    push_lines(&app, &demo(), 0, 3);
    open_log(&mut app, "demo");
    let text = app.active_buffer().unwrap().rope.to_string();
    let files = |root: &Path| {
        let mut names: Vec<_> = fs::read_dir(root).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        names
    };
    let before = files(&root);

    app.type_text("x");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    app.press(KeyCode::Backspace, KeyModifiers::NONE);
    app.handle_test_event(Event::Paste("pasted".into()));
    app.press(KeyCode::Char('a'), cmd());
    app.press(KeyCode::Char('x'), cmd());
    app.press(KeyCode::Char('z'), cmd());
    app.test_execute(crate::command::Command::Paste);
    app.test_execute(crate::command::Command::Redo);
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.rope.to_string(), text);
    assert!(!buf.is_dirty());
    assert!(
        app.message
            .as_ref()
            .is_some_and(|m| m.text == "Read-only: a log can't be edited"),
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    // Copy (the selection from Cmd+A is still there).
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), text);

    // Save: nothing written anywhere, no save started.
    app.press(KeyCode::Char('s'), cmd());
    app.pump();
    assert!(!app.active_buffer().unwrap().save_in_flight());
    assert_eq!(files(&root), before);
    assert!(
        app.buffers
            .find_path(Path::new(""))
            .is_some_and(|id| app.buffers.get(id).unwrap().log.is_some())
    );
}

/// Find works over a log tab's text.
#[tokio::test(flavor = "multi_thread")]
async fn tools_find_works_in_a_log_tab() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.logs.push(&demo(), Level::Info, "a needle here");
    app.logs.push(&demo(), Level::Warn, "hay");
    app.logs.push(&demo(), Level::Info, "another needle");
    open_log(&mut app, "demo");
    app.press(KeyCode::Char('f'), cmd());
    app.type_text("needle");
    assert_eq!(app.find.matches().len(), 2);
    // A new line with a match: the count follows.
    app.logs.push(&demo(), Level::Info, "third needle");
    assert!(app.settle(|a| a.find.matches().len() == 3).await);
}

/// Opening a log again shows its tab (no second one); a split shows the same log, both views
/// following; opening it in the new group finds the split's tab.
#[tokio::test(flavor = "multi_thread")]
async fn tools_a_log_tab_is_reused_and_splits_show_it() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    push_lines(&app, &demo(), 0, 3);
    open_log(&mut app, "demo");
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    assert_eq!(app.group().tabs.len(), 2);
    open_log(&mut app, "demo");
    assert_eq!(app.group().tabs.len(), 2, "reused");
    assert_eq!(app.group().active, 0);
    let logs = app.buffers.iter().filter(|(_, b)| b.log.is_some()).count();
    assert_eq!(logs, 1);

    app.press(KeyCode::Char('\\'), cmd());
    assert_eq!(app.groups.len(), 2);
    assert_eq!(app.active_buffer().unwrap().name(), "demo (log)");
    push_lines(&app, &demo(), 3, 2);
    assert!(app.settle(|a| tab_lines(a).len() == 5).await);
    let ends: Vec<usize> = app
        .groups
        .ids()
        .into_iter()
        .map(|g| {
            let tab = app.groups.get(g).unwrap().active_tab().unwrap();
            tab.view.selections.primary().head.0
        })
        .collect();
    let len = app.active_buffer().unwrap().rope.len();
    assert_eq!(ends, [len, len], "both views follow");
    open_log(&mut app, "demo");
    assert_eq!(app.group().tabs.len(), 1, "the split's own tab");

    // A view off the tail stays put while the other follows.
    app.press(KeyCode::Up, KeyModifiers::NONE);
    let off = app.group().active_tab().unwrap().view.selections.primary().head;
    push_lines(&app, &demo(), 5, 2);
    assert!(app.settle(|a| tab_lines(a).len() == 7).await);
    assert_eq!(app.group().active_tab().unwrap().view.selections.primary().head, off);
    let len = app.active_buffer().unwrap().rope.len();
    let first = app.groups.ids()[0];
    let other = app.groups.get(first).unwrap().active_tab().unwrap();
    assert_eq!(other.view.selections.primary().head.0, len, "the other view follows");

    // A group without a tab of it gets a new tab on the same buffer.
    fs::write(root.join("b.cpp"), "int b;\n").unwrap();
    app.test_open(Buffer::open(&root.join("b.cpp")).unwrap());
    let log = app.groups.get(first).unwrap().active_tab().unwrap().buffer;
    let index = app.group().find(log).unwrap();
    app.group_mut().active = index;
    app.press(KeyCode::Char('w'), cmd());
    assert_eq!(app.group().tabs.len(), 1);
    open_log(&mut app, "demo");
    assert_eq!(app.group().active_tab().unwrap().buffer, log, "the same buffer");
    assert_eq!(app.buffers.iter().filter(|(_, b)| b.log.is_some()).count(), 1);
}

/// "Tools: Clear Log" empties the active log tab and its ring; outside a log tab it says how.
#[tokio::test(flavor = "multi_thread")]
async fn tools_clear_log_empties_the_tab() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), false);
    push_lines(&app, &demo(), 0, 3);
    open_log(&mut app, "demo");
    run_command(&mut app, "Tools: Clear Log");
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "");
    assert!(app.logs.texts(&demo()).is_empty());
    push_lines(&app, &demo(), 3, 1);
    assert!(app.settle(|a| tab_lines(a) == ["line 3"]).await);
    // Undo doesn't bring the cleared lines back.
    app.press(KeyCode::Char('z'), cmd());
    assert_eq!(tab_lines(&app), ["line 3"]);

    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    run_command(&mut app, "Tools: Clear Log");
    assert!(app.message.as_ref().is_some_and(|m| m.text.contains("Tools: Open Log")));
    assert_eq!(app.logs.texts(&demo()), ["line 3"]);
}

/// Lines the ring drops leave the tab too (bounded as the ring is); a view off the tail stays
/// on its text.
#[tokio::test(flavor = "multi_thread")]
async fn tools_the_rings_dropped_lines_leave_the_tab() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.logs.set_caps(&demo(), 5, 1 << 20);
    push_lines(&app, &demo(), 0, 3);
    open_log(&mut app, "demo");
    push_lines(&app, &demo(), 3, 10);
    assert!(
        app.settle(|a| tab_lines(a).first().is_some_and(|l| l == "line 8"))
            .await
    );
    assert_eq!(tab_lines(&app), ["line 8", "line 9", "line 10", "line 11", "line 12"]);
    // The cursor on "line 11", then two more lines: it stays on "line 11".
    for _ in 0..2 {
        app.press(KeyCode::Up, KeyModifiers::NONE);
    }
    assert_eq!(tab_lines(&app)[cursor_and_top(&app).0], "line 11");
    push_lines(&app, &demo(), 13, 2);
    assert!(
        app.settle(|a| tab_lines(a).first().is_some_and(|l| l == "line 10"))
            .await
    );
    assert_eq!(tab_lines(&app).len(), 5);
    assert_eq!(tab_lines(&app)[cursor_and_top(&app).0], "line 11");
}

/// Many lines arriving in one turn are one change set (one revision), no undo step.
#[tokio::test(flavor = "multi_thread")]
async fn tools_many_appends_in_one_turn_are_one_change() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.logs.push(&demo(), Level::Info, "first");
    open_log(&mut app, "demo");
    let (appends, revision) = (app.log_tabs.appends, app.active_buffer().unwrap().revision());
    push_lines(&app, &demo(), 0, 1000);
    app.pump();
    assert_eq!(app.log_tabs.appends, appends + 1);
    assert_eq!(app.active_buffer().unwrap().revision(), revision + 1);
    assert_eq!(tab_lines(&app).len(), 1001);
    assert!(!app.active_buffer().unwrap().is_dirty());
    app.pump();
    assert_eq!(app.log_tabs.appends, appends + 1, "nothing new: no change");
}

/// Log tabs aren't saved in the session: a restart shows the file tab only.
#[tokio::test(flavor = "multi_thread")]
async fn tools_sessions_never_restore_a_log_tab() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = open_app(dir.path(), &fake, Logs::default(), true);
    assert!(app.settle(|a| !a.session.loading()).await);
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    push_lines(&app, &demo(), 0, 3);
    open_log(&mut app, "demo");
    app.press(KeyCode::Char('\\'), cmd());
    assert_eq!(app.groups.len(), 2);
    app.test_execute(crate::command::Command::Quit);
    assert!(app.quitting());
    app.shutdown().await;
    drop(app);
    // The file holds the file tab only (a log tab's empty path would be dropped on restore
    // anyway, so read what was written).
    let state = workspace::normalize(dir.path()).join("state");
    let session = crate::session::load(&state, &root).unwrap().expect("a session");
    let tabs: Vec<_> = session.groups.iter().flat_map(|g| &g.tabs).collect();
    assert_eq!(tabs.len(), 1, "{tabs:?}");
    assert!(matches!(tabs[0], crate::session::TabState::Text { path, .. } if path == Path::new("a.cpp")));

    let (mut app, _) = open_app(dir.path(), &fake, Logs::default(), true);
    assert!(app.settle(|a| !a.session.loading()).await);
    assert_eq!(app.session.restored, 1);
    assert!(app.buffers.iter().all(|(_, b)| b.log.is_none()));
    let names: Vec<String> = app
        .groups
        .ids()
        .into_iter()
        .flat_map(|g| {
            app.groups
                .get(g)
                .unwrap()
                .tabs
                .iter()
                .filter_map(|t| t.buffer())
                .collect::<Vec<_>>()
        })
        .map(|id| app.buffers.get(id).unwrap().name())
        .collect();
    assert_eq!(names, ["a.cpp"]);
    app.shutdown_servers().await;
}

/// Waits out the start's own events (listing, git, …) with real turns: none for 300 ms.
async fn quiet(app: &mut App) {
    let mut turns = 0;
    while app.test_turn(Duration::from_millis(300)).await {
        turns += 1;
        assert!(turns < 200, "the app never went quiet");
    }
}

/// New lines wake the loop by themselves (`test_turn`: no polling, as `run`) while a log tab is
/// open, again after each `LOG_FRAME`; with no log tab the store stays unarmed after its first
/// wake, so lines nobody shows cost no turns.
#[tokio::test(flavor = "multi_thread")]
async fn tools_new_lines_wake_the_loop_only_for_log_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = open_app(dir.path(), &fake, Logs::default(), false);
    quiet(&mut app).await;
    push_lines(&app, &demo(), 0, 1);
    assert!(app.test_turn(Duration::from_secs(2)).await, "the store's first wake");
    assert_eq!(app.log_tabs.deadline(), None, "no log tab: no rearm due");
    push_lines(&app, &demo(), 1, 1);
    assert!(!app.test_turn(Duration::from_millis(300)).await, "unarmed: no wake");

    open_log(&mut app, "demo");
    assert_eq!(tab_lines(&app), ["line 0", "line 1"]);
    for n in 2..5 {
        // Let the throttle's timer rearm the store, then a line must wake the loop.
        while app.log_tabs.deadline().is_some() {
            assert!(app.test_turn(Duration::from_secs(1)).await, "the rearm timer");
        }
        push_lines(&app, &demo(), n, 1);
        let line = format!("line {n}");
        let mut turns = 0;
        while !tab_has(&app, &line) {
            assert!(app.test_turn(Duration::from_secs(2)).await, "{line} woke the loop");
            turns += 1;
            assert!(turns < 50);
        }
    }
}

/// A flood of lines (a protocol trace) costs about two loop turns per `LOG_FRAME` (the wake and
/// the rearm), not one per line, and each turn takes in what arrived as one change set.
#[tokio::test(flavor = "multi_thread")]
async fn tools_a_flood_of_lines_costs_turns_per_frame_not_per_line() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = open_app(dir.path(), &fake, Logs::default(), false);
    app.logs.push(&demo(), Level::Info, "start");
    open_log(&mut app, "demo");
    quiet(&mut app).await;
    let appends = app.log_tabs.appends;
    let logs = app.logs.clone();
    let start = std::time::Instant::now();
    // 300 bursts of 10 lines, 1 ms or more apart.
    let pusher = std::thread::spawn(move || {
        for i in 0..3000 {
            logs.push(&demo(), Level::Info, &format!("line {i}"));
            if i % 10 == 9 {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });
    let mut turns = 0u64;
    while !(pusher.is_finished() && tab_lines(&app).len() == 3001) {
        if app.test_turn(Duration::from_millis(200)).await {
            turns += 1;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{} lines",
            tab_lines(&app).len()
        );
    }
    let frames = (start.elapsed().as_millis() / 20) as u64; // `tools::LOG_FRAME`
    // About 2 per frame; one per burst (300) without the throttle.
    assert!(turns <= 3 * frames + 10, "{turns} turns in {frames} frames");
    assert!(app.log_tabs.appends - appends <= turns, "one change set per turn");
    assert_eq!(tab_lines(&app)[3000], "line 2999");
}

/// Every command run on a log tab leaves it alone: no file written or staged, no diff, history
/// or explorer reveal of its empty path, its text unchanged (but by Clear Log) and never dirty.
#[tokio::test(flavor = "multi_thread")]
async fn tools_every_command_on_a_log_tab_leaves_it_alone() {
    use crate::command::Command;
    use crate::milestone10_tests::{dirs, open_file_app, settle_git};
    let (_d, root, state) = dirs();
    crate::git::test_repo::init(&root);
    fs::write(root.join("f.txt"), "one\n").unwrap();
    crate::git::test_repo::commit_all(&root, "one");
    fs::write(root.join("f.txt"), "two\n").unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    app.log_dir = state.join("logs");
    app.config_dir = state.join("config");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    push_lines(&app, &demo(), 0, 3);
    open_log(&mut app, "demo");
    let log = app.group().active_tab().unwrap().buffer;
    let text = app.buffers.get(log).unwrap().rope.to_string();
    let skip = |c: &Command| {
        matches!(
            c,
            Command::Quit
                | Command::CloseTab
                | Command::NextTab
                | Command::PrevTab
                | Command::OpenUserSettings
                | Command::OpenLogFile
                | Command::ClearToolLog
        )
    };
    let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
    for c in Command::all().into_iter().filter(|c| !skip(c)) {
        app.test_execute(c);
        for g in app.groups.ids() {
            for tab in &app.groups.get(g).unwrap().tabs {
                if let Some(d) = tab.diff() {
                    assert!(!d.path.as_os_str().is_empty(), "{c:?} opened a diff of the log");
                }
            }
        }
        assert!(
            app.buffers
                .iter()
                .all(|(_, b)| b.log.is_some() || !b.path.as_os_str().is_empty()),
            "{c:?}"
        );
        // No message about a file without a name ("Not in a git repository", " has no file
        // history…", …): file commands pass a log tab by.
        if let Some(m) = app.message.take() {
            assert!(
                !m.text.starts_with(' ') && m.text != "Not in a git repository",
                "{c:?}: {}",
                m.text
            );
        }
        app.prompt = None;
        app.palette = None;
        while app.groups.len() > 1 {
            app.test_execute(Command::FocusGroup(2));
            app.test_execute(Command::CloseTab);
        }
        let index = app.group().find(log).expect("the log tab");
        app.group_mut().active = index;
        app.test_execute(Command::FocusGroup(1));
        draw(&mut app, &mut term);
    }
    assert!(app.settle(|a| !a.buffers.iter().any(|(_, b)| b.save_in_flight())).await);
    let buf = app.buffers.get(log).unwrap();
    assert_eq!(buf.rope.to_string(), text);
    assert!(!buf.is_dirty());
    assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), "two\n");
    assert!(!root.join(".jed").exists(), "no file history");
    assert!(
        settle_git(&mut app, |s| s
            .entry(Path::new("f.txt"))
            .is_some_and(|e| e.staged.is_none()))
        .await,
        "nothing staged"
    );
    // Go Back from a file never lands on (or records) the log.
    assert!(app.nav.history.entries().iter().all(|e| !e.path.as_os_str().is_empty()));
}

/// A `.workspace` `[lsp.plaintext]` server isn't started for a log tab (Plain Text, no file).
#[tokio::test(flavor = "multi_thread")]
async fn tools_a_log_tab_starts_no_language_server() {
    let dir = tempfile::tempdir().unwrap();
    let base = workspace::normalize(dir.path());
    let root = base.join("proj");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.cpp"), "int a;\n").unwrap();
    fs::write(
        root.join(".workspace"),
        "[lsp.plaintext]\ncommand = \"my-text-server\"\n",
    )
    .unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let trust = TrustStore::load(base.join("state/trust.toml"));
    let mut app = App::new(Workspace::load(root.clone()), trust, None, true);
    app.lsp.set_launcher(fake.clone());
    app.test_start();
    app.press(KeyCode::Char('t'), KeyModifiers::NONE);
    assert_eq!(app.trust, crate::workspace::trust::Trust::Trusted);
    push_lines(&app, &demo(), 0, 1);
    open_log(&mut app, "demo");
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await
    );
    assert_eq!(fake.launches(), ["clangd"]);
    assert!(app.lsp.server_for(Language::PlainText).is_none());
    app.shutdown_servers().await;
}
