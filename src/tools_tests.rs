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
//!
//! tools-c, the Tools view: Cmd+Shift+U, "Tools: Show" and the activity bar's icon show it with
//! the keys; it lists the language server (languages, state, progress, an error badge, failed
//! after a crash), task runs with their status and "jed"; the selection stays on its row by
//! key; Enter / a click open a log tab, a task row focuses its panel tab; many rows get the side
//! lists' scroll bar; error lines redraw it while shown; the session restores it.

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

// ---- tools-c: the Tools view ----

use crate::app::Focus;
use crate::command::Command;
use crate::layout::{PanelTab, ScrollTarget, SidePanel};
use crate::tasks::RunId;
use crate::tools_view::RowKey;
use crossterm::event::{MouseButton, MouseEventKind};

const TASKS: &str = "[tasks.ok]\ncommand = \"true\"\n\n[tasks.bad]\ncommand = \"exit 3\"\n";

/// `open_app` with the user's `tasks.toml` holding `tasks` (`ok`: `true`, `bad`: `exit 3`).
fn tools_app(dir: &Path, fake: &Arc<FakeLauncher>, state: bool) -> (App, PathBuf) {
    let base = workspace::normalize(dir);
    let root = base.join("proj");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.cpp"), "int a;\n").unwrap();
    let config = base.join("state/config");
    fs::create_dir_all(&config).unwrap();
    let path = config.join(crate::tasks::templates::FILE_NAME);
    fs::write(&path, TASKS).unwrap();
    let trust = TrustStore::load(base.join("state/trust.toml"));
    let mut app = App::new(Workspace::load(root.clone()), trust, None, true);
    app.lsp.set_launcher(fake.clone());
    app.config_dir = config;
    app.tasks.user = crate::tasks::templates::load(&path).tasks;
    if state {
        app.state_dir = Some(base.join("state"));
    }
    app.test_start();
    // Room for a server's row with its progress.
    app.ui.side_width = 40;
    (app, root)
}

/// Opens `a.cpp` and waits for the fake clangd to run.
async fn with_clangd(app: &mut App, root: &Path) {
    app.test_open(Buffer::open(&root.join("a.cpp")).unwrap());
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await
    );
}

/// Runs task `name` and waits for it to end (and its end note).
async fn run_ended(app: &mut App, name: &str) -> RunId {
    let id = app.run_task(name).unwrap();
    assert!(
        app.settle(|a| {
            let run = a.tasks.get(id).unwrap();
            crate::tasks::end_note(&run.name, &run.status())
                .is_some_and(|n| run.screen().screen().contents().contains(&n))
        })
        .await
    );
    id
}

fn terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(100, 30)).unwrap()
}

