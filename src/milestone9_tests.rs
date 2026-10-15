//! Milestone 9 end to end (so far: 9a, the `lsp` core).
//!
//! 9a: opening a C++ file starts its server (the in-process fake, `lsp::fake`) once: `initialize`
//! carries the root and offers UTF-8 then UTF-16 positions, UTF-8 is negotiated when the server
//! picks it and UTF-16 when it names none; a C file shares the C++ server and plain text starts
//! none. `.workspace` `[lsp.cpp]` runs only once the workspace is trusted: nothing while the
//! trust prompt is open, the built-in server in restricted mode, the workspace's when trusted. A
//! missing binary (real process launcher) and a crash each give one notification and no restart.
//! Quitting sends `shutdown` and `exit` and waits for the server to stop, also (bounded) when it
//! never answers; a real process that floods stderr before answering and then ignores
//! `shutdown` still initializes and is killed and reaped. Against the real servers (skipped with a note when absent): clangd on `PATH`
//! and `ty server` (`PATH` or the repo's `.venv/bin`) initialize with UTF-8 and leave no process
//! behind after quit.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::app::{App, Level, Prompt};
use crate::buffer::Buffer;
use crate::buffer::pos::PosEncoding;
use crate::lsp::fake::{FakeLauncher, FakeOptions};
use crate::lsp::{Launcher, ProcessLauncher, SHUTDOWN_TIMEOUT, ServerConfig, State};
use crate::syntax::Language;
use crate::workspace::trust::{Trust, TrustStore};
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// A workspace `<dir>/proj` with `files`, its trust store in `<dir>/state`.
fn project(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
    let root = workspace::normalize(dir).join("proj");
    fs::create_dir_all(&root).unwrap();
    for (name, text) in files {
        fs::write(root.join(name), text).unwrap();
    }
    root
}

/// The app on `root` with servers started by `launcher`, no file open yet.
fn open_app(root: &Path, launcher: Arc<dyn Launcher>) -> App {
    let trust = TrustStore::load(root.parent().unwrap().join("state/trust.toml"));
    let mut app = App::new(Workspace::load(root.to_path_buf()), trust, None, true);
    app.lsp.set_launcher(launcher);
    app.test_start();
    app
}

fn open(app: &mut App, path: &Path) {
    app.test_open(Buffer::open(path).unwrap());
}

fn encoding(app: &App, language: Language) -> Option<PosEncoding> {
    app.lsp.server_for(language).and_then(|s| s.encoding())
}

