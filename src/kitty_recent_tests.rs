//! The `kitty_recent` task end to end: Ctrl+R in a jed marks the workspaces other jeds have open
//! (found through kitty's remote control socket, a fake one here) and Enter on a marked one sends
//! `focus-window` for its window (`kitty_recent_enter_on_an_open_workspace_focuses_its_window`),
//! on an unmarked one `launch` of `jed <path>` in a new OS window
//! (`kitty_recent_enter_on_a_closed_workspace_launches_jed`); with the socket gone, refusing or
//! silent Enter shows the old "To open …, run: jed …" message with the reason and the failure is
//! in the log (`kitty_recent_failures_fall_back_to_the_message`); without `KITTY_LISTEN_ON` no
//! call is made and nothing is marked (`kitty_recent_without_a_socket_makes_no_calls`); a kitty
//! that never answers doesn't block the UI thread (`kitty_recent_a_silent_kitty_does_not_block_the_ui`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};
use tracing_subscriber::EnvFilter;

use crate::app::App;
use crate::logs::{Logs, Tool};
use crate::termctl::Env;
use crate::termctl::kitty::fake::{Mode, serve, short_dir};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

/// `kitty`'s `ls` data (a JSON string): one window per `(window id, workspace)`, spread over
/// two OS windows, plus a shell whose title says jed and one without the var.
fn ls(windows: &[(u64, &Path)]) -> Value {
    let jed: Vec<Value> = windows
        .iter()
        .map(|(id, p)| json!({"id": id, "title": "jed", "user_vars": {"jed": "1", "jed_workspace": p}}))
        .collect();
    let (first, rest) = jed.split_at(jed.len().min(1));
    Value::String(
        json!([
            {"id": 1, "tabs": [{"id": 1, "windows": first}]},
            {"id": 2, "tabs": [
                {"id": 2, "windows": rest},
                {"id": 3, "windows": [{"id": 90, "title": "jed", "cwd": "/", "user_vars": {}}]}]}
        ])
        .to_string(),
    )
}

struct Setup {
    root: PathBuf,
    open: PathBuf,
    closed: PathBuf,
    state: PathBuf,
    _dir: tempfile::TempDir,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let base = workspace::normalize(dir.path());
    let (root, open, closed, state) = (
        base.join("here"),
        base.join("open"),
        base.join("closed"),
        base.join("state"),
    );
    for d in [&root, &open, &closed, &state] {
        fs::create_dir_all(d).unwrap();
    }
    // Most recent first: `open` then `closed`.
    fs::write(
        crate::recent::file(&state),
        format!(
            "[[workspace]]\npath = {:?}\n[[workspace]]\npath = {:?}\n",
            open.display().to_string(),
            closed.display().to_string()
        ),
    )
    .unwrap();
    Setup {
        root,
        open,
        closed,
        state,
        _dir: dir,
    }
}