/// Draws, then each drawn row of the Tools view's list (trailing blanks trimmed).
fn tools_rows(app: &mut App, term: &mut Terminal<TestBackend>) -> Vec<String> {
    draw(app, term);
    let hit = app.hits.tools.clone().expect("the Tools view drawn");
    (0..hit.rows.len() as u16)
        .map(|i| {
            (hit.list.x..hit.list.x + hit.list.width)
                .map(|x| term.backend().buffer()[(x, hit.list.y + i)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// The cell where `text` starts on the drawn row showing `key`.
fn cell_of(app: &App, term: &Terminal<TestBackend>, key: &RowKey, text: &str) -> ratatui::buffer::Cell {
    let hit = app.hits.tools.as_ref().unwrap();
    let i = hit.rows.iter().position(|k| k == key).expect("the row drawn");
    let y = hit.list.y + i as u16;
    let row: Vec<String> = (hit.list.x..hit.list.x + hit.list.width)
        .map(|x| term.backend().buffer()[(x, y)].symbol().to_string())
        .collect();
    let at = (0..row.len())
        .find(|&i| row[i..].concat().starts_with(text))
        .unwrap_or_else(|| panic!("{text:?} in {:?}", row.concat()));
    term.backend().buffer()[(hit.list.x + at as u16, y)].clone()
}

fn fg(app: &App, key: &str) -> ratatui::style::Color {
    app.theme.get(key).fg.unwrap()
}

fn clangd_row() -> RowKey {
    RowKey::Server("clangd".into())
}

/// Cmd+Shift+U shows the view with the keys: the language server (its languages and state,
/// its progress while it has some, its log's error count in a badge, failed once it crashed),
/// each task run with its status, and "jed" last, each mark in its state's colour.
#[tokio::test(flavor = "multi_thread")]
async fn tools_view_lists_servers_tasks_and_jed_with_their_states() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = tools_app(dir.path(), &fake, false);
    with_clangd(&mut app, &root).await;
    let ok = run_ended(&mut app, "ok").await;
    let bad = run_ended(&mut app, "bad").await;
    app.press(KeyCode::Char('u'), cmd() | KeyModifiers::SHIFT);
    assert!(app.ui.side_visible && app.ui.side_panel == SidePanel::Tools);
    assert_eq!(app.ui.focus, Focus::SidePanel);
    assert_eq!(
        app.tools_view.selected_key(),
        Some(&clangd_row()),
        "the first row selected"
    );

    let mut term = terminal();
    let rows = tools_rows(&mut app, &mut term);
    assert_eq!(
        rows,
        [
            " ● clangd  C++ · running",
            " ✓ ok  task · exited 0",
            " ✗ bad  task · exited 3",
            " ≡ jed  jed's own log",
        ]
    );
    assert_eq!(
        app.hits.tools.as_ref().unwrap().rows[1..3],
        [RowKey::Task(ok), RowKey::Task(bad)]
    );
    assert_eq!(cell_of(&app, &term, &clangd_row(), "●").fg, fg(&app, "tools.running"));
    assert_eq!(
        cell_of(&app, &term, &RowKey::Task(ok), "✓").fg,
        fg(&app, "task.succeeded")
    );
    assert_eq!(
        cell_of(&app, &term, &RowKey::Task(bad), "✗").fg,
        fg(&app, "task.failed")
    );
    // The panel's title row: "TOOLS".
    let side = app.hits.regions.side.unwrap();
    let title: String = (side.x..side.x + side.width)
        .map(|x| term.backend().buffer()[(x, side.y)].symbol().to_string())
        .collect();
    assert!(title.contains("TOOLS"), "{title}");

    // Progress replaces the state while it lasts.
    let notify = |app: &App, method: &str, params: serde_json::Value| {
        app.lsp.server_for(Language::Cpp).unwrap().notify(method, params);
    };
    notify(
        &app,
        "test/progress",
        json!({ "token": "i", "value": { "kind": "begin", "title": "Indexing", "percentage": 40 } }),
    );
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).unwrap().progress.latest().is_some())
            .await
    );
    assert_eq!(tools_rows(&mut app, &mut term)[0], " ● clangd  C++ · Indexing 40%");

    // An error line in its log: a badge at the right edge.
    notify(&app, "test/log", json!({ "type": 1, "message": "index broken" }));
    assert!(app.settle(|a| a.logs.errors(&clangd()) == 1).await);
    let row = tools_rows(&mut app, &mut term)[0].clone();
    assert!(row.ends_with(" 1"), "{row:?}");
    let badge = cell_of(&app, &term, &clangd_row(), " 1 ");
    assert_eq!(badge.bg, app.theme.get("tools.errorBadge").bg.unwrap());

    // A crash: failed, its progress gone; its row (and its log's badge) stays.
    notify(&app, "test/crash", serde_json::Value::Null);
    assert!(
        app.settle(|a| matches!(a.lsp.server_for(Language::Cpp).unwrap().state, State::Failed))
            .await
    );
    let row = tools_rows(&mut app, &mut term)[0].clone();
    assert!(row.starts_with(" ✗ clangd  C++ · failed"), "{row:?}");
    assert!(row.ends_with(" 2"), "the crash is an error line too: {row:?}");
    assert_eq!(cell_of(&app, &term, &clangd_row(), "✗").fg, fg(&app, "tools.failed"));
    app.shutdown_servers().await;
}

/// A server whose language no longer resolves to it (a `.workspace` change) stops, and its row
/// stays — as exited, in the exited colour (its log remains).
#[tokio::test(flavor = "multi_thread")]
async fn tools_a_stopped_server_shows_as_exited() {
    let dir = tempfile::tempdir().unwrap();
    let base = workspace::normalize(dir.path());
    let root = base.join("proj");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.cpp"), "int a;\n").unwrap();
    // A trusted `.workspace` server, so replacing it by the built-in one stops a server.
    let text = "[lsp.cpp]\ncommand = \"clangd\"\n";
    fs::write(root.join(".workspace"), text).unwrap();
    let config = crate::workspace::config::WorkspaceConfig::parse(text).unwrap();
    let mut trust = TrustStore::load(base.join("state/trust.toml"));
    trust.record(&root, &config, true);
    trust.write().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = App::new(
        Workspace::load(root.clone()),
        TrustStore::load(base.join("state/trust.toml")),
        None,
        true,
    );
    app.lsp.set_launcher(fake.clone());
    app.ui.side_width = 40;
    app.test_start();
    with_clangd(&mut app, &root).await;
    app.test_execute(Command::ShowTools);
    let mut term = terminal();
    assert_eq!(tools_rows(&mut app, &mut term)[0], " ● clangd  C++ · running");
    // The only tab closes and `.workspace` no longer configures a server: the running
    // workspace server stops and stays in the view.
    app.test_execute(Command::CloseTab);
    fs::write(root.join(".workspace"), "[settings]\nrender_whitespace = \"none\"\n").unwrap();
    assert!(app.settle(|a| a.lsp.stopped().len() == 1).await);
    let rows = tools_rows(&mut app, &mut term);
    assert_eq!(rows, [" – clangd  C++ · exited", " ≡ jed  jed's own log"]);
    assert_eq!(cell_of(&app, &term, &clangd_row(), "–").fg, fg(&app, "tools.exited"));
    app.shutdown_servers().await;
}

/// The selection stays on its row by key while rows come and go; a row that leaves hands it to
/// the row taking its place.
#[tokio::test(flavor = "multi_thread")]
async fn tools_the_selection_is_kept_by_key() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = tools_app(dir.path(), &fake, false);
    let ok = run_ended(&mut app, "ok").await;
    app.test_execute(Command::ShowTools);
    let mut term = terminal();
    assert_eq!(tools_rows(&mut app, &mut term).len(), 2);
    app.press(KeyCode::End, KeyModifiers::NONE);
    assert_eq!(app.tools_view.selected_key(), Some(&RowKey::Jed));
    // A server and a run appear before it: still on "jed".
    with_clangd(&mut app, &root).await;
    let bad = run_ended(&mut app, "bad").await;
    app.test_execute(Command::ShowTools);
    assert_eq!(tools_rows(&mut app, &mut term).len(), 4);
    assert_eq!(app.tools_view.selected_key(), Some(&RowKey::Jed));
    let selected = |app: &App| app.tools_view.selected_row(&app.tools_rows());
    assert_eq!(selected(&app), Some(3));
    // Up to `ok`, whose tab is then closed: the selection moves to `bad`, now in its place.
    app.press(KeyCode::Up, KeyModifiers::NONE);
    app.press(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(app.tools_view.selected_key(), Some(&RowKey::Task(ok)));
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.ui.panel_tab, PanelTab::Task(ok));
    app.test_execute(Command::CloseTask);
    assert!(app.tasks.get(ok).is_none());
    let rows = tools_rows(&mut app, &mut term);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(app.tools_view.selected_key(), Some(&RowKey::Task(bad)));
    // Home / PageDown / PageUp.
    app.test_execute(Command::ShowTools);
    app.press(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(app.tools_view.selected_key(), Some(&clangd_row()));
    app.press(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(app.tools_view.selected_key(), Some(&RowKey::Task(bad)));
    app.press(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(app.tools_view.selected_key(), Some(&clangd_row()));
    app.press(KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(app.tools_view.selected_key(), Some(&RowKey::Jed));
    app.press(KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(app.tools_view.selected_key(), Some(&clangd_row()));
    // Esc: back to the editor.
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.ui.focus, Focus::Editor);
    app.shutdown_servers().await;
}

/// Enter on a server's row opens its log tab, focused; a click on "jed" selects it and opens
/// jed's log.
#[tokio::test(flavor = "multi_thread")]
async fn tools_enter_and_a_click_open_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, root) = tools_app(dir.path(), &fake, false);
    with_clangd(&mut app, &root).await;
    app.test_execute(Command::ShowTools);
    assert_eq!(app.tools_view.selected_key(), Some(&clangd_row()));
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.log.as_ref().map(|l| l.tool.clone()), Some(clangd()));
    assert_eq!(buf.name(), "clangd (log)");
    assert_eq!(app.ui.focus, Focus::Editor);
    assert!(tab_has(&app, "fake ready"), "{:?}", tab_lines(&app));

    let mut term = terminal();
    tools_rows(&mut app, &mut term);
    let hit = app.hits.tools.clone().unwrap();
    let y = hit.list.y + hit.rows.iter().position(|k| *k == RowKey::Jed).unwrap() as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 3, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), hit.list.x + 3, y);
    assert_eq!(app.tools_view.selected_key(), Some(&RowKey::Jed));
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.log.as_ref().map(|l| l.tool.clone()), Some(Tool::Jed));
    assert_eq!(app.ui.focus, Focus::Editor);
    // Opened again: the same tab.
    let tabs = app.group().tabs.len();
    app.test_execute(Command::ShowTools);
    app.press(KeyCode::Home, KeyModifiers::NONE);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.group().tabs.len(), tabs, "clangd's tab reused");
    app.shutdown_servers().await;
}

