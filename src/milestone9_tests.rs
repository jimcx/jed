//! Milestone 9 end to end (so far: 9a, the `lsp` core; 9b, document sync).
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
//!
//! 9b: an opened file is `didOpen`ed (`languageId`, version 1, its text) once its server runs;
//! the fake rebuilds its copy from `didChange` ranges alone (converted by its own position code)
//! and equals the buffer, versions going up by one per loop turn, after typing, Enter, paste,
//! Cmd+D edits, undo / redo and Cmd+Shift+L at 960 selections (one coalesced range per key), in
//! both encodings, and after 300 seeded random edits (astral chars, CRLF / CR / LF, multi-cursor,
//! line commands; several change sets per `didChange`); never the full text unless the server
//! only takes full sync (then the whole text); a no-sync server gets nothing, an open / close
//! only one just `didOpen`. Saving sends `didSave` (with the text when asked, after the changes
//! of a key that came before the save's result); closing a tab `didClose`; an explorer rename
//! closes and reopens it at the new URI (as C for `.c`), a `.workspace` association change
//! reopens it with the new `languageId`, and one to plain text closes it and stops tracking. Typing at ~63k cursors in a 1 MB C++ file with the fake server
//! stays within M6's keystroke budget (release). Real clangd and ty take edits at two cursors,
//! undo and a save and stay up, and still exit quickly on quit.

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
    assert!(app.settle(|_| fake.methods().len() >= 3).await);
    assert_eq!(fake.methods(), ["initialize", "initialized", "textDocument/didOpen"]);

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
    // Edits at two cursors, undo and a save (9b): the server stays up and says nothing (a bad
    // range would be logged by it, not answered; 9c checks its diagnostics follow the text).
    let path = root.join(file.0);
    app.press(K::Down, cmd() | M::ALT);
    app.press(K::End, M::NONE);
    app.type_text(" é😀");
    app.pump();
    app.press(K::Char('z'), cmd());
    app.pump();
    app.type_text("y");
    app.press(K::Char('s'), cmd());
    assert!(
        app.settle(|a| a.message.as_ref().is_some_and(|m| m.text.starts_with("Saved")))
            .await
    );
    idle(&mut app).await;
    let id = app.buffers.find_path(&path).unwrap();
    assert_eq!(app.lsp.version(id), Some(4), "three didChanges after didOpen");
    assert!(matches!(
        app.lsp.server_for(language).unwrap().state,
        State::Running { .. }
    ));
    assert!(app.message.as_ref().is_some_and(|m| m.text.starts_with("Saved")));
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
    real_server(
        Language::Cpp,
        ("a.cpp", "int main() {\n  return 0;\n}\n"),
        |_| {},
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_ty() {
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/ty");
    let Some(ty) = which("ty").or_else(|| venv.is_file().then_some(venv)) else {
        eprintln!("milestone_9_real_ty: skipped, ty isn't on PATH or in .venv/bin");
        return;
    };
    let configure = |app: &mut App| app.lsp.builtins.ty = ServerConfig::new(ty.to_str().unwrap(), &["server"]);
    real_server(Language::Python, ("a.py", "x: int = 1\ny: int = 2\n"), configure, "ty").await;
}

// ---- 9b: document sync ----

fn uri(path: &Path) -> String {
    crate::lsp::uri::from_path(path).as_str().to_string()
}

fn buffer_id(app: &App, path: &Path) -> crate::buffer::BufferId {
    app.buffers.find_path(path).expect("buffer open")
}

/// Waits until the fake's copy of `path` equals the buffer's text, at the version the app last
/// sent; and checks the fake found nothing wrong in the stream.
async fn in_sync(app: &mut App, fake: &FakeLauncher, path: &Path, what: &str) {
    let id = buffer_id(app, path);
    let u = uri(path);
    let synced = app
        .settle(|a| {
            let text = a.buffers.get(id).unwrap().rope.to_string();
            fake.document(&u)
                .is_some_and(|d| d.text == text && Some(d.version as i32) == a.lsp.version(id))
        })
        .await;
    assert!(
        synced,
        "{what}: fake {:?}\nbuffer {:?} (version {:?})\nerrors {:?}",
        fake.document(&u),
        app.buffers.get(id).unwrap().rope.to_string(),
        app.lsp.version(id),
        fake.errors()
    );
    assert!(fake.errors().is_empty(), "{what}: {:?}", fake.errors());
}

/// The `didChange` notifications received so far.
fn did_changes(fake: &FakeLauncher) -> Vec<serde_json::Value> {
    fake.received()
        .into_iter()
        .filter(|m| m["method"] == "textDocument/didChange")
        .collect()
}

/// Content changes without a range (whole-text replacements) received so far.
fn full_text_changes(fake: &FakeLauncher) -> usize {
    did_changes(fake)
        .iter()
        .flat_map(|m| m["params"]["contentChanges"].as_array().unwrap().clone())
        .filter(|c| c.get("range").is_none())
        .count()
}

/// Opens `path` against `fake` and waits for its `didOpen`.
async fn open_synced(app: &mut App, fake: &FakeLauncher, path: &Path) {
    open(app, path);
    in_sync(app, fake, path, "didOpen").await;
}

/// Typing, Enter, paste, Cmd+D multi-cursor edits, undo / redo, Cmd+Shift+L at 300+
/// selections (one coalesced range): after each, the fake's copy (rebuilt from `didChange`
/// ranges only) equals the buffer, versions going up by one. Both encodings.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_document_follows_edits() {
    for utf8 in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut text = String::from("int main() {\r\n  int é😀 = 1;\r\n}\n");
        for i in 0..320 {
            text.push_str(&format!("int item_{i} = item; // 😀 item\n"));
        }
        let root = project(dir.path(), &[("a.cpp", &text)]);
        let path = root.join("a.cpp");
        let fake = FakeLauncher::new(FakeOptions {
            utf8,
            ..Default::default()
        });
        let mut app = open_app(&root, fake.clone());
        open_synced(&mut app, &fake, &path).await;
        let doc = fake.document(&uri(&path)).unwrap();
        assert_eq!((doc.language.as_str(), doc.version), ("cpp", 1));
        assert_eq!(doc.text, text);

        // Typing (a burst before one loop turn: one didChange with several change sets).
        app.press(K::End, M::NONE);
        app.type_text(" // 😀x");
        in_sync(&mut app, &fake, &path, "typing").await;
        assert_eq!(
            app.lsp.version(buffer_id(&app, &path)),
            Some(2),
            "one didChange per turn"
        );
        app.press(K::Enter, M::NONE);
        app.type_text("é");
        in_sync(&mut app, &fake, &path, "enter").await;
        app.handle_test_event(crossterm::event::Event::Paste("a\r\nb😀\nc".into()));
        in_sync(&mut app, &fake, &path, "paste").await;

        // Cmd+D on `int` thrice, then type over them, delete, undo, redo.
        app.press(K::Char('k'), cmd()); // nothing bound: harmless
        app.press(K::Up, M::NONE);
        app.press(K::Up, M::NONE);
        app.press(K::Home, M::NONE);
        for _ in 0..3 {
            app.press(K::Char('d'), cmd());
        }
        app.type_text("long");
        app.press(K::Backspace, M::NONE);
        in_sync(&mut app, &fake, &path, "Cmd+D typing").await;
        app.press(K::Char('z'), cmd());
        in_sync(&mut app, &fake, &path, "undo").await;
        app.press(K::Char('z'), cmd());
        app.press(K::Char('z'), cmd());
        in_sync(&mut app, &fake, &path, "undo twice").await;
        app.press(K::Char('z'), cmd() | M::SHIFT);
        in_sync(&mut app, &fake, &path, "redo").await;

        // Cmd+Shift+L on `item`: 960 selections, over `COALESCE_AT`.
        app.press(K::Esc, M::NONE);
        app.press(K::Char('f'), cmd());
        app.type_text("item");
        app.press(K::Esc, M::NONE);
        app.press(K::Char('l'), cmd() | M::SHIFT);
        let count = app.group().active_tab().unwrap().view.selections.len();
        assert!(count > crate::lsp::documents::COALESCE_AT, "{count} selections");
        let before = did_changes(&fake).len();
        app.type_text("thing");
        app.pump();
        in_sync(&mut app, &fake, &path, "Cmd+Shift+L typing").await;
        let sent = did_changes(&fake);
        assert_eq!(
            sent[before..]
                .iter()
                .map(|m| m["params"]["contentChanges"].as_array().unwrap().len())
                .sum::<usize>(),
            5,
            "one coalesced range per keystroke"
        );
        app.press(K::Char('z'), cmd());
        in_sync(&mut app, &fake, &path, "Cmd+Shift+L undo").await;
        assert_eq!(full_text_changes(&fake), 0, "ranges only, never the full text");
        app.shutdown_servers().await;
    }
}