async fn started(s: &Setup, listen_on: Option<String>) -> App {
    let ws = Workspace::load(s.root.clone());
    let trust = TrustStore::load(s.state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.state_dir = Some(s.state.clone());
    app.test_termctl_env(Env {
        kitty_listen_on: listen_on,
    });
    app.test_start();
    assert!(app.settle(|a| a.recent.recorded == 1).await, "recorded");
    app
}

fn unix(path: &Path) -> Option<String> {
    Some(format!("unix:{}", path.display()))
}

/// Ctrl+R and waits: the rows as (label, keys).
async fn ctrl_r(app: &mut App) -> Vec<(String, Option<String>)> {
    app.press(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app.settle(|a| a.recent.listing_running == 0).await, "listed");
    let p = app.palette.as_ref().expect("picker");
    p.rows.iter().map(|r| (r.label.clone(), r.keys.clone())).collect()
}

fn message(app: &App) -> String {
    app.message.as_ref().unwrap().text.clone()
}

fn cmds(log: &std::sync::Mutex<Vec<Value>>) -> Vec<String> {
    log.lock()
        .unwrap()
        .iter()
        .map(|v| v["cmd"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_recent_enter_on_an_open_workspace_focuses_its_window() {
    let s = setup();
    // The workspace's window is in kitty's second OS window, not the first.
    let (sock, log) = serve(Mode::Normal, ls(&[(77, Path::new("/elsewhere")), (41, &s.open)]));
    let mut app = started(&s, unix(&sock)).await;
    let rows = ctrl_r(&mut app).await;
    assert_eq!(
        rows,
        [
            ("open".to_string(), Some("open".to_string())),
            ("closed".to_string(), None)
        ]
    );
    assert_eq!(cmds(&log), ["ls"]);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| a.recent.opening_running == 0).await);
    assert!(message(&app).starts_with("Switched to "), "{}", message(&app));
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[1]["cmd"], "focus-window");
    assert_eq!(log[1]["payload"], json!({"match": "id:41"}));
    assert_eq!(log[1]["version"], json!([0, 14, 2]));
    assert_eq!(app.workspace.root, s.root, "jed stays on its workspace");
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_recent_enter_on_a_closed_workspace_launches_jed() {
    let s = setup();
    let (sock, log) = serve(Mode::Normal, ls(&[(41, &s.open)]));
    let mut app = started(&s, unix(&sock)).await;
    ctrl_r(&mut app).await;
    app.type_text("closed");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| a.recent.opening_running == 0).await);
    assert!(message(&app).contains("in a new window"), "{}", message(&app));
    let log = log.lock().unwrap();
    assert_eq!(log[1]["cmd"], "launch");
    let path = s.closed.to_string_lossy().to_string();
    assert_eq!(
        log[1]["payload"],
        json!({"type": "os-window", "cwd": path, "args": ["jed", path]})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_recent_failures_fall_back_to_the_message() {
    let s = setup();
    // The same flow against a refusing, a closing and a vanished kitty.
    let (refuse, _) = serve(Mode::Refuse, ls(&[]));
    let (close, _) = serve(Mode::Close, ls(&[]));
    let gone = short_dir().join("nothing-here");
    // The log: `tracing` on this thread, into the "jed" tool log.
    let logs = Logs::default();
    let (subscriber, _handle) = crate::logging::subscriber(EnvFilter::new("info"), std::io::sink, logs.clone());
    let _second = tracing::Dispatch::new(tracing_subscriber::registry());
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();
    for (sock, why) in [
        (refuse, "remote control refused"),
        (close, "no kitty response"),
        (gone, "can't connect"),
    ] {
        let mut app = started(&s, unix(&sock)).await;
        app.logs = logs.clone();
        let rows = ctrl_r(&mut app).await;
        assert!(rows.iter().all(|r| r.1.is_none()), "no marks without a list: {rows:?}");
        app.press(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settle(|a| a.recent.opening_running == 0).await);
        let m = message(&app);
        assert!(m.starts_with("To open ") && m.contains("run: jed "), "{m}");
        assert!(m.contains(&format!("run: jed {} (", s.open.display())), "{m}");
        assert!(m.ends_with(')') && m.contains(why), "the reason is appended: {m}");
    }
    let jed = logs.texts(&Tool::Jed).join("\n");
    assert!(jed.contains("terminal window not opened"), "{jed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_recent_without_a_socket_makes_no_calls() {
    let s = setup();
    // A listening socket nobody was told about.
    let (_sock, log) = serve(Mode::Normal, ls(&[(41, &s.open)]));
    let mut app = started(&s, None).await;
    let rows = ctrl_r(&mut app).await;
    assert!(rows.iter().all(|r| r.1.is_none()), "{rows:?}");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    let m = message(&app);
    assert!(
        m.starts_with("To open ") && m.ends_with(&format!("run: jed {}", s.open.display())),
        "{m}"
    );
    assert_eq!(app.recent.opening_running, 0);
    assert!(cmds(&log).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_recent_a_silent_kitty_does_not_block_the_ui() {
    let s = setup();
    let (sock, _log) = serve(Mode::Silent, ls(&[]));
    let mut app = started(&s, unix(&sock)).await;
    let t = Instant::now();
    app.press(KeyCode::Char('r'), KeyModifiers::CONTROL);
    // The listing waits on kitty for up to 2 s; keys are handled meanwhile.
    app.type_text("x");
    assert!(app.recent.listing_running == 1, "still waiting on kitty");
    assert!(t.elapsed() < Duration::from_millis(500), "the keys were not held up");
    assert!(app.settle(|a| a.recent.listing_running == 0).await, "gives up");
    assert!(t.elapsed() < Duration::from_secs(5));
    let waited = t.elapsed();
    assert!(
        waited >= Duration::from_millis(1500),
        "gave up on kitty, not on a closed socket: {waited:?}"
    );
    // The picker still opens, without marks; Enter waits on kitty off the UI thread too, then
    // falls back to the message.
    let p = app.palette.as_ref().expect("picker");
    assert!(p.rows.iter().all(|r| r.keys.is_none()));
    let t = Instant::now();
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.recent.opening_running, 1);
    assert!(t.elapsed() < Duration::from_millis(500), "Enter was not held up");
    assert!(app.settle(|a| a.recent.opening_running == 0).await, "gives up");
    let m = message(&app);
    assert!(m.starts_with("To open ") && m.contains("in time"), "{m}");
}