/// A task run's row shows and focuses its panel tab (no log tab of its own).
#[tokio::test(flavor = "multi_thread")]
async fn tools_a_task_row_focuses_its_panel_tab() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = tools_app(dir.path(), &fake, false);
    let ok = run_ended(&mut app, "ok").await;
    let bad = run_ended(&mut app, "bad").await;
    app.test_execute(Command::TogglePanel);
    assert!(!app.ui.panel_visible);
    app.test_execute(Command::ShowTools);
    assert_eq!(
        app.tools_view.selected_key(),
        Some(&RowKey::Task(ok)),
        "no server: a run first"
    );
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.ui.panel_visible);
    assert_eq!(app.ui.panel_tab, PanelTab::Task(ok));
    assert_eq!(app.ui.focus, Focus::Panel);
    assert!(app.buffers.iter().all(|(_, b)| b.log.is_none()), "no log tab");
    // A click on the other run's row.
    let mut term = terminal();
    app.test_execute(Command::ShowTools);
    tools_rows(&mut app, &mut term);
    let hit = app.hits.tools.clone().unwrap();
    let y = hit.list.y + hit.rows.iter().position(|k| *k == RowKey::Task(bad)).unwrap() as u16;
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.list.x + 3, y);
    assert_eq!(app.ui.panel_tab, PanelTab::Task(bad));
    assert_eq!(app.ui.focus, Focus::Panel);
}