/// Saving sends `didSave` (with the text when the server asks for it); closing the last tab on
/// a buffer sends `didClose`; a rename or a language change closes and reopens it; a server
/// without open / close sync gets nothing; a full-sync server gets the whole text per change.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_open_save_close_and_sync_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n"), ("b.cpp", "int b;\n")]);
    let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
    let fake = FakeLauncher::new(FakeOptions {
        sync: Some(serde_json::json!({ "openClose": true, "change": 2, "save": { "includeText": true } })),
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &a).await;
    open_synced(&mut app, &fake, &b).await;
    app.type_text("x");
    app.press(K::Char('s'), cmd());
    // The save finishes and a key comes before its result is taken in: `didSave` still follows
    // that key's `didChange` (the fake checks the saved text against its copy).
    let t = Instant::now();
    while fs::read_to_string(&b).unwrap() != "xint b;\n" {
        assert!(t.elapsed() < Duration::from_secs(5), "save didn't finish");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    app.type_text("y");
    app.pump();
    assert!(app.settle(|_| !fake.saves().is_empty()).await);
    assert_eq!(fake.saves(), [(uri(&b), Some("xyint b;\n".to_string()))]);
    assert_eq!(
        fake.document(&uri(&b)).unwrap().text,
        "xyint b;\n",
        "changes before the save"
    );
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    // Saved again (the "y" came after the first save), so closing doesn't ask.
    app.press(K::Char('s'), cmd());
    assert!(app.settle(|_| fake.saves().len() == 2).await);

    // Close b's tab: didClose; a stays open.
    app.press(K::Char('w'), cmd());
    assert!(app.settle(|_| fake.document(&uri(&b)).is_none()).await);
    assert!(fake.document(&uri(&a)).is_some());

    // An explorer rename to a C file: closed, then opened at its new URI as C.
    app.press(K::Char('e'), cmd() | M::SHIFT);
    assert!(app.settle(|app| app.explorer.find(&a).is_some()).await);
    app.press(K::Home, M::NONE);
    for _ in 0..3 {
        if app.explorer.selected.as_deref() != Some(&a) {
            app.press(K::Down, M::NONE);
        }
    }
    assert_eq!(app.explorer.selected.as_deref(), Some(a.as_path()));
    app.press(K::F(2), M::NONE);
    for _ in 0.."pp".len() {
        app.press(K::Backspace, M::NONE);
    }
    app.press(K::Enter, M::NONE);
    let c = root.join("a.c");
    assert!(
        app.settle(|_| fake.document(&uri(&c)).is_some_and(|d| d.language == "c"))
            .await,
        "{:?}",
        fake.methods()
    );
    assert!(fake.document(&uri(&a)).is_none());
    assert_eq!(fake.document(&uri(&c)).unwrap().version, 1);
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;

    // Full sync: the whole text each time (the server can't take ranges); save without text.
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n")]);
    let a = root.join("a.cpp");
    let fake = FakeLauncher::new(FakeOptions {
        sync: Some(serde_json::json!(1)),
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &a).await;
    app.type_text("ab");
    in_sync(&mut app, &fake, &a, "full sync").await;
    assert_eq!(full_text_changes(&fake), 1);
    app.press(K::Char('s'), cmd());
    assert!(app.settle(|_| !fake.saves().is_empty()).await);
    assert_eq!(fake.saves(), [(uri(&a), None)]);
    app.shutdown_servers().await;

    // No document sync: nothing at all. Open / close only: `didOpen`, but no changes and (no
    // `save` option) no `didSave`.
    for (sync, want) in [
        (serde_json::json!(0), &[][..]),
        (
            serde_json::json!({ "openClose": true, "change": 0 }),
            &["textDocument/didOpen"][..],
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("a.cpp", "int a;\n")]);
        let fake = FakeLauncher::new(FakeOptions {
            sync: Some(sync),
            ..Default::default()
        });
        let mut app = open_app(&root, fake.clone());
        open(&mut app, &root.join("a.cpp"));
        assert!(app.settle(|a| encoding(a, Language::Cpp).is_some()).await);
        idle(&mut app).await;
        app.type_text("x");
        app.press(K::Char('s'), cmd());
        assert!(
            app.settle(|a| a.message.as_ref().is_some_and(|m| m.text.starts_with("Saved")))
                .await
        );
        idle(&mut app).await;
        let sent: Vec<String> = fake
            .methods()
            .into_iter()
            .filter(|m| m.starts_with("textDocument/"))
            .collect();
        assert_eq!(sent, want);
        app.shutdown_servers().await;
    }
}

/// A rename that keeps the language closes the document and opens it at the new URI; a
/// `.workspace` association that changes the language (same server: clangd takes C and C++)
/// closes and reopens it with the new `languageId`; one to a language without a server closes
/// it and stops tracking the buffer's changes.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_rename_and_language_change_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n")]);
    let a = root.join("a.cpp");
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &a).await;
    app.type_text("x");
    in_sync(&mut app, &fake, &a, "typing").await;

    // Explorer rename a.cpp → a.cc (still C++).
    app.press(K::Char('e'), cmd() | M::SHIFT);
    assert!(app.settle(|app| app.explorer.find(&a).is_some()).await);
    app.press(K::Home, M::NONE);
    for _ in 0..3 {
        if app.explorer.selected.as_deref() != Some(&a) {
            app.press(K::Down, M::NONE);
        }
    }
    assert_eq!(app.explorer.selected.as_deref(), Some(a.as_path()));
    app.press(K::F(2), M::NONE);
    app.press(K::Backspace, M::NONE);
    app.press(K::Backspace, M::NONE);
    app.type_text("c");
    app.press(K::Enter, M::NONE);
    let cc = root.join("a.cc");
    assert!(
        app.settle(|_| fake.document(&uri(&cc)).is_some()).await,
        "{:?}",
        fake.methods()
    );
    assert!(fake.document(&uri(&a)).is_none(), "the old URI closed");
    let doc = fake.document(&uri(&cc)).unwrap();
    assert_eq!(
        (doc.language.as_str(), doc.version, doc.text.as_str()),
        ("cpp", 1, "xint a;\n")
    );

    // `.workspace` makes it C: same path, same server, reopened as `c`.
    fs::write(
        root.join(".workspace"),
        "[files]\nassociations = { \"*.cc\" = \"c\" }\n",
    )
    .unwrap();
    assert!(
        app.settle(|_| fake.document(&uri(&cc)).is_some_and(|d| d.language == "c"))
            .await,
        "{:?}",
        fake.methods()
    );
    let id = buffer_id(&app, &cc);
    app.ui.focus = crate::app::Focus::Editor;
    app.type_text("y");
    in_sync(&mut app, &fake, &cc, "edit as C").await;
    assert_eq!(app.lsp.version(id), Some(2));

    // Plain text: closed, and the buffer no longer records its change sets.
    fs::write(
        root.join(".workspace"),
        "[files]\nassociations = { \"*.cc\" = \"plaintext\" }\n",
    )
    .unwrap();
    assert!(app.settle(|_| fake.document(&uri(&cc)).is_none()).await);
    assert_eq!(app.lsp.version(id), None);
    app.type_text("z");
    idle(&mut app).await;
    assert!(
        app.buffers.get_mut(id).unwrap().take_tracked().is_empty(),
        "not tracked"
    );
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;
}

