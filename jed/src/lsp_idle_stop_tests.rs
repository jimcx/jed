//! End-to-end tests for `lsp_idle_stop-b` (PLAN.md "Stop idle language servers"): the idle clock
//! (driven with `test_set_last_input` / `test_lsp_idle_tick`, never a sleep), stopping every
//! running server after `idle_stop_minutes` without input (`shutdown` + `exit`, listed as idle
//! stopped, diagnostics kept), what does and doesn't reset the clock (file changes, server
//! notifications, losing the focus don't; a key, a paste, the focus coming back do), the next
//! input starting the server again with `didOpen` for every open buffer and a hover then
//! answered, a fresh publish replacing the kept diagnostics, `0` never stopping, a failed server
//! staying failed, a lowered value applying at the next check, and a live reload of the setting.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode as K, KeyModifiers as M};
use serde_json::json;

use crate::app::App;
use crate::config::settings;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::lsp::{Event as LspEvent, State};
use crate::milestone9_tests::{
    diag, lsp_project, marks, open, open_app, project, publish, put_cursor, show_hover, uri,
};
use crate::syntax::Language;

const MIN: Duration = Duration::from_secs(60);

fn later(minutes: u32) -> Instant {
    Instant::now() + MIN * minutes
}

fn didopens(fake: &FakeLauncher) -> usize {
    fake.methods().iter().filter(|m| *m == "textDocument/didOpen").count()
}

/// `a.cpp` and `b.cpp` open and synced with a fake server that published one diagnostic on `a.cpp`.
async fn two_files(dir: &Path) -> (App, Arc<FakeLauncher>, PathBuf) {
    let (mut app, fake, root) = lsp_project(dir, &[("a.cpp", "int a;\n"), ("b.cpp", "int b;\n")], false).await;
    open(&mut app, &root.join("b.cpp"));
    assert!(app.settle(|_| didopens(&fake) == 2).await);
    publish(
        &app,
        Language::Cpp,
        &root.join("a.cpp"),
        None,
        json!([diag(0, 0, 3, 1, "old")]),
    );
    assert!(app.settle(|a| marks(a, &root.join("a.cpp")).len() == 1).await);
    (app, fake, root)
}

fn errors(app: &App) -> usize {
    app.lsp.diagnostics.counts().of(crate::diagnostics::Severity::Error)
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_stops_then_restarts_on_input() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, root) = two_files(dir.path()).await;
    let a = root.join("a.cpp");
    // Not yet due.
    app.test_set_last_input(Instant::now());
    app.test_lsp_idle_tick(later(59));
    assert_eq!(app.lsp.servers().len(), 1);
    assert!(app.test_lsp_idle_deadline().is_some());

    app.test_lsp_idle_tick(later(61));
    assert!(app.lsp.servers().is_empty());
    assert_eq!(app.lsp.idle_stopped().len(), 1);
    assert!(app.lsp.stopped().is_empty(), "not an exit");
    assert!(app.test_lsp_idle_deadline().is_none());
    assert!(app.settle(|_| fake.exited() == 1).await);
    let methods = fake.methods();
    assert!(methods.iter().any(|m| m == "shutdown") && methods.iter().any(|m| m == "exit"));
    app.pump();
    assert_eq!(marks(&app, &a).len(), 1, "diagnostics kept");
    assert_eq!(errors(&app), 1);
    assert!(app.message.is_none(), "no notice");
    // A loop turn doesn't start it again by itself.
    app.pump();
    assert_eq!(fake.launches().len(), 1);

    // The next key starts it, with didOpen for both buffers.
    app.press(K::Right, M::NONE);
    assert!(app.lsp.idle_stopped().is_empty());
    assert!(app.settle(|_| didopens(&fake) == 4).await, "{:?}", fake.methods());
    assert_eq!(fake.launches().len(), 2);
    assert_eq!(fake.document(&uri(&a)).unwrap().text, "int a;\n");
    assert_eq!(marks(&app, &a)[0].2, "old", "kept until the new one publishes");

    // A hover pressed now is answered.
    fake.respond("textDocument/hover", json!({ "contents": "hello" }));
    put_cursor(&mut app, 1);
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);

    // Its first publish replaces the kept diagnostics.
    publish(&app, Language::Cpp, &a, None, json!([diag(0, 4, 5, 2, "new")]));
    assert!(
        app.settle(|x| marks(x, &a).iter().map(|m| m.2.as_str()).collect::<Vec<_>>() == ["new"])
            .await
    );
    assert_eq!(errors(&app), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_clock_resets() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, root) = two_files(dir.path()).await;
    let base = Instant::now() - MIN * 30;
    app.test_set_last_input(base);
    let due = app.test_lsp_idle_deadline().unwrap();
    assert_eq!(due, base + MIN * 60);

    // Not input: a file change, server traffic, a loop turn, losing the focus.
    app.test_fs_changed(vec![root.join("a.cpp")]);
    let server = app.lsp.server_for(Language::Cpp).unwrap().id;
    app.test_lsp_event(LspEvent::Notification {
        server,
        method: "$/progress".into(),
        params: json!({}),
    });
    app.pump();
    app.test_focus_lost();
    assert_eq!(app.test_lsp_idle_deadline(), Some(due));

    // Input: each moves the deadline.
    let events = [
        Event::Key(crossterm::event::KeyEvent::new(K::Right, M::NONE)),
        Event::Paste("x".into()),
        Event::FocusGained,
    ];
    for ev in events {
        app.test_set_last_input(base);
        app.handle_test_event(ev.clone());
        assert!(app.test_lsp_idle_deadline().unwrap() > due, "{ev:?}");
    }
}