/// Many rows (tools with a log): the list's scroll bar works as the other side lists' (thumb,
/// hover, drag, track press, wheel; the selection and focus unchanged), and the wheel over the
/// rows scrolls them.
#[tokio::test(flavor = "multi_thread")]
async fn tools_many_rows_get_a_scroll_bar() {
    use crate::todo_tests::side_scroll_bars::{List, drag_the_bar};
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = tools_app(dir.path(), &fake, false);
    for i in 0..30 {
        app.logs.push(&Tool::Server(format!("demo{i}")), Level::Info, "line");
    }
    app.test_execute(Command::ShowTools);
    let list = List {
        target: ScrollTarget::Side(SidePanel::Tools),
        rows: |a| a.tools_rows().len(),
        top: |a| a.tools_view.scroll(&a.tools_rows()),
        selection: |a| format!("{:?}", a.tools_view.selected_key()),
    };
    drag_the_bar(&mut app, &list);

    // The wheel over the rows.
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    app.press(KeyCode::Home, KeyModifiers::NONE);
    tools_rows(&mut app, &mut term);
    let hit = app.hits.tools.clone().unwrap();
    app.mouse(MouseEventKind::ScrollDown, hit.list.x + 2, hit.list.y + 2);
    assert_eq!(app.tools_view.scroll(&app.tools_rows()), 3);
    // End: the last row ("jed") scrolled into view.
    app.press(KeyCode::End, KeyModifiers::NONE);
    let rows = tools_rows(&mut app, &mut term);
    assert_eq!(rows.last().unwrap(), " ≡ jed  jed's own log");
}