/// A tiny xorshift generator: the same edits on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Seeded random edits through the keymap (typing astral and accented chars, Enter, deletes,
/// moves and selections, Cmd+D, add cursor below, paste with CRLF, move / copy / comment lines,
/// undo, redo) on a file with CRLF, lone CR and LF line breaks, in both encodings; changes are
/// sent at random turns (several change sets per `didChange`) and the fake's copy is compared
/// with the buffer every few steps.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_random_edits_stay_in_sync() {
    let text = "int main() {\r\n  int é😀 = 1;\r\n  // ꙮ x\r}\nint a😀 = 2;\n\tz\r\n";
    for utf8 in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = project(dir.path(), &[("r.cpp", text)]);
        let path = root.join("r.cpp");
        let fake = FakeLauncher::new(FakeOptions {
            utf8,
            ..Default::default()
        });
        let mut app = open_app(&root, fake.clone());
        open_synced(&mut app, &fake, &path).await;
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ utf8 as u64);
        let typed = ['a', 'é', '😀', ' ', '(', '}', '\t', 'ꙮ', '"'];
        for step in 0..300 {
            match rng.below(20) {
                0..=4 => app.press(K::Char(typed[rng.below(typed.len())]), M::NONE),
                5 => app.press(K::Enter, M::NONE),
                6 => app.press(K::Backspace, M::NONE),
                7 => app.press(K::Delete, M::NONE),
                8 => app.press([K::Left, K::Right, K::Up, K::Down][rng.below(4)], M::NONE),
                9 => app.press([K::Right, K::Down, K::End][rng.below(3)], M::SHIFT),
                10 => app.press(K::Char('d'), cmd()),
                11 => app.press(K::Down, cmd() | M::ALT),
                12 => app.handle_test_event(crossterm::event::Event::Paste("p\r\nq😀\n".into())),
                13 => app.press(K::Char('z'), cmd()),
                14 => app.press(K::Char('z'), cmd() | M::SHIFT),
                15 => app.press(K::Down, M::ALT),
                16 => app.press(K::Up, M::ALT | M::SHIFT),
                17 => app.press(K::Char('/'), cmd()),
                18 => app.press(K::Esc, M::NONE),
                _ => app.press([K::Home, K::End, K::PageUp][rng.below(3)], M::NONE),
            }
            if rng.below(3) == 0 {
                app.pump(); // a loop turn: sends what changed so far
            }
            if step % 25 == 24 {
                in_sync(&mut app, &fake, &path, &format!("utf8 {utf8}, step {step}")).await;
            }
        }
        in_sync(&mut app, &fake, &path, "the end").await;
        assert_eq!(full_text_changes(&fake), 0);
        let version = app.lsp.version(buffer_id(&app, &path)).unwrap();
        assert!(version > 20, "{version} didChanges");
        app.shutdown_servers().await;
    }
}