/// Runs a loop turn (`sync_servers` included), lets background work run for a while, then
/// takes in its events (for "nothing happens" checks).
async fn idle(app: &mut App) {
    app.pump();
    tokio::time::sleep(Duration::from_millis(200)).await;
    app.pump();
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_cpp_file_starts_its_server_with_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        &[("a.cpp", "int main() {}\n"), ("b.c", "int x;\n"), ("n.txt", "hi\n")],
    );
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("n.txt"));
    idle(&mut app).await;
    assert!(fake.launches().is_empty(), "plain text has no server");
    assert!(app.lsp.servers().is_empty());

    open(&mut app, &root.join("a.cpp"));
    assert!(app.settle(|a| encoding(a, Language::Cpp).is_some()).await);
    assert_eq!(encoding(&app, Language::Cpp), Some(PosEncoding::Utf8));
    assert_eq!(fake.launches(), ["clangd"]);
    let init = fake.first("initialize").unwrap();
    let root_uri = crate::lsp::uri::from_path(&root);
    assert_eq!(init["params"]["rootUri"], root_uri.as_str());
    assert_eq!(init["params"]["workspaceFolders"][0]["uri"], root_uri.as_str());
    assert_eq!(
        init["params"]["capabilities"]["general"]["positionEncodings"],
        serde_json::json!(["utf-8", "utf-16"])
    );
    assert_eq!(init["params"]["processId"], std::process::id());
    // The app hears of `initialize`'s answer as `initialized` goes out: let the fake read it.
    assert!(app.settle(|_| fake.methods().len() >= 2).await);
    assert_eq!(fake.methods(), ["initialize", "initialized"]);

    open(&mut app, &root.join("b.c"));
    idle(&mut app).await;
    assert_eq!(fake.launches(), ["clangd"], "C shares the C++ server");
    assert_eq!(encoding(&app, Language::C), Some(PosEncoding::Utf8));
    assert!(app.message.is_none(), "starting is quiet");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_utf16_when_the_server_names_no_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.py", "x = 1\n")]);
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.py"));
    assert!(app.settle(|a| encoding(a, Language::Python).is_some()).await);
    assert_eq!(encoding(&app, Language::Python), Some(PosEncoding::Utf16));
    assert_eq!(fake.launches(), ["ty"]);
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_workspace_servers_only_run_when_trusted() {
    let files = [
        ("a.cpp", "int main() {}\n"),
        (".workspace", "[lsp.cpp]\ncommand = \"my-clangd\"\nargs = [\"--x\"]\n"),
    ];

    // Undecided: the prompt is open and nothing runs for C++; Esc = restricted → the built-in.
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &files);
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    assert!(matches!(app.prompt, Some(Prompt::Trust { .. })));
    open(&mut app, &root.join("a.cpp"));
    idle(&mut app).await;
    assert!(fake.launches().is_empty(), "{:?}", fake.launches());
    assert!(app.lsp.server_for(Language::Cpp).is_none());
    app.press(K::Esc, M::NONE);
    assert_eq!(app.trust, Trust::Restricted);
    assert!(app.settle(|a| encoding(a, Language::Cpp).is_some()).await);
    assert_eq!(fake.launches(), ["clangd"], "restricted: the built-in server");
    app.shutdown_servers().await;

    // Trusted: the workspace's server, with its arguments.
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &files);
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.cpp"));
    app.press(K::Char('t'), M::NONE);
    assert_eq!(app.trust, Trust::Trusted);
    assert!(app.settle(|a| encoding(a, Language::Cpp).is_some()).await);
    assert_eq!(fake.launches(), ["my-clangd"]);
    let s = app.lsp.server_for(Language::Cpp).unwrap();
    assert_eq!(
        (s.config.command.as_str(), s.config.args.as_slice()),
        ("my-clangd", &["--x".to_string()][..])
    );
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_missing_binary_notifies_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(
        dir.path(),
        &[("a.cpp", "int a;\n"), ("b.cpp", "int b;\n"), ("c.c", "int c;\n")],
    );
    let mut app = open_app(&root, Arc::new(ProcessLauncher));
    app.lsp.builtins.clangd = ServerConfig::new("red-test-no-such-language-server", &[]);
    open(&mut app, &root.join("a.cpp"));
    assert!(app.settle(|a| a.message.is_some()).await);
    let message = app.message.take().unwrap();
    assert_eq!(message.level, Level::Error);
    assert_eq!(
        message.text,
        "Language server red-test-no-such-language-server not found: install it, or set [lsp.cpp] command in .workspace"
    );
    open(&mut app, &root.join("b.cpp"));
    open(&mut app, &root.join("c.c"));
    idle(&mut app).await;
    assert!(app.message.is_none(), "reported once");
    assert_eq!(app.lsp.servers().len(), 1);
    assert!(matches!(app.lsp.servers()[0].state, State::Failed));
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_crash_notifies_once_without_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n"), ("b.cpp", "int b;\n")]);
    let fake = FakeLauncher::new(FakeOptions {
        crash_after_init: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.cpp"));
    assert!(app.settle(|a| a.message.is_some()).await);
    let message = app.message.take().unwrap();
    assert_eq!(message.level, Level::Error);
    assert!(
        message.text.starts_with("Language server clangd stopped"),
        "{}",
        message.text
    );
    open(&mut app, &root.join("b.cpp"));
    idle(&mut app).await;
    assert!(app.message.is_none());
    assert_eq!(fake.launches(), ["clangd"], "not restarted");
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_quit_shuts_servers_down() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n"), ("b.py", "b = 1\n")]);
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.cpp"));
    open(&mut app, &root.join("b.py"));
    assert!(
        app.settle(|a| encoding(a, Language::Cpp).is_some() && encoding(a, Language::Python).is_some())
            .await
    );
    app.press(K::Char('q'), cmd());
    assert!(app.quitting());
    app.shutdown_servers().await;
    assert_eq!(fake.exited(), 2, "both servers stopped");
    let methods = fake.methods();
    assert_eq!(methods.iter().filter(|m| *m == "shutdown").count(), 2, "{methods:?}");
    assert_eq!(methods.iter().filter(|m| *m == "exit").count(), 2, "{methods:?}");
    assert!(app.lsp.servers().is_empty());

    // A server that never answers `shutdown` is cut off after the timeout.
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n")]);
    let fake = FakeLauncher::new(FakeOptions {
        ignore_shutdown: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open(&mut app, &root.join("a.cpp"));
    assert!(app.settle(|a| encoding(a, Language::Cpp).is_some()).await);
    let t = Instant::now();
    app.shutdown_servers().await;
    assert!(t.elapsed() >= SHUTDOWN_TIMEOUT && t.elapsed() < SHUTDOWN_TIMEOUT + Duration::from_secs(1));
    assert!(!fake.methods().contains(&"exit".to_string()));
    assert_eq!(fake.exited(), 1, "its input closed");
}

/// A real process that floods its stderr (more than a pipe holds) before answering `initialize`,
/// then ignores everything: stderr is read while stdout is, and after `SHUTDOWN_TIMEOUT` the
/// process is killed and reaped.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_noisy_stuck_server_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n")]);
    let mut app = open_app(&root, Arc::new(ProcessLauncher));
    let body = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"positionEncoding":"utf-8"}}}"#;
    let script = format!(
        "yes 'noise on stderr' | head -n 20000 >&2; printf 'Content-Length: {}\\r\\n\\r\\n%s' '{body}'; exec sleep 1000",
        body.len()
    );
    app.lsp.builtins.clangd = ServerConfig::new("sh", &["-c", &script]);
    open(&mut app, &root.join("a.cpp"));
    assert!(
        app.settle(|a| encoding(a, Language::Cpp).is_some()).await,
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    assert_eq!(encoding(&app, Language::Cpp), Some(PosEncoding::Utf8));
    if let Some(n) = children_named("sleep") {
        assert_eq!(n, 1);
    }
    let t = Instant::now();
    app.shutdown_servers().await;
    assert!(t.elapsed() >= SHUTDOWN_TIMEOUT, "{:?}", t.elapsed());
    assert!(
        t.elapsed() < SHUTDOWN_TIMEOUT + Duration::from_secs(1),
        "{:?}",
        t.elapsed()
    );
    if let Some(n) = children_named("sleep") {
        assert_eq!(n, 0, "killed and reaped");
    }
}