/// The activity bar's fourth icon shows the view (with the keys); clicked again it hides the
/// side panel.
#[tokio::test(flavor = "multi_thread")]
async fn tools_the_activity_bar_icon_shows_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = tools_app(dir.path(), &fake, false);
    let mut term = terminal();
    draw(&mut app, &mut term);
    let &(y, panel) = app.hits.icons.last().unwrap();
    assert_eq!(panel, SidePanel::Tools);
    assert_eq!(term.backend().buffer()[(1, y)].symbol(), "≡");
    app.mouse(MouseEventKind::Down(MouseButton::Left), 1, y);
    assert!(app.ui.side_visible && app.ui.side_panel == SidePanel::Tools);
    assert_eq!(app.ui.focus, Focus::SidePanel);
    assert_eq!(
        app.tools_view.selected_key(),
        Some(&RowKey::Jed),
        "the first (only) row"
    );
    assert_eq!(tools_rows(&mut app, &mut term), [" ≡ jed  jed's own log"]);
    assert_eq!(term.backend().buffer()[(1, y)].fg, fg(&app, "activityBar.active"));
    app.mouse(MouseEventKind::Down(MouseButton::Left), 1, y);
    assert!(!app.ui.side_visible);
}

/// "Tools: Show" from the palette.
#[tokio::test(flavor = "multi_thread")]
async fn tools_show_from_the_palette() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = tools_app(dir.path(), &fake, false);
    app.ui.side_visible = false;
    run_command(&mut app, "Tools: Show");
    assert!(app.ui.side_visible && app.ui.side_panel == SidePanel::Tools);
    assert_eq!(app.ui.focus, Focus::SidePanel);
    assert_eq!(app.last_command, Some(Command::ShowTools));
}

/// With the view shown (and no log tab), new log lines wake the loop; only a changed error
/// count asks for a redraw, and the badge shows it.
#[tokio::test(flavor = "multi_thread")]
async fn tools_error_lines_redraw_the_shown_view() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = tools_app(dir.path(), &fake, false);
    push_lines(&app, &demo(), 0, 1);
    app.test_execute(Command::ShowTools);
    let mut term = terminal();
    tools_rows(&mut app, &mut term);
    quiet(&mut app).await;
    app.test_take_redraw();
    // Rearmed when the view appeared: an info line wakes the loop, nothing to redraw.
    push_lines(&app, &demo(), 1, 1);
    assert!(app.test_turn(Duration::from_secs(2)).await, "woke");
    assert!(!app.test_take_redraw(), "no badge changed");
    while app.log_tabs.deadline().is_some() {
        assert!(app.test_turn(Duration::from_secs(1)).await, "the rearm timer");
    }
    app.test_take_redraw();
    app.logs.push(&demo(), Level::Error, "broken");
    assert!(app.test_turn(Duration::from_secs(2)).await, "woke");
    assert!(app.test_take_redraw(), "the badge changed");
    let rows = tools_rows(&mut app, &mut term);
    let demo_row = rows
        .iter()
        .find(|r| r.contains("demo"))
        .expect("a row for the server's log");
    assert!(demo_row.ends_with(" 1"), "{rows:?}");
}

/// The side panel's view is in the session: the Tools view comes back.
#[tokio::test(flavor = "multi_thread")]
async fn tools_the_session_restores_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeLauncher::new(FakeOptions::default());
    let (mut app, _root) = tools_app(dir.path(), &fake, true);
    assert!(app.settle(|a| !a.session.loading()).await);
    app.test_execute(Command::ShowTools);
    app.test_execute(Command::Quit);
    assert!(app.quitting());
    app.shutdown().await;
    drop(app);
    let (mut app, _root) = tools_app(dir.path(), &fake, true);
    assert!(app.settle(|a| !a.session.loading()).await);
    assert_eq!(app.session.restored, 1);
    assert!(app.ui.side_visible && app.ui.side_panel == SidePanel::Tools);
    let mut term = terminal();
    assert_eq!(tools_rows(&mut app, &mut term), [" ≡ jed  jed's own log"]);
}