/// A key typed at every one of ~60k selections in a 1 MB C++ file open on the fake server, plus
/// the loop turn that sends its `didChange` and a frame: within the M6 keystroke budget (release
/// only; the fake's copy is checked in any build).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_typing_at_many_cursors_stays_fast() {
    /// M6's `KEYSTROKE_BUDGET` (one 60 Hz frame).
    const KEYSTROKE_BUDGET: Duration = Duration::from_millis(16);
    let size = if cfg!(debug_assertions) { 250_000 } else { 1_000_000 };
    let mut text = String::new();
    let mut i = 0;
    while text.len() < size {
        text.push_str(&format!("int item = value_{i}; // item\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("big.cpp", &text)]);
    let path = root.join("big.cpp");
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    app.settings.syntax_max_bytes = 1000; // time the sync, not tree-sitter
    open_synced(&mut app, &fake, &path).await;
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.press(K::Right, M::NONE);
    app.press(K::Right, M::NONE);
    app.press(K::Right, M::NONE);
    app.press(K::Right, M::NONE);
    app.press(K::Right, M::NONE);
    app.press(K::Char('l'), cmd() | M::SHIFT);
    let count = app.group().active_tab().unwrap().view.selections.len();
    assert_eq!(count, 2 * i);

    let n = 5u32;
    let mut worst = Duration::ZERO;
    let t = Instant::now();
    for c in "thing".chars() {
        let key = Instant::now();
        app.press(K::Char(c), M::NONE);
        app.pump(); // the loop turn: `sync_servers` sends the didChange
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        worst = worst.max(key.elapsed());
    }
    let typing = t.elapsed();
    println!(
        "typing at {count} cursors with a server: {n} keys in {typing:?}, avg {:?}, worst {worst:?}",
        typing / n
    );
    in_sync(&mut app, &fake, &path, "after typing").await;
    assert!(
        app.active_buffer()
            .unwrap()
            .rope
            .to_string()
            .starts_with("int thing = value_0; // thing\n")
    );
    let t = Instant::now();
    app.press(K::Char('z'), cmd());
    app.pump();
    let undo = t.elapsed();
    println!("undo with a server: {undo:?}");
    in_sync(&mut app, &fake, &path, "after undo").await;
    assert_eq!(full_text_changes(&fake), 0);
    if !cfg!(debug_assertions) {
        assert!(typing / n < KEYSTROKE_BUDGET, "typing too slow: avg {:?}", typing / n);
    }
    app.shutdown_servers().await;
}