/// Child processes of this test process named `name` (clangd renames itself `clangd.main`);
/// `None` without `pgrep`.
fn children_named(name: &str) -> Option<usize> {
    let out = std::process::Command::new("pgrep")
        .args(["-l", "-P", &std::process::id().to_string()])
        .output()
        .ok()?;
    let dotted = format!("{name}.");
    let count = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|comm| *comm == name || comm.starts_with(&dotted))
        .count();
    Some(count)
}

/// Opens `file` against a real server and checks the handshake and a clean exit.
async fn real_server(language: Language, file: (&str, &str), configure: impl FnOnce(&mut App), process: &str) {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[file]);
    let mut app = open_app(&root, Arc::new(ProcessLauncher));
    configure(&mut app);
    open(&mut app, &root.join(file.0));
    assert!(
        app.settle(|a| encoding(a, language).is_some()).await,
        "{process} initialized; message: {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    assert_eq!(encoding(&app, language), Some(PosEncoding::Utf8));
    assert!(app.message.is_none());
    if let Some(n) = children_named(process) {
        assert_eq!(n, 1, "{process} running");
    }
    app.press(K::Char('q'), cmd());
    let t = Instant::now();
    app.shutdown_servers().await;
    assert!(
        t.elapsed() < SHUTDOWN_TIMEOUT,
        "{process} exits when asked: {:?}",
        t.elapsed()
    );
    if let Some(n) = children_named(process) {
        assert_eq!(n, 0, "no {process} left");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_clangd() {
    if which("clangd").is_none() {
        eprintln!("milestone_9_real_clangd: skipped, clangd isn't on PATH");
        return;
    }
    real_server(Language::Cpp, ("a.cpp", "int main() { return 0; }\n"), |_| {}, "clangd").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_ty() {
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/ty");
    let Some(ty) = which("ty").or_else(|| venv.is_file().then_some(venv)) else {
        eprintln!("milestone_9_real_ty: skipped, ty isn't on PATH or in .venv/bin");
        return;
    };
    let configure = |app: &mut App| app.lsp.builtins.ty = ServerConfig::new(ty.to_str().unwrap(), &["server"]);
    real_server(Language::Python, ("a.py", "x: int = 1\n"), configure, "ty").await;
}