/// A shell tab's output, like a file change or server traffic, isn't input; a mouse event is.
#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_terminal_output_is_not_input_but_a_mouse_event_is() {
    use crossterm::event::MouseEventKind;
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, _root) = lsp_project(dir.path(), &[("a.cpp", "int a;\n")], false).await;
    app.tasks.shell = jterm::testing::fake_shell();
    app.new_shell_in_editor();
    let base = Instant::now() - MIN * 30;
    app.test_set_last_input(base);
    let due = app.test_lsp_idle_deadline().unwrap();
    let screen = |a: &App| {
        a.groups
            .shell_tabs()
            .map(|s| s.run.screen().screen().contents())
            .collect::<String>()
    };
    app.group().active_shell().unwrap().run.write(b"echo idle-marker-xyz\n");
    assert!(app.settle(|a| screen(a).matches("idle-marker-xyz").count() >= 2).await);
    assert_eq!(app.test_lsp_idle_deadline(), Some(due), "output is not input");

    app.mouse(MouseEventKind::Moved, 5, 5);
    assert!(app.test_lsp_idle_deadline().unwrap() > due, "a mouse event is");
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_zero_never_stops() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        &[("a.cpp", "int a;\n"), (".workspace", "[lsp]\nidle_stop_minutes = 0\n")],
    );
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.cpp"));
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).and_then(|s| s.encoding()).is_some())
            .await
    );
    assert!(app.test_lsp_idle_deadline().is_none());
    app.test_lsp_idle_tick(later(100_000));
    assert_eq!(app.lsp.servers().len(), 1);
    assert!(app.lsp.idle_stopped().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_failed_stays_failed() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n")]);
    let fake = FakeLauncher::new(FakeOptions {
        missing: vec!["clangd".into()],
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.cpp"));
    assert!(
        app.settle(|a| a
            .lsp
            .server_for(Language::Cpp)
            .is_some_and(|s| matches!(s.state, State::Failed)))
            .await
    );
    assert!(app.test_lsp_idle_deadline().is_none(), "nothing to stop");
    let launches = fake.launches().len();
    app.test_lsp_idle_tick(later(120));
    assert_eq!(app.lsp.servers().len(), 1);
    assert!(app.lsp.idle_stopped().is_empty());
    app.press(K::Right, M::NONE);
    app.pump();
    assert!(matches!(app.lsp.servers()[0].state, State::Failed));
    assert_eq!(fake.launches().len(), launches, "not started again");
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_lowered_value_applies_at_the_next_check() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, _root) = lsp_project(dir.path(), &[("a.cpp", "int a;\n")], false).await;
    app.test_set_last_input(Instant::now());
    app.test_lsp_idle_tick(later(10));
    assert_eq!(app.lsp.servers().len(), 1, "60 min: not yet");
    app.workspace.user_lsp_idle_stop_minutes = Some(5);
    assert!(app.test_lsp_idle_deadline().unwrap() < later(6));
    app.test_lsp_idle_tick(later(10));
    assert!(app.lsp.servers().is_empty());
    assert_eq!(app.lsp.idle_stopped().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_live_reload_settings() {
    let dir = tempfile::tempdir().unwrap();
    let base = crate::workspace::normalize(dir.path());
    let config_dir = base.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    let user_file = config_dir.join(settings::FILE_NAME);
    fs::write(&user_file, "[settings]\n").unwrap();
    let root = base.join("ws");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".workspace"), "").unwrap();
    let path = root.join("a.txt");
    fs::write(&path, "x\n").unwrap();
    let loaded = settings::load(&user_file);
    let ws = crate::workspace::Workspace::load(root.clone()).with_user_settings(loaded.overrides);
    let trust = crate::workspace::trust::TrustStore::load(base.join("trust.toml"));
    let initial = crate::app::InitialFile {
        buffer: crate::buffer::Buffer::open(&path).unwrap(),
        goto: None,
    };
    let mut app = App::new(ws, trust, Some(initial), true);
    app.config_dir = config_dir;
    app.enable_file_watching();
    app.test_start();
    assert_eq!(
        app.workspace.lsp_idle_stop(),
        Some(Duration::from_secs(3600)),
        "default"
    );
    fs::write(&user_file, "[lsp]\nidle_stop_minutes = 7\n").unwrap();
    assert!(
        app.settle(|a| a.workspace.lsp_idle_stop() == Some(Duration::from_secs(420)))
            .await,
        "{:?}",
        app.workspace.lsp_idle_stop()
    );
    fs::write(&user_file, "[lsp]\nidle_stop_minutes = 0\n").unwrap();
    assert!(app.settle(|a| a.workspace.lsp_idle_stop().is_none()).await, "0 = never");
}

/// A request still waiting for a server that is stopped for idleness is dropped, not left
/// pending for ever.
#[tokio::test(flavor = "multi_thread")]
async fn lsp_idle_stop_drops_pending_requests() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", "int a;\n")], false).await;
    fake.hold("textDocument/hover", true);
    put_cursor(&mut app, 1);
    show_hover(&mut app);
    assert!(
        app.settle(|a| a.pending_requests().iter().any(|(k, _)| k.contains("hover")))
            .await,
        "{:?}",
        app.pending_requests()
    );
    app.test_set_last_input(Instant::now());
    app.test_lsp_idle_tick(later(61));
    assert!(app.lsp.servers().is_empty());
    assert!(app.pending_requests().is_empty(), "{:?}", app.pending_requests());
}
