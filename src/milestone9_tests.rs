//! Milestone 9 end to end (so far: 9a, the `lsp` core; 9b, document sync; 9c, diagnostics;
//! 9d, the bottom panel and its Problems list; 9e, hover and go to definition).
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
//!
//! 9c: diagnostics the fake publishes (`test/publish`) are drawn as underlines in their
//! severity's theme colour at exactly their cells (UTF-16 ranges past an astral char; the most
//! severe wins where they overlap); the status bar counts errors and warnings in the workspace
//! (also files not open, which get their marks when opened) and shows the cursor line's
//! diagnostic unless a notification is up. Marks follow typing before them; a publish for an
//! older version is read in that version's text and mapped through the edits since (also one
//! not yet sent); without a version, or one no longer kept, in the current text; each publish
//! replaces the last, an empty one clears; a crashed server's diagnostics all go. ~31k
//! diagnostics on a 1 MB file are placed in one loop turn and followed through typing at ~63k
//! cursors within the keystroke budget (release). Real clangd and ty report an error on the
//! right word and clear it once it's fixed.
//!
//! 9d: Cmd+J shows the bottom panel under the editor groups (hidden again by Cmd+J, the editor
//! getting the keys back): a "PROBLEMS" tab with the total, then every file with problems
//! (open and not) sorted by path, each with its problems by position (icon in its colour, the
//! message's first line, `source(code)`, `[Ln, Col]`); the editor's view loses the panel's rows
//! and still keeps the cursor on screen. The selection stays on its problem when a publish adds
//! a file above it. Enter opens the selected problem with its range selected (also a file not
//! open yet, and after typing moved the mark: the row's column follows); a click opens it with
//! the list keeping the keys, a double click focuses the editor; End / PageUp scroll a 300-row
//! list. F8 / Shift+F8 step from the cursor through every problem (two at one place one by
//! one), opening the next file with problems and wrapping, the message in the status bar, the
//! list's selection following and the focus staying put. Cmd+Shift+M focuses the list (typing
//! does nothing; Down moves), Esc or an editor click gives the keys back, Cmd+Shift+M from the
//! list hides it; the status bar's counts show / hide it; its tab focuses it; dragging its top
//! border resizes it within the editor's and its own minimum heights. With no problems F8 and
//! the list's keys only say so; a file outside the root is listed with its folder and opens;
//! tiny terminals give the editor its rows first and never panic.
//!
//! 9e: Cmd+Alt+I (Ctrl+Alt+I) asks the server for the hover at the cursor; the answer (markdown:
//! code block, rule, a long paragraph) is drawn in a popup just above the hovered word's row
//! (below near the top), at the range's start, in the hover colours, the paragraph wrapped at
//! spaces to the editor's width; a long one fills the room above, shows "…" and scrolls with
//! the wheel over it, and clicks on it don't reach the text. A key, a paste (an edit) or the
//! wheel elsewhere closes it; Esc only closes it. Resting the mouse on text asks once the timer
//! fires (fired by `test_hover_tick`, nothing before); the popup stays while the pointer moves
//! within the word or onto it and closes past the line's end. A newer hover cancels the pending
//! one (`$/cancelRequest`), moving the pointer or a key cancels a pending one, an answer arriving
//! after the cursor moved is dropped, an empty one to the command says "No hover information".
//! F12 (and Ctrl+Click) asks for the definition (UTF-16 positions): one place in the same file
//! moves the cursor there (Cmd+U goes back), one in a file not open opens it there (column read
//! past an astral char), a `LocationLink` uses its selection range, none says "No definition
//! found for 'target'"; several fill the panel's LOCATIONS tab (grouped by file, each line
//! previewed with the place highlighted, files not open read in the background), focused on the
//! first place; Down / Enter open one, a click opens one keeping the list focused. A server that
//! crashes with requests pending drops them; an edit that leaves the cursor (Delete) drops a
//! pending definition; the popup takes clicks over the find widget and stays in its group's
//! editor at any size. Real clangd and ty show a symbol's signature on
//! hover and F12 opens the definition in another file.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use serde_json::{Value, json};

use crate::app::{App, Level, Prompt};
use crate::buffer::Buffer;
use crate::buffer::pos::PosEncoding;
use crate::diagnostics::Severity;
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

    // A server that never answers `shutdown` is cut off by its task after `SHUTDOWN_TIMEOUT`.
    // Which mechanism stopped it is asked directly (the task ended by itself, not aborted by
    // the manager's fallback), with a fallback far enough out that a loaded machine can't hit
    // it first; timing it against the 1 s margin quit uses was flaky under load.
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
    let by_itself = app.lsp.shutdown_within(Duration::from_secs(30)).await;
    let took = t.elapsed();
    assert!(by_itself, "the task's own timeout stopped it, not the abort ({took:?})");
    assert!(took >= SHUTDOWN_TIMEOUT, "it waited for the answer first: {took:?}");
    assert!(!fake.methods().contains(&"exit".to_string()));
    // The task stopped without waiting for the fake to see its input close (nothing to wait
    // for once it's out of time): the fake counts its exit a moment later.
    assert!(app.settle(|_| fake.exited() == 1).await, "its input closed");
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
    // As above: the task's own timeout kills it (a 30 s fallback, so load can't decide it).
    let by_itself = app.lsp.shutdown_within(Duration::from_secs(30)).await;
    assert!(by_itself, "killed by its task, not aborted: {:?}", t.elapsed());
    assert!(t.elapsed() >= SHUTDOWN_TIMEOUT, "{:?}", t.elapsed());
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

/// Held by each test that runs a real server: they count the test process's children by name,
/// so two at once would see each other's.
static REAL_SERVERS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Opens `file` against a real server and checks the handshake and a clean exit.
async fn real_server(language: Language, file: (&str, &str), configure: impl FnOnce(&mut App), process: &str) {
    let _one_at_a_time = REAL_SERVERS.lock().await;
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

// ---- 9c: diagnostics ----

/// Makes the fake server for `language` publish `diagnostics` for `path` (at `version`, if any).
fn publish(app: &App, language: Language, path: &Path, version: Option<i32>, diagnostics: Value) {
    let mut params = json!({ "uri": uri(path), "diagnostics": diagnostics });
    if let Some(v) = version {
        params["version"] = json!(v);
    }
    app.lsp.server_for(language).unwrap().notify("test/publish", params);
}

/// An LSP diagnostic on one line (`severity`: 1 error … 4 hint).
fn diag(line: u32, from: u32, to: u32, severity: u8, message: &str) -> Value {
    json!({
        "range": { "start": { "line": line, "character": from }, "end": { "line": line, "character": to } },
        "severity": severity,
        "message": message,
    })
}

/// The buffer's marks as (text under it, severity, message).
fn marks(app: &App, path: &Path) -> Vec<(String, Severity, String)> {
    let buf = app.buffers.get(buffer_id(app, path)).unwrap();
    let text = buf.rope.to_string();
    buf.diagnostics
        .all()
        .iter()
        .map(|m| {
            (
                text[m.start.0..m.end.0].to_string(),
                m.diag.severity,
                m.diag.message.clone(),
            )
        })
        .collect()
}

fn mark(text: &str, severity: Severity, message: &str) -> (String, Severity, String) {
    (text.into(), severity, message.into())
}

fn terminal() -> ratatui::Terminal<ratatui::backend::TestBackend> {
    ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 16)).unwrap()
}

fn draw(term: &mut ratatui::Terminal<ratatui::backend::TestBackend>, app: &mut App) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

/// The first cell where `needle` is drawn, one char per cell.
fn find_cells(term: &ratatui::Terminal<ratatui::backend::TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let buf = term.backend().buffer();
    let chars: Vec<String> = needle.chars().map(String::from).collect();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(chars.len() as u16) {
            if chars
                .iter()
                .enumerate()
                .all(|(i, c)| buf[(x + i as u16, y)].symbol() == c)
            {
                return Some((x, y));
            }
        }
    }
    None
}

/// The squiggle colour of a cell (`None`: not underlined).
fn squiggle(term: &ratatui::Terminal<ratatui::backend::TestBackend>, x: u16, y: u16) -> Option<ratatui::style::Color> {
    let cell = &term.backend().buffer()[(x, y)];
    cell.modifier
        .contains(ratatui::style::Modifier::UNDERLINED)
        .then_some(cell.underline_color)
}

/// The squiggle colours under `needle` (where it's first drawn), one per char.
fn squiggles_under(
    term: &ratatui::Terminal<ratatui::backend::TestBackend>,
    needle: &str,
) -> Vec<Option<ratatui::style::Color>> {
    let (x, y) = find_cells(term, needle).unwrap_or_else(|| panic!("{needle:?} drawn"));
    (0..needle.chars().count() as u16)
        .map(|i| squiggle(term, x + i, y))
        .collect()
}

fn status(term: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buf = term.backend().buffer();
    let y = buf.area.height - 1;
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

fn colour(app: &App, severity: Severity) -> Option<ratatui::style::Color> {
    let c = app.theme.get(severity.theme_key()).underline_color;
    assert!(c.is_some());
    c
}

/// Published diagnostics (UTF-16 ranges, past an astral char) are drawn as underlines in their
/// severity's colour at exactly their cells; the status bar counts errors and warnings across
/// the workspace and shows the cursor line's diagnostic (the most severe; a notification wins
/// until a key); a file that isn't open counts too and gets its marks when opened.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_diagnostics_show_as_squiggles_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let text = "int main() {\n  /*😀*/ foo();\n  return bar;\n}\n";
    let root = project(dir.path(), &[("a.cpp", text), ("b.cpp", "int b = /*😀*/ oops;\n")]);
    let (pa, pb) = (root.join("a.cpp"), root.join("b.cpp"));
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &pa).await;
    let mut term = terminal();
    draw(&mut term, &mut app);
    assert!(status(&term).contains("⊗ 0  ⚠ 0"), "{}", status(&term));

    // UTF-16: 😀 is two units, so `foo` is at 9..12 (bytes 11..14).
    publish(
        &app,
        Language::Cpp,
        &pa,
        Some(1),
        json!([
            diag(2, 9, 12, 2, "unused\n  bar"),
            diag(1, 9, 12, 1, "use of undeclared identifier 'foo'"),
            diag(1, 9, 10, 4, "a hint"),
        ]),
    );
    assert!(app.settle(|a| marks(a, &pa).len() == 3).await);
    assert_eq!(
        marks(&app, &pa),
        [
            mark("f", Severity::Hint, "a hint"),
            mark("foo", Severity::Error, "use of undeclared identifier 'foo'"),
            mark("bar", Severity::Warning, "unused\n  bar"),
        ]
    );
    draw(&mut term, &mut app);
    let (error, warning) = (colour(&app, Severity::Error), colour(&app, Severity::Warning));
    assert_eq!(
        squiggles_under(&term, " foo()"),
        [None, error, error, error, None, None],
        "the error wins over the hint"
    );
    assert_eq!(squiggles_under(&term, " bar;"), [None, warning, warning, warning, None]);
    assert!(status(&term).contains("⊗ 1  ⚠ 1"), "{}", status(&term));
    assert!(!status(&term).contains("undeclared"), "the cursor's line has none");

    app.press(K::Down, M::NONE);
    draw(&mut term, &mut app);
    assert!(
        status(&term).contains(" ⊗ use of undeclared identifier 'foo' "),
        "{}",
        status(&term)
    );
    app.press(K::Down, M::NONE);
    draw(&mut term, &mut app);
    assert!(status(&term).contains(" ⚠ unused bar "), "one line: {}", status(&term));
    // A notification wins until the next key.
    app.press(K::Char('s'), cmd());
    assert!(app.settle(|a| a.message.is_some()).await);
    draw(&mut term, &mut app);
    assert!(status(&term).contains("Saved") && !status(&term).contains("unused"));
    app.press(K::Right, M::NONE);
    draw(&mut term, &mut app);
    assert!(status(&term).contains(" ⚠ unused bar "), "{}", status(&term));

    // A file that isn't open: counted, stored, and placed when it opens (in the server's
    // encoding: UTF-16, `oops` at 15..19, bytes 17..21).
    publish(
        &app,
        Language::Cpp,
        &pb,
        None,
        json!([diag(0, 15, 19, 1, "use of undeclared identifier 'oops'")]),
    );
    assert!(
        app.settle(|a| a.lsp.diagnostics.counts().of(Severity::Error) == 2)
            .await
    );
    draw(&mut term, &mut app);
    assert!(status(&term).contains("⊗ 2  ⚠ 1"), "{}", status(&term));
    open(&mut app, &pb);
    assert_eq!(
        marks(&app, &pb),
        [mark("oops", Severity::Error, "use of undeclared identifier 'oops'")]
    );
    draw(&mut term, &mut app);
    assert_eq!(squiggles_under(&term, "oops;"), [error, error, error, error, None]);
    // Open now: a publish without a version is read in its text, in UTF-16 too.
    publish(&app, Language::Cpp, &pb, None, json!([diag(0, 15, 19, 2, "again")]));
    assert!(app.settle(|a| marks(a, &pb).iter().any(|m| m.2 == "again")).await);
    assert_eq!(marks(&app, &pb), [mark("oops", Severity::Warning, "again")]);
    app.shutdown_servers().await;
}

/// Where squiggles go at the edges: an empty range under the char there, under the one before
/// at a line end, on the cell past an empty line's end; a range over a line break underlines
/// nothing past the line's last char.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_diagnostics_at_line_ends_and_empty_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a;\n\nint b = c\nlong yy\nz;\n")]);
    let path = root.join("a.cpp");
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &path).await;
    let over_break = json!({
        "range": { "start": { "line": 3, "character": 5 }, "end": { "line": 4, "character": 1 } },
        "severity": 4,
        "message": "over a line break",
    });
    publish(
        &app,
        Language::Cpp,
        &path,
        Some(1),
        json!([
            diag(0, 4, 4, 1, "e"),
            diag(1, 0, 0, 2, "w"),
            diag(2, 9, 9, 3, "i"),
            over_break
        ]),
    );
    assert!(app.settle(|a| marks(a, &path).len() == 4).await);
    let mut term = terminal();
    draw(&mut term, &mut app);
    let [e, w, i, h] = Severity::ALL.map(|s| colour(&app, s));
    assert_eq!(squiggles_under(&term, "int a;"), [None, None, None, None, e, None]);
    let (x, y) = find_cells(&term, "int a;").unwrap();
    assert_eq!(
        (squiggle(&term, x, y + 1), squiggle(&term, x + 1, y + 1)),
        (w, None),
        "the empty line: one cell"
    );
    assert_eq!(squiggle(&term, x + 6, y), None, "nothing past `int a;`");
    let (x, y) = find_cells(&term, "int b = c").unwrap();
    assert_eq!(
        (squiggle(&term, x + 8, y), squiggle(&term, x + 9, y)),
        (i, None),
        "under `c`"
    );
    assert_eq!(squiggles_under(&term, "int b = ")[..8], [None; 8]);
    assert_eq!(squiggles_under(&term, "long yy"), [None, None, None, None, None, h, h]);
    let (x, y) = find_cells(&term, "long yy").unwrap();
    assert_eq!(squiggle(&term, x + 7, y), None, "not past the line's end");
    assert_eq!(squiggles_under(&term, "z;"), [h, None]);
    app.shutdown_servers().await;
}

/// Marks follow edits until the next publish: typing before a word keeps its squiggle on it; a
/// publish for an older version is read in that version's text and mapped through the edits
/// since (also ones made after it was sent); without a version (or one no longer kept) it's
/// read in the current text; each publish replaces the last; an empty one clears.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_diagnostics_follow_edits_until_republished() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int main() {\n  foo();\n}\n")]);
    let path = root.join("a.cpp");
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &path).await;
    let id = buffer_id(&app, &path);
    publish(&app, Language::Cpp, &path, Some(1), json!([diag(1, 2, 5, 1, "v1")]));
    assert!(app.settle(|a| !marks(a, &path).is_empty()).await);

    // Typing before it: the squiggle stays on its word.
    app.press(K::Down, M::NONE);
    app.press(K::Home, M::NONE);
    app.type_text("xy ");
    assert_eq!(marks(&app, &path), [mark("foo", Severity::Error, "v1")]);
    let mut term = terminal();
    draw(&mut term, &mut app);
    let error = colour(&app, Severity::Error);
    assert_eq!(
        squiggles_under(&term, " xy foo("),
        [None, None, None, None, error, error, error, None]
    );
    in_sync(&mut app, &fake, &path, "typed").await;
    assert_eq!(app.lsp.version(id), Some(2));

    // Published for version 2 ("  xy foo();": foo at 5..8) while the text moved on: version 3
    // sent, and keys not yet sent when the publish comes in, one a line break above it (so
    // reading line 1 in the current text instead of version 2's would land on `int main`).
    app.press(K::Home, M::NONE);
    app.type_text("w");
    app.pump();
    assert_eq!(app.lsp.version(id), Some(3));
    publish(&app, Language::Cpp, &path, Some(2), json!([diag(1, 5, 8, 2, "v2")]));
    app.type_text("v");
    app.press(K::Up, M::NONE);
    app.press(K::Home, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| marks(a, &path).iter().any(|m| m.2 == "v2")).await);
    assert_eq!(
        marks(&app, &path),
        [mark("foo", Severity::Warning, "v2")],
        "replaced, mapped"
    );
    assert_eq!(pos_line(&app, &path), "  wvxy foo();");

    // No version: the current text ("  wvxy foo();" on line 2: foo at 7..10).
    publish(&app, Language::Cpp, &path, None, json!([diag(2, 7, 10, 3, "now")]));
    assert!(app.settle(|a| marks(a, &path).iter().any(|m| m.2 == "now")).await);
    assert_eq!(marks(&app, &path), [mark("foo", Severity::Info, "now")]);
    // Version 1 is no longer kept (versions before 2 went with the version-2 publish): read
    // in the current text too.
    publish(&app, Language::Cpp, &path, Some(1), json!([diag(2, 2, 5, 3, "old")]));
    assert!(app.settle(|a| marks(a, &path).iter().any(|m| m.2 == "old")).await);
    assert_eq!(marks(&app, &path), [mark("wvx", Severity::Info, "old")]);

    // A publish handled in the same loop turn as a key not yet sent (both come before the
    // turn's `sync_servers`): the key is sent first, so the publish (for the version before
    // it) is mapped through it: a line break above `foo`.
    let sent = app.lsp.version(id);
    app.press(K::Enter, M::NONE);
    let mut params = json!({ "uri": uri(&path), "diagnostics": [diag(2, 7, 10, 1, "same turn")] });
    params["version"] = json!(sent);
    app.test_lsp_event(crate::lsp::Event::Diagnostics {
        server: app.lsp.server_for(Language::Cpp).unwrap().id,
        published: Box::new(crate::lsp::diagnostics::parse(params).unwrap()),
    });
    assert_eq!(app.lsp.version(id), sent.map(|v| v + 1), "the key went out first");
    assert_eq!(marks(&app, &path), [mark("foo", Severity::Error, "same turn")]);

    // An empty publish clears.
    let version = app.lsp.version(id);
    publish(&app, Language::Cpp, &path, version, json!([]));
    assert!(app.settle(|a| marks(a, &path).is_empty()).await);
    assert_eq!(app.lsp.diagnostics.counts(), Default::default());
    draw(&mut term, &mut app);
    assert_eq!(squiggles_under(&term, "foo();"), [None; 6]);
    in_sync(&mut app, &fake, &path, "after the publishes").await;
    app.shutdown_servers().await;
}

/// Exactly `KEEP_VERSIONS` versions are kept: after 32 sent edits (each a line break above
/// `foo`), version 1 is gone (read in the current text: an empty line) and version 2, the oldest
/// kept, is read in its own text and mapped down to `foo`.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_diagnostics_keep_the_last_32_versions() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "\nfoo();\n")]);
    let path = root.join("a.cpp");
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &path).await;
    let id = buffer_id(&app, &path);
    assert_eq!(crate::lsp::documents::KEEP_VERSIONS, 32);
    for _ in 0..32 {
        app.press(K::Enter, M::NONE);
        app.pump();
    }
    assert_eq!(app.lsp.version(id), Some(33));
    in_sync(&mut app, &fake, &path, "32 line breaks").await;
    publish(&app, Language::Cpp, &path, Some(1), json!([diag(1, 0, 3, 1, "v1")]));
    assert!(app.settle(|a| marks(a, &path).iter().any(|m| m.2 == "v1")).await);
    assert_eq!(marks(&app, &path), [mark("", Severity::Error, "v1")], "not kept");
    publish(&app, Language::Cpp, &path, Some(2), json!([diag(2, 0, 3, 1, "v2")]));
    assert!(app.settle(|a| marks(a, &path).iter().any(|m| m.2 == "v2")).await);
    assert_eq!(marks(&app, &path), [mark("foo", Severity::Error, "v2")], "kept");
    app.shutdown_servers().await;
}

/// Line 2 of the buffer at `path`.
fn pos_line(app: &App, path: &Path) -> String {
    let buf = app.buffers.get(buffer_id(app, path)).unwrap();
    crate::buffer::pos::line_text(&buf.rope, 2)
}

/// A server that stops takes its diagnostics along: open and unopened files', squiggles and counts.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_diagnostics_go_when_the_server_dies() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a = x;\n"), ("b.cpp", "int b = y;\n")]);
    let (pa, pb) = (root.join("a.cpp"), root.join("b.cpp"));
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &pa).await;
    publish(&app, Language::Cpp, &pa, Some(1), json!([diag(0, 8, 9, 1, "x?")]));
    publish(&app, Language::Cpp, &pb, None, json!([diag(0, 8, 9, 2, "y?")]));
    assert!(
        app.settle(|a| a.lsp.diagnostics.counts().of(Severity::Warning) == 1)
            .await
    );
    let mut term = terminal();
    draw(&mut term, &mut app);
    assert!(status(&term).contains("⊗ 1  ⚠ 1"));
    assert_eq!(squiggles_under(&term, "x;")[0], colour(&app, Severity::Error));

    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/crash", Value::Null);
    assert!(app.settle(|a| a.message.is_some()).await);
    assert!(app.message.as_ref().unwrap().text.contains("clangd stopped"));
    assert!(marks(&app, &pa).is_empty());
    assert_eq!(app.lsp.diagnostics.files().count(), 0);
    draw(&mut term, &mut app);
    assert!(status(&term).contains("⊗ 0  ⚠ 0"), "{}", status(&term));
    assert_eq!(squiggles_under(&term, "x;"), [None, None]);
    app.shutdown_servers().await;
}

/// A server stopped because the config no longer names it (a `.workspace` `[lsp.cpp]`) takes
/// its diagnostics along in the same loop turn.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_diagnostics_go_when_the_server_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int a = x;\n")]);
    let path = root.join("a.cpp");
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &path).await;
    publish(&app, Language::Cpp, &path, Some(1), json!([diag(0, 8, 9, 1, "x?")]));
    assert!(app.settle(|a| !marks(a, &path).is_empty()).await);
    let old = app.lsp.server_for(Language::Cpp).unwrap().id;
    fs::write(root.join(".workspace"), "[lsp.cpp]\ncommand = \"my-clangd\"\n").unwrap();
    // `settle` checks right after the turn's `sync_servers`, before any event from the server.
    assert!(
        app.settle(|a| a.lsp.server_for(Language::Cpp).is_none_or(|s| s.id != old))
            .await
    );
    assert!(marks(&app, &path).is_empty());
    assert_eq!(app.lsp.diagnostics.counts(), Default::default());
    app.shutdown_servers().await;
}

/// Waits up to `secs` for `done` (real servers can take a while).
async fn settle_for(app: &mut App, secs: u64, done: impl Fn(&App) -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if app.settle(&done).await {
            return true;
        }
    }
    false
}

/// A real server reports an error in `file` on `word`; `fix` edits it away and the error goes.
async fn real_diagnostics(
    file: (&str, &str),
    word: &str,
    configure: impl FnOnce(&mut App),
    fix: impl FnOnce(&mut App),
    process: &str,
) {
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[file]);
    let path = root.join(file.0);
    let mut app = open_app(&root, Arc::new(ProcessLauncher));
    configure(&mut app);
    open(&mut app, &path);
    let has_error = |a: &App| marks(a, &path).iter().any(|m| m.1 == Severity::Error);
    assert!(
        settle_for(&mut app, 60, has_error).await,
        "{process} reports an error; message: {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    let errors: Vec<_> = marks(&app, &path)
        .into_iter()
        .filter(|m| m.1 == Severity::Error)
        .collect();
    assert!(errors.iter().any(|m| m.0 == word), "{process}: {errors:?}");
    assert!(app.lsp.diagnostics.counts().of(Severity::Error) >= 1);
    fix(&mut app);
    assert!(
        settle_for(&mut app, 60, |a| !has_error(a)).await,
        "{process} clears the error after the fix: {:?}, text {:?}",
        marks(&app, &path),
        app.active_buffer().unwrap().rope.to_string()
    );
    assert_eq!(app.lsp.diagnostics.counts().of(Severity::Error), 0);
    app.press(K::Char('q'), cmd());
    app.shutdown_servers().await;
    if let Some(n) = children_named(process) {
        assert_eq!(n, 0, "no {process} left");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_clangd_diagnostics() {
    if which("clangd").is_none() {
        eprintln!("milestone_9_real_clangd_diagnostics: skipped, clangd isn't on PATH");
        return;
    }
    let fix = |app: &mut App| {
        // Select the identifier (the cursor at its end, Cmd+D) and replace it.
        app.press(K::Down, M::NONE);
        app.press(K::End, M::NONE);
        app.press(K::Left, M::NONE);
        app.press(K::Char('d'), cmd());
        app.type_text("0");
    };
    real_diagnostics(
        ("a.cpp", "int main() {\n  return undefined_thing;\n}\n"),
        "undefined_thing",
        |_| {},
        fix,
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_ty_diagnostics() {
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/ty");
    let Some(ty) = which("ty").or_else(|| venv.is_file().then_some(venv)) else {
        eprintln!("milestone_9_real_ty_diagnostics: skipped, ty isn't on PATH or in .venv/bin");
        return;
    };
    let configure = |app: &mut App| app.lsp.builtins.ty = ServerConfig::new(ty.to_str().unwrap(), &["server"]);
    let fix = |app: &mut App| {
        app.press(K::End, M::NONE);
        app.press(K::Left, M::SHIFT);
        app.press(K::Left, M::SHIFT);
        app.press(K::Left, M::SHIFT);
        app.type_text("1");
    };
    real_diagnostics(("a.py", "x: int = \"a\"\n"), "\"a\"", configure, fix, "ty").await;
}

/// Thousands of diagnostics (one per line of a 1 MB file): placed once, then mapped through a
/// key at ~60k cursors and drawn within M6's keystroke budget (release only).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_many_diagnostics_stay_cheap() {
    const KEYSTROKE_BUDGET: Duration = Duration::from_millis(16);
    let size = if cfg!(debug_assertions) { 250_000 } else { 1_000_000 };
    let mut text = String::new();
    let mut lines = 0;
    while text.len() < size {
        text.push_str(&format!("int item = value_{lines}; // item\n"));
        lines += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("big.cpp", &text)]);
    let path = root.join("big.cpp");
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    app.settings.syntax_max_bytes = 1000; // time diagnostics, not tree-sitter
    open_synced(&mut app, &fake, &path).await;
    let diags: Vec<Value> = (0..lines as u32)
        .map(|l| diag(l, 11, 18, 1 + (l % 4) as u8, "value"))
        .collect();
    publish(&app, Language::Cpp, &path, Some(1), Value::Array(diags));
    // Read in the server's task; placed on the UI thread in one loop turn (timed: the longest).
    let t = Instant::now();
    let mut longest = Duration::ZERO;
    while marks(&app, &path).len() != lines && t.elapsed() < Duration::from_secs(10) {
        let turn = Instant::now();
        app.pump();
        longest = longest.max(turn.elapsed());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(marks(&app, &path).len(), lines);
    println!(
        "{lines} diagnostics published and placed in {:?}; the longest loop turn {longest:?}",
        t.elapsed()
    );
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    draw(&mut term, &mut app);
    for _ in 0..5 {
        app.press(K::Right, M::NONE);
    }
    app.press(K::Char('l'), cmd() | M::SHIFT);
    let count = app.group().active_tab().unwrap().view.selections.len();
    assert_eq!(count, 2 * lines);
    let n = 5u32;
    let mut worst = Duration::ZERO;
    let t = Instant::now();
    for c in "thing".chars() {
        let key = Instant::now();
        app.press(K::Char(c), M::NONE);
        app.pump();
        draw(&mut term, &mut app);
        worst = worst.max(key.elapsed());
    }
    let typing = t.elapsed();
    println!(
        "typing at {count} cursors with {lines} diagnostics: avg {:?}, worst {worst:?}",
        typing / n
    );
    let on_value = |m: &(String, Severity, String)| m.0.starts_with("value_") && m.0.len() == 7;
    assert!(
        marks(&app, &path).iter().all(on_value),
        "the squiggles stay on `value_…`"
    );
    let error = colour(&app, Severity::Error);
    assert_eq!(squiggles_under(&term, " value_0;")[..3], [None, error, error]);
    if !cfg!(debug_assertions) {
        assert!(typing / n < KEYSTROKE_BUDGET, "typing too slow: avg {:?}", typing / n);
        assert!(longest < 2 * KEYSTROKE_BUDGET, "placing them took {longest:?}");
    }

    // The Problems list over all of them: a key and a frame only look at the rows on screen.
    app.press(K::Char('j'), cmd());
    draw(&mut term, &mut app);
    let t = Instant::now();
    app.press(K::End, M::NONE);
    draw(&mut term, &mut app);
    app.press(K::PageUp, M::NONE);
    draw(&mut term, &mut app);
    let panel = t.elapsed() / 2;
    println!("Problems list of {lines} diagnostics: a key + frame {panel:?}");
    assert!(
        find_cells(&term, &format!("[Ln {lines}, Col 12]")).is_none(),
        "paged up"
    );
    assert!(find_cells(&term, "[Ln ").is_some());
    if !cfg!(debug_assertions) {
        assert!(panel < KEYSTROKE_BUDGET, "the Problems list: {panel:?}");
    }
    app.shutdown_servers().await;
}

// ---- 9d: the bottom panel and its Problems list ----

/// Rows of the fixture in `problems_project`, as the Problems list shows them (files by path):
///
/// ```text
/// a.cpp            2   ⊗ use of undeclared identifier 'foo'  clang(undeclared_var_use) [Ln 1, Col 9]
///                      ⚠ unused bar [Ln 2, Col 21]
/// c.cpp            1   ⓘ c info [Ln 1, Col 9]
/// b.cpp  sub       1   ⊗ b error [Ln 3, Col 9]
/// ```
const A_CPP: &str = "int x = foo;\nint main() { return bar; }\n";

/// A project with `a.cpp` open (synced with the fake) and diagnostics published for it and two
/// files that aren't open (`c.cpp`, `sub/b.cpp`).
async fn problems_project(dir: &Path) -> (App, Arc<FakeLauncher>, PathBuf) {
    let root = project(
        dir,
        &[
            ("a.cpp", A_CPP),
            ("c.cpp", "int c = oops;\n"),
            ("long.txt", &"line\n".repeat(60)),
        ],
    );
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub/b.cpp"), "\n\nint b = nope;\n").unwrap();
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    let mut foo = diag(0, 8, 11, 1, "use of undeclared identifier 'foo'");
    foo["source"] = json!("clang");
    foo["code"] = json!("undeclared_var_use");
    publish(
        &app,
        Language::Cpp,
        &root.join("a.cpp"),
        None,
        json!([diag(1, 20, 23, 2, "unused bar"), foo]),
    );
    publish(
        &app,
        Language::Cpp,
        &root.join("c.cpp"),
        None,
        json!([diag(0, 8, 12, 3, "c info")]),
    );
    publish(
        &app,
        Language::Cpp,
        &root.join("sub/b.cpp"),
        None,
        json!([diag(2, 8, 12, 1, "b error")]),
    );
    assert!(app.settle(|a| a.lsp.diagnostics.files().count() == 3).await);
    (app, fake, root)
}

fn big_terminal() -> ratatui::Terminal<ratatui::backend::TestBackend> {
    ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 30)).unwrap()
}

/// Screen row `y` from column `x` as text.
fn screen_row(term: &ratatui::Terminal<ratatui::backend::TestBackend>, x: u16, y: u16) -> String {
    let buf = term.backend().buffer();
    (x..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// The bottom panel's list rows as drawn (below its border and tab strip), trimmed.
fn panel_rows(term: &ratatui::Terminal<ratatui::backend::TestBackend>, app: &App) -> Vec<String> {
    let Some(panel) = app.hits.regions.panel else {
        return Vec::new();
    };
    (panel.y + 2..panel.y + panel.height)
        .map(|y| screen_row(term, panel.x, y).trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

fn click(app: &mut App, (x, y): (u16, u16)) {
    use crossterm::event::{MouseButton, MouseEventKind};
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

fn active_path(app: &App) -> Option<PathBuf> {
    app.active_buffer().map(|b| b.path.clone())
}

/// The active view's primary selection as (anchor, head) byte offsets.
fn selection(app: &App) -> (usize, usize) {
    let s = app.group().active_tab().unwrap().view.selections.primary();
    (s.anchor.0, s.head.0)
}

fn view_height(app: &App) -> usize {
    app.group().active_tab().unwrap().view.height
}

/// Cmd+J shows the panel under the editor (the list focused, files sorted by path, each with
/// its problems by position, the count in the tab) and hides it again; the editor's view
/// shrinks by the panel's height and still keeps the cursor on screen.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_problems_panel_toggles_and_lists() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, root) = problems_project(dir.path()).await;
    let mut term = big_terminal();
    draw(&mut term, &mut app);
    assert!(find_cells(&term, "PROBLEMS").is_none(), "hidden at first");
    let full = view_height(&app);

    app.press(K::Char('j'), cmd());
    assert!(app.ui.panel_visible);
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    draw(&mut term, &mut app);
    let panel = app.hits.regions.panel.expect("drawn");
    assert_eq!(panel.height, crate::app::PANEL_HEIGHT);
    assert_eq!(panel.y + panel.height, 29, "above the status bar");
    let (x, y) = find_cells(&term, "PROBLEMS").expect("its tab");
    assert_eq!(y, panel.y + 1);
    assert!(
        screen_row(&term, x, y).starts_with("PROBLEMS  4 "),
        "{}",
        screen_row(&term, x, y)
    );
    let rows = panel_rows(&term, &app);
    assert_eq!(rows.len(), 7, "{rows:#?}");
    assert!(rows[0].starts_with("a.cpp") && rows[0].ends_with(" 2"), "{}", rows[0]);
    assert_eq!(
        rows[1],
        "⊗ use of undeclared identifier 'foo'  clang(undeclared_var_use) [Ln 1, Col 9]"
    );
    assert_eq!(rows[2], "⚠ unused bar  [Ln 2, Col 21]");
    assert!(rows[3].starts_with("c.cpp") && rows[3].ends_with(" 1"));
    assert_eq!(rows[4], "ⓘ c info  [Ln 1, Col 9]");
    assert!(
        rows[5].starts_with("b.cpp sub") && rows[5].ends_with(" 1"),
        "{}",
        rows[5]
    );
    assert_eq!(rows[6], "⊗ b error  [Ln 3, Col 9]");
    // Severity icons in their colours.
    let (ix, iy) = find_cells(&term, "⚠ unused").unwrap();
    let warning_icon = app.theme.get("problemsWarningIcon.foreground").fg;
    assert_eq!(term.backend().buffer()[(ix, iy)].fg, warning_icon.unwrap());
    // The editor gave up the panel's rows.
    assert_eq!(view_height(&app), full - panel.height as usize);

    // A long file: the cursor stays on screen above the panel.
    open(&mut app, &root.join("long.txt"));
    for _ in 0..40 {
        app.press(K::Down, M::NONE);
    }
    draw(&mut term, &mut app);
    let view = &app.group().active_tab().unwrap().view;
    assert_eq!(view.height, full - panel.height as usize);
    assert!(
        view.top_line() <= 40 && 40 < view.top_line() + view.height,
        "top {}",
        view.top_line()
    );
    let cursor = term.get_cursor_position().unwrap();
    assert!(cursor.y < panel.y, "the cursor is drawn above the panel: {cursor:?}");

    app.press(K::Char('j'), cmd());
    assert!(!app.ui.panel_visible);
    draw(&mut term, &mut app);
    assert!(find_cells(&term, "PROBLEMS").is_none());
    assert_eq!(view_height(&app), full);
    app.press(K::Char('j'), cmd());
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    app.press(K::Char('j'), cmd());
    assert_eq!(
        app.ui.focus,
        crate::app::Focus::Editor,
        "hiding it gives the editor the keys"
    );
    app.shutdown_servers().await;
}

/// The selection stays on its problem when a publish adds a file above it; Enter opens the
/// selected problem with its range selected (a file not open yet too), also after edits moved
/// it; a click opens a problem keeping the list focused, a double click focuses the editor; a
/// long list scrolls to its last row.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_problems_open_and_keep_their_selection() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, root) = problems_project(dir.path()).await;
    let (pa, pc, pb) = (root.join("a.cpp"), root.join("c.cpp"), root.join("sub/b.cpp"));
    let mut term = big_terminal();
    app.press(K::Char('m'), cmd() | M::SHIFT);
    for _ in 0..4 {
        app.press(K::Down, M::NONE);
    }
    let selected = |app: &App| app.problems.list.selected().cloned();
    assert_eq!(selected(&app), Some((pc.clone(), 1)), "c's problem");

    // A file sorting above it gets problems: the same problem stays selected.
    fs::write(root.join("b0.cpp"), "int b0;\n").unwrap();
    publish(
        &app,
        Language::Cpp,
        &root.join("b0.cpp"),
        None,
        json!([diag(0, 4, 6, 2, "b0 warning")]),
    );
    assert!(app.settle(|a| a.lsp.diagnostics.files().count() == 4).await);
    assert_eq!(selected(&app), Some((pc.clone(), 1)));
    draw(&mut term, &mut app);
    let rows = panel_rows(&term, &app);
    let b0 = rows.iter().position(|r| r.starts_with("b0.cpp")).expect("b0.cpp's row");
    assert!(
        rows[b0 + 1].starts_with("⚠ b0 warning") && rows[b0 + 2].starts_with("c.cpp"),
        "{rows:#?}"
    );
    let (_, y) = find_cells(&term, "ⓘ c info").unwrap();
    let active_bg = app.theme.get("list.active").bg.unwrap();
    assert_eq!(
        term.backend().buffer()[(panel_x(&app) + 1, y)].bg,
        active_bg,
        "still the highlighted row"
    );

    // Enter: c.cpp (not open) opens with `oops` selected, the editor focused.
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(pc.as_path())).await);
    assert_eq!(selection(&app), (8, 12));
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);

    // a.cpp's error, after typing before it on its line: the mark moved, and so do the row's
    // column and what Enter selects.
    app.press(K::Char('m'), cmd() | M::SHIFT);
    app.press(K::Home, M::NONE);
    app.press(K::Down, M::NONE);
    assert_eq!(selected(&app), Some((pa.clone(), 1)));
    app.press(K::Enter, M::NONE);
    assert_eq!(active_path(&app).as_deref(), Some(pa.as_path()));
    assert_eq!(selection(&app), (8, 11));
    app.press(K::Home, M::NONE);
    app.type_text("abc ");
    draw(&mut term, &mut app);
    assert!(find_cells(&term, "clang(undeclared_var_use) [Ln 1, Col 13]").is_some());
    app.press(K::Char('m'), cmd() | M::SHIFT);
    app.press(K::Enter, M::NONE);
    assert_eq!(selection(&app), (12, 15));
    let text = app.active_buffer().unwrap().rope.to_string();
    assert_eq!(&text[12..15], "foo");

    // A click on a problem: opened, the list keeps the keys; a double click: the editor.
    draw(&mut term, &mut app);
    let bar = find_cells(&term, "⚠ unused bar").unwrap();
    click(&mut app, bar);
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    assert_eq!(selected(&app), Some((pa.clone(), 2)));
    let bar_at = text.find("bar;").unwrap();
    assert_eq!(selection(&app), (bar_at, bar_at + 3));
    app.press(K::Home, M::NONE); // in the list: the first row
    assert_eq!(selected(&app), Some((pa.clone(), 0)));
    click(&mut app, bar);
    click(&mut app, bar);
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    // A file row: a click only selects it; a double click opens its first problem (selected)
    // and focuses the editor.
    app.press(K::Char('m'), cmd() | M::SHIFT);
    draw(&mut term, &mut app);
    let a_row = panel_row_starting(&term, &app, "a.cpp").expect("a.cpp's row");
    click(&mut app, a_row);
    assert_eq!(selected(&app), Some((pa.clone(), 0)));
    assert_eq!(selection(&app), (bar_at, bar_at + 3), "not opened");
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    click(&mut app, a_row);
    assert_eq!(selection(&app), (12, 15), "its first problem, `foo`");
    assert_eq!(selected(&app), Some((pa.clone(), 1)), "which becomes the selection");
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    // A file not open yet, by a click (End scrolls the list down to it).
    app.press(K::Char('m'), cmd() | M::SHIFT);
    app.press(K::End, M::NONE);
    draw(&mut term, &mut app);
    let b = find_cells(&term, "⊗ b error").unwrap();
    click(&mut app, b);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(pb.as_path())).await);
    assert_eq!(selection(&app), (10, 14), "line 3, `nope`");
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);

    // A long list: End selects the last row and scrolls to it.
    let many: Vec<Value> = (0..300).map(|i| diag(i, 0, 1, 2, &format!("many {i}"))).collect();
    publish(&app, Language::Cpp, &root.join("c.cpp"), None, json!(many));
    assert!(
        app.settle(|a| a.lsp.diagnostics.counts().of(Severity::Warning) == 302)
            .await
    );
    app.press(K::End, M::NONE);
    draw(&mut term, &mut app);
    let rows = panel_rows(&term, &app);
    assert_eq!(rows.last().unwrap(), "⊗ b error  [Ln 3, Col 9]", "{rows:#?}");
    // Rows: a.cpp + 2, b0.cpp + 1, c.cpp + 300, sub/b.cpp + 1 = 308; 8 on screen, so End
    // shows rows 300..308 and two PageUps (7 rows each) select row 293, now the top one.
    let files = |a: &App| crate::problems::files(&a.lsp.diagnostics, &a.buffers).len();
    let selected_row = |a: &App| {
        a.problems
            .list
            .selected_row(&crate::problems::files(&a.lsp.diagnostics, &a.buffers))
    };
    assert_eq!((files(&app), selected_row(&app)), (4, Some(307)));
    app.press(K::PageUp, M::NONE);
    app.press(K::PageUp, M::NONE);
    assert_eq!(selected_row(&app), Some(293));
    draw(&mut term, &mut app);
    let rows = panel_rows(&term, &app);
    assert!(rows[0].starts_with("⚠ many 287 "), "{rows:#?}");
    assert!(rows[7].starts_with("⚠ many 294 "), "{rows:#?}");
    // The wheel over the list scrolls it (3 rows a notch), not the selection.
    let list_y = app.hits.regions.panel.unwrap().y + 4;
    app.mouse(crossterm::event::MouseEventKind::ScrollDown, 70, list_y);
    draw(&mut term, &mut app);
    let rows = panel_rows(&term, &app);
    assert!(rows[0].starts_with("⚠ many 290 "), "{rows:#?}");
    assert_eq!(selected_row(&app), Some(293));
    app.shutdown_servers().await;
}

fn panel_x(app: &App) -> u16 {
    app.hits.regions.panel.unwrap().x
}

/// The first cell of the panel's list row whose text starts with `prefix`.
fn panel_row_starting(
    term: &ratatui::Terminal<ratatui::backend::TestBackend>,
    app: &App,
    prefix: &str,
) -> Option<(u16, u16)> {
    let panel = app.hits.regions.panel?;
    (panel.y + 2..panel.y + panel.height).find_map(|y| {
        let row = screen_row(term, panel.x, y);
        let x = row.chars().take_while(|c| *c == ' ').count() as u16;
        row.trim_start().starts_with(prefix).then_some((panel.x + x, y))
    })
}

/// F8 / Shift+F8 go from the cursor to the next / previous problem (several starting at one
/// place one by one), on to the next file with problems (opening it), wrapping; a cursor at the
/// start, the message in the status bar, the Problems list's selection following; the focus
/// stays where it was.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_next_problem_goes_through_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, root) = problems_project(dir.path()).await;
    let (pa, pc, pb) = (root.join("a.cpp"), root.join("c.cpp"), root.join("sub/b.cpp"));
    // A hint where the error starts: both are visited.
    let mut foo = diag(0, 8, 11, 1, "use of undeclared identifier 'foo'");
    foo["source"] = json!("clang");
    publish(
        &app,
        Language::Cpp,
        &pa,
        None,
        json!([diag(1, 20, 23, 2, "unused bar"), foo, diag(0, 8, 9, 4, "a hint")]),
    );
    assert!(app.settle(|a| a.lsp.diagnostics.counts().of(Severity::Hint) == 1).await);
    // Wide enough for the whole message in the status bar.
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 30)).unwrap();
    let f8 = |app: &mut App, shift: bool| app.press(K::F(8), if shift { M::SHIFT } else { M::NONE });
    let at = |app: &App| (active_path(app).unwrap(), selection(app));
    let bar = A_CPP.find("bar;").unwrap();

    f8(&mut app, false);
    assert_eq!(at(&app), (pa.clone(), (8, 8)), "the hint, first by (start, end)");
    draw(&mut term, &mut app);
    assert!(status(&term).contains("… a hint  (1 of 3)"), "{}", status(&term));
    f8(&mut app, false);
    assert_eq!(at(&app), (pa.clone(), (8, 8)), "the error at the same place");
    draw(&mut term, &mut app);
    assert!(
        status(&term).contains("⊗ use of undeclared identifier 'foo'  clang  (2 of 3)"),
        "{}",
        status(&term)
    );
    f8(&mut app, false);
    assert_eq!(at(&app), (pa.clone(), (bar, bar)));
    f8(&mut app, false);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(pc.as_path())).await);
    assert_eq!(selection(&app), (8, 8));
    assert_eq!(app.problems.list.selected().cloned(), Some((pc.clone(), 1)));
    f8(&mut app, false);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(pb.as_path())).await);
    assert_eq!(selection(&app), (10, 10));
    f8(&mut app, false);
    assert_eq!(at(&app), (pa.clone(), (8, 8)), "wrapped to the first file");
    assert_eq!(app.problems.list.selected().cloned(), Some((pa.clone(), 1)));

    // Back.
    f8(&mut app, true);
    assert_eq!(at(&app), (pb.clone(), (10, 10)), "wrapped to the last file");
    f8(&mut app, true);
    assert_eq!(at(&app), (pc.clone(), (8, 8)));
    f8(&mut app, true);
    assert_eq!(at(&app), (pa.clone(), (bar, bar)));
    f8(&mut app, true);
    assert_eq!(at(&app), (pa.clone(), (8, 8)));
    draw(&mut term, &mut app);
    assert!(status(&term).contains("(2 of 3)"), "the error: {}", status(&term));
    // From the list, the focus stays there.
    app.press(K::Char('m'), cmd() | M::SHIFT);
    f8(&mut app, false);
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    assert_eq!(at(&app), (pa.clone(), (bar, bar)));
    // A cursor moved off the last one: from its position again.
    app.press(K::Esc, M::NONE);
    app.press(K::Up, M::NONE);
    app.press(K::Home, M::NONE);
    f8(&mut app, false);
    assert_eq!(at(&app), (pa.clone(), (8, 8)), "the hint again (by position)");
    app.shutdown_servers().await;
}

/// Keys and focus: Cmd+Shift+M gives the list the keys (typing does nothing to the buffer), Esc
/// and a click in the editor give them back, Cmd+Shift+M from the list hides the panel; the
/// status bar's counts toggle it; its tab focuses it; dragging its top border resizes it.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_problems_focus_and_mouse() {
    use crate::app::Focus;
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, _root) = problems_project(dir.path()).await;
    let mut term = big_terminal();
    let text = |app: &App| app.active_buffer().unwrap().rope.to_string();

    app.press(K::Char('m'), cmd() | M::SHIFT);
    assert_eq!((app.ui.panel_visible, app.ui.focus), (true, Focus::Panel));
    assert_eq!(
        app.problems
            .list
            .selected_row(&crate::problems::files(&app.lsp.diagnostics, &app.buffers)),
        Some(0)
    );
    app.type_text("xy");
    app.press(K::Backspace, M::NONE);
    assert_eq!(text(&app), A_CPP, "keys in the list don't edit");
    app.press(K::Down, M::NONE);
    assert_eq!(
        app.last_command,
        Some(crate::command::Command::Problems(crate::command::ProblemsCommand::Down))
    );
    assert!(app.ui.side_visible);
    app.press(K::Char('b'), cmd());
    assert!(!app.ui.side_visible);
    assert_eq!(
        app.ui.focus,
        Focus::Panel,
        "hiding the side panel leaves the keys in the list"
    );
    app.press(K::Char('b'), cmd());
    app.press(K::Esc, M::NONE);
    assert_eq!((app.ui.panel_visible, app.ui.focus), (true, Focus::Editor));
    app.press(K::Char('m'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, Focus::Panel);
    app.press(K::Char('m'), cmd() | M::SHIFT);
    assert_eq!((app.ui.panel_visible, app.ui.focus), (false, Focus::Editor));

    // The status bar's counts: show and focus, then hide.
    draw(&mut term, &mut app);
    let counts = find_cells(&term, "⊗ 2").unwrap();
    assert_eq!(counts.1, 29);
    click(&mut app, counts);
    assert_eq!((app.ui.panel_visible, app.ui.focus), (true, Focus::Panel));
    // A click in the editor gives it the keys; one on the panel's tab gives them back.
    draw(&mut term, &mut app);
    click(&mut app, (60, 3));
    assert_eq!(app.ui.focus, Focus::Editor);
    // Shown but not focused: the counts hide it all the same (Cmd+Shift+M would focus it).
    click(&mut app, counts);
    assert_eq!((app.ui.panel_visible, app.ui.focus), (false, Focus::Editor));
    click(&mut app, counts);
    assert_eq!((app.ui.panel_visible, app.ui.focus), (true, Focus::Panel));
    click(&mut app, (60, 3));
    app.press(K::Char('m'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, Focus::Panel, "shown, unfocused: Cmd+Shift+M focuses it");
    click(&mut app, (60, 3));
    assert_eq!(app.ui.focus, Focus::Editor);
    let tab = find_cells(&term, "PROBLEMS").unwrap();
    click(&mut app, tab);
    assert_eq!(app.ui.focus, Focus::Panel);
    draw(&mut term, &mut app);
    click(&mut app, counts);
    assert!(!app.ui.panel_visible);
    assert_eq!(app.ui.focus, Focus::Editor);

    // Dragging the top border up makes it taller (the editor smaller); it stops at the
    // editor's minimum.
    use crossterm::event::{MouseButton, MouseEventKind};
    app.press(K::Char('j'), cmd());
    draw(&mut term, &mut app);
    let panel = app.hits.regions.panel.unwrap();
    let before = view_height(&app);
    app.mouse(MouseEventKind::Down(MouseButton::Left), 70, panel.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), 70, panel.y - 5);
    app.mouse(MouseEventKind::Up(MouseButton::Left), 70, panel.y - 5);
    draw(&mut term, &mut app);
    assert_eq!(app.hits.regions.panel.unwrap().height, panel.height + 5);
    assert_eq!(view_height(&app), before - 5);
    let panel = app.hits.regions.panel.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), 70, panel.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), 70, 0);
    draw(&mut term, &mut app);
    assert_eq!(app.hits.regions.editor.height, crate::layout::MIN_EDITOR_HEIGHT);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), 70, 29);
    app.mouse(MouseEventKind::Up(MouseButton::Left), 70, 29);
    draw(&mut term, &mut app);
    assert_eq!(app.hits.regions.panel.unwrap().height, crate::layout::MIN_PANEL_HEIGHT);
    app.shutdown_servers().await;
}

/// Edge cases: F8 and the list's keys with no problems at all (a message, nothing else), a
/// problem in a file outside the root (listed with its whole folder, opened at its range), and
/// the panel on terminals too small for it and the editor (the editor keeps its rows first; no
/// panic drawing, moving, dragging or clicking).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_problems_edge_cases() {
    use crate::app::Focus;
    use crossterm::event::{MouseButton, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", A_CPP)]);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    let pa = root.join("a.cpp");
    open_synced(&mut app, &fake, &pa).await;
    let mut term = big_terminal();

    // No problems: F8 says so and moves nothing; the list's keys do nothing.
    app.press(K::F(8), M::NONE);
    assert_eq!(app.message.as_ref().map(|m| m.text.as_str()), Some("No problems"));
    assert_eq!((active_path(&app), selection(&app)), (Some(pa.clone()), (0, 0)));
    app.press(K::Char('j'), cmd());
    for key in [K::Down, K::End, K::PageDown, K::Up, K::Enter] {
        app.press(key, M::NONE);
    }
    assert_eq!(app.problems.list.selected(), None);
    assert_eq!(app.ui.focus, Focus::Panel);
    draw(&mut term, &mut app);
    assert!(find_cells(&term, "No problems have been detected in the workspace.").is_some());
    assert!(find_cells(&term, "PROBLEMS  ").is_some());
    let panel = app.hits.regions.panel.unwrap();
    click(&mut app, (panel.x + 5, panel.y + 3));
    assert_eq!((active_path(&app), selection(&app)), (Some(pa.clone()), (0, 0)));

    // A file outside the root: its whole folder is shown; Enter opens it at its range.
    let outside = workspace::normalize(dir.path()).join("outside");
    fs::create_dir_all(&outside).unwrap();
    let px = outside.join("x.cpp");
    fs::write(&px, "int x = zap;\n").unwrap();
    publish(&app, Language::Cpp, &px, None, json!([diag(0, 8, 11, 1, "zap?")]));
    assert!(app.settle(|a| a.lsp.diagnostics.files().count() == 1).await);
    app.press(K::Home, M::NONE);
    draw(&mut term, &mut app);
    let rows = panel_rows(&term, &app);
    let folder = outside.display().to_string();
    assert!(
        rows[0].starts_with(&format!("x.cpp {folder}")) && rows[0].ends_with(" 1"),
        "{rows:#?}"
    );
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(px.as_path())).await);
    assert_eq!(selection(&app), (8, 11));
    // F8 from it: the only file, so round to its own problem.
    app.press(K::F(8), M::NONE);
    assert_eq!((active_path(&app), selection(&app)), (Some(px.clone()), (8, 8)));

    // Tiny terminals: the editor keeps `MIN_EDITOR_HEIGHT` (or all there is), the panel only
    // what's left; drawing, keys, wheel, a border drag and clicks never panic.
    app.press(K::Char('m'), cmd() | M::SHIFT);
    for h in 4..=13 {
        for w in [30, 41] {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            draw(&mut term, &mut app);
            let r = app.hits.regions;
            let body = h - 1;
            assert_eq!(
                r.editor.height,
                body.min(crate::layout::MIN_EDITOR_HEIGHT.max(body.saturating_sub(app.ui.panel_height))),
                "{w}x{h}"
            );
            if let Some(p) = r.panel {
                assert_eq!(r.editor.height + p.height, body, "{w}x{h}");
                app.mouse(MouseEventKind::Down(MouseButton::Left), r.editor.x + 2, p.y);
                app.mouse(MouseEventKind::Drag(MouseButton::Left), r.editor.x + 2, 0);
                app.mouse(MouseEventKind::Up(MouseButton::Left), r.editor.x + 2, 0);
                draw(&mut term, &mut app);
                assert_eq!(
                    app.hits.regions.editor.height,
                    crate::layout::MIN_EDITOR_HEIGHT.min(body)
                );
                for y in p.y..h {
                    click(&mut app, (r.editor.x + 3, y));
                    app.mouse(MouseEventKind::ScrollDown, r.editor.x + 3, y);
                    draw(&mut term, &mut app);
                }
                app.ui.panel_height = crate::app::PANEL_HEIGHT;
            } else {
                assert_eq!(r.editor.height, body, "no room for the panel: {w}x{h}");
            }
            app.ui.focus = Focus::Panel;
            for key in [K::Down, K::PageDown, K::End, K::PageUp] {
                app.press(key, M::NONE);
                draw(&mut term, &mut app);
            }
        }
    }
    app.shutdown_servers().await;
}

// ---- 9e: hover and go to definition ----

/// `a.cpp` for the hover tests: twelve comment lines, then `int x = value;` (line 12).
fn hover_text() -> String {
    let mut text: String = (0..12).map(|i| format!("// line {i}\n")).collect();
    text.push_str("int x = value;\n");
    text
}

/// A project with `a.cpp` (`text`) open and synced with a fake server (UTF-8 unless `utf16`).
async fn lsp_project(dir: &Path, files: &[(&str, &str)], utf16: bool) -> (App, Arc<FakeLauncher>, PathBuf) {
    let root = project(dir, files);
    let fake = FakeLauncher::new(FakeOptions {
        utf8: !utf16,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    (app, fake, root)
}

fn put_cursor(app: &mut App, at: usize) {
    use crate::buffer::pos::ByteIdx;
    use crate::selection::{Selection, Selections};
    let (_, view) = app.active_mut().unwrap();
    view.selections = Selections::single(Selection::cursor(ByteIdx(at)));
}

fn show_hover(app: &mut App) {
    app.press(K::Char('i'), cmd() | M::ALT);
}

/// The popup's inside as drawn: one string per row (padding and trailing spaces trimmed).
fn popup_rows(term: &ratatui::Terminal<ratatui::backend::TestBackend>, app: &App) -> Vec<String> {
    let r = app.hits.hover.expect("popup drawn");
    let buf = term.backend().buffer();
    (r.y + 1..r.y + r.height - 1)
        .map(|y| {
            (r.x + 1..r.x + r.width - 1)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

/// The last `textDocument/hover` (or other) request's position.
fn last_position(fake: &FakeLauncher, method: &str) -> (u64, u64) {
    let p = fake.requests(method).last().expect("a request").clone();
    (
        p["position"]["line"].as_u64().unwrap(),
        p["position"]["character"].as_u64().unwrap(),
    )
}

/// Request ids of `method` received, in order.
fn request_ids(fake: &FakeLauncher, method: &str) -> Vec<i64> {
    fake.received()
        .into_iter()
        .filter(|m| m["method"] == method)
        .filter_map(|m| m["id"].as_i64())
        .collect()
}

const SENTENCE: &str = "The value of x, a long line that wraps because it is longer than the popup can be wide, \
                        which is at most seventy-two cells and here less";

/// The command asks for the cursor's position; the answer (markdown: a code block, a rule, a
/// paragraph) is drawn in a popup above the hovered word (its range from the server), in the
/// hover colours, as wide as the editor allows, the long paragraph wrapped at spaces. Near the
/// top it goes below; a long one is cut with "…" and scrolls with the wheel over it. A key, an
/// edit and Esc close it (Esc does nothing else); the hover is asked for again each time.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_hover_popup_from_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let text = hover_text();
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", &text)], false).await;
    let range = json!({ "start": { "line": 12, "character": 8 }, "end": { "line": 12, "character": 13 } });
    fake.respond(
        "textDocument/hover",
        json!({
            "contents": { "kind": "markdown", "value": format!("```cpp\nint value\n```\n---\n{SENTENCE}") },
            "range": range,
        }),
    );
    let mut term = terminal();
    let value_at = text.find("value").unwrap();
    put_cursor(&mut app, value_at + 2);
    draw(&mut term, &mut app);
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    show_hover(&mut app);
    assert!(
        app.settle(|a| a.hover.popup.is_some()).await,
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    assert_eq!(last_position(&fake, "textDocument/hover"), (12, 10));
    draw(&mut term, &mut app);
    let r = app.hits.hover.unwrap();
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    assert_eq!(r.y + r.height, vy, "just above the hovered row");
    assert_eq!(
        (r.x, r.width),
        (editor.x, editor.width),
        "as wide as the editor (pushed left)"
    );
    assert!(vx >= r.x);
    let rows = popup_rows(&term, &app);
    assert_eq!(rows[0], "int value");
    assert!(rows[1].chars().all(|c| c == '─'), "a rule: {rows:?}");
    assert_eq!(rows[2..].join(" "), SENTENCE, "wrapped at spaces: {rows:?}");
    assert!(rows.len() > 4);
    assert!(rows[2..].iter().all(|r| r.chars().count() <= editor.width as usize - 4));
    // Colours: the hover's background and code colour.
    let buf = term.backend().buffer();
    let (cx, cy) = (r.x + 2, r.y + 1);
    assert_eq!(
        buf[(cx, cy)].bg,
        app.theme.get("editorHoverWidget.background").bg.unwrap()
    );
    assert_eq!(buf[(cx, cy)].fg, app.theme.get("textPreformat.foreground").fg.unwrap());
    assert_eq!(buf[(r.x, r.y)].symbol(), "┌");
    assert_eq!(app.hits.hit(cx, cy), crate::layout::Hit::Hover);

    // A key closes it (and moves as usual).
    app.press(K::Right, M::NONE);
    assert!(app.hover.popup.is_none());
    draw(&mut term, &mut app);
    assert!(app.hits.hover.is_none());
    assert!(find_cells(&term, "int value").is_none());

    // An edit closes it (a paste: not a key).
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    app.handle_test_event(crossterm::event::Event::Paste("z".into()));
    assert!(app.hover.popup.is_none());
    app.press(K::Char('z'), cmd());

    // Esc closes only it: the cursors stay.
    app.press(K::Up, cmd() | M::ALT);
    let sels = app.group().active_tab().unwrap().view.selections.len();
    assert_eq!(sels, 2);
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    app.press(K::Esc, M::NONE);
    assert!(app.hover.popup.is_none());
    assert_eq!(
        app.group().active_tab().unwrap().view.selections.len(),
        2,
        "Esc only closed it"
    );
    app.press(K::Esc, M::NONE);
    assert_eq!(app.group().active_tab().unwrap().view.selections.len(), 1);
    assert_eq!(fake.requests("textDocument/hover").len(), 3);

    // Near the top: below the row.
    fake.respond(
        "textDocument/hover",
        json!({ "contents": "`int x`", "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 7 } } }),
    );
    put_cursor(&mut app, 4);
    draw(&mut term, &mut app);
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    let (_, line0) = find_cells(&term, "// line 0").unwrap();
    let r = app.hits.hover.unwrap();
    assert_eq!(r.y, line0 + 1, "below");
    assert_eq!((r.width, r.height), (5 + 4, 3), "as wide as its text: {r:?}");
    assert_eq!(popup_rows(&term, &app), ["int x"]);
    assert_eq!(r.x, find_cells(&term, "line 0").unwrap().0, "at the range's start");
    let buf = term.backend().buffer();
    assert_eq!(
        buf[(r.x + 2, r.y + 1)].fg,
        app.theme.get("textPreformat.foreground").fg.unwrap(),
        "inline code"
    );

    // A heading is bold, inline code in the code colour, the rest in the hover's text colour.
    fake.respond(
        "textDocument/hover",
        json!({ "contents": "### Head\nsome `code` here" }),
    );
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    assert_eq!(popup_rows(&term, &app), ["Head", "some code here"]);
    let r = app.hits.hover.unwrap();
    let buf = term.backend().buffer();
    let bold = app.theme.get("editorHoverWidget.heading").add_modifier;
    let (head, text) = ((r.x + 2, r.y + 1), (r.x + 2, r.y + 2));
    assert!(buf[head].modifier.contains(bold));
    assert!(!buf[text].modifier.contains(bold));
    assert_eq!(buf[text].fg, app.theme.get("editorHoverWidget.foreground").fg.unwrap());
    let code_x = r.x + 2 + "some ".len() as u16;
    assert_eq!(buf[(code_x, r.y + 2)].symbol(), "c");
    assert_eq!(
        buf[(code_x, r.y + 2)].fg,
        app.theme.get("textPreformat.foreground").fg.unwrap()
    );
    assert_eq!(
        buf[(code_x + 5, r.y + 2)].fg,
        app.theme.get("editorHoverWidget.foreground").fg.unwrap(),
        "after the code span"
    );
    app.press(K::Esc, M::NONE);

    // Long: cut to the room above, "…" in the border, the wheel scrolls it.
    let long: Vec<String> = (0..40).map(|i| format!("row {i}")).collect();
    fake.respond(
        "textDocument/hover",
        json!({ "contents": { "kind": "plaintext", "value": long.join("\n") }, "range": range }),
    );
    put_cursor(&mut app, value_at);
    draw(&mut term, &mut app);
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    let r = app.hits.hover.unwrap();
    assert_eq!((r.y, r.y + r.height), (editor.y, vy), "all the room above");
    let rows = popup_rows(&term, &app);
    assert_eq!(rows.len(), r.height as usize - 2);
    assert_eq!(rows[0], "row 0");
    let buf = term.backend().buffer();
    assert_eq!(buf[(r.x + r.width - 3, r.y + r.height - 1)].symbol(), "…", "more below");
    use crossterm::event::MouseEventKind as E;
    app.mouse(E::ScrollDown, r.x + 3, r.y + 2);
    assert!(app.hover.popup.is_some(), "the wheel over it scrolls it");
    draw(&mut term, &mut app);
    assert_eq!(popup_rows(&term, &app)[0], "row 3");
    let buf = term.backend().buffer();
    assert_eq!(buf[(r.x + r.width - 3, r.y)].symbol(), "…", "more above");
    for _ in 0..20 {
        app.mouse(E::ScrollDown, r.x + 3, r.y + 2);
    }
    draw(&mut term, &mut app);
    assert_eq!(
        popup_rows(&term, &app).last().unwrap(),
        "row 39",
        "down to the last row"
    );
    // A click on it does nothing (it doesn't reach the text under it).
    let before = selection(&app);
    click(&mut app, (r.x + 3, r.y + 2));
    assert!(app.hover.popup.is_some());
    assert_eq!(selection(&app), before);
    // The wheel elsewhere closes it.
    app.mouse(E::ScrollDown, r.x + 3, vy + 1);
    assert!(app.hover.popup.is_none());

    // Its view scrolled or its tab changed by something other than a key or the wheel (the
    // view revealing its cursor while drawing, a file opening): the next event closes it, here
    // a move over it (which itself keeps it).
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    let r = app.hits.hover.unwrap();
    app.mouse(E::Moved, r.x + 3, r.y + 2);
    assert!(app.hover.popup.is_some(), "a move over it keeps it");
    {
        let (buf, view) = app.active_mut().unwrap();
        let top = view.top_line();
        view.set_top_line(&buf.rope, top + 1);
    }
    app.mouse(E::Moved, r.x + 3, r.y + 2);
    assert!(app.hover.popup.is_none(), "scrolled: closed");
    {
        let (buf, view) = app.active_mut().unwrap();
        view.set_top_line(&buf.rope, 0);
    }
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    let r = app.hits.hover.unwrap();
    app.test_open(Buffer::from_str("other\n"));
    app.mouse(E::Moved, r.x + 3, r.y + 2);
    assert!(app.hover.popup.is_none(), "another tab: closed");
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

/// The popup is drawn over the find widget and takes the clicks there: they don't reach the
/// widget's input.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_hover_popup_over_the_find_widget() {
    let dir = tempfile::tempdir().unwrap();
    let text = hover_text();
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", &text)], false).await;
    let mut term = terminal();
    draw(&mut term, &mut app);
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    // One line as wide as the editor allows: the popup spans it.
    let wide = "w".repeat(editor.width as usize - 4);
    fake.respond("textDocument/hover", json!({ "contents": wide }));
    app.press(K::Char('f'), cmd());
    put_cursor(&mut app, text.find("line 3").unwrap());
    let (x, y) = find_cells(&term, "line 3").unwrap();
    click(&mut app, (x, y));
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    assert!(app.find.visible);
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    let find = app.hits.find.unwrap().area;
    let r = app.hits.hover.unwrap();
    assert!(r.intersects(find), "{r:?} over {find:?}");
    let on = (find.x + 2, find.y);
    assert!(r.contains(ratatui::layout::Position { x: on.0, y: on.1 }));
    click(&mut app, on);
    assert_eq!(
        app.ui.focus,
        crate::app::Focus::Editor,
        "the find widget didn't get the click"
    );
    assert!(app.hover.popup.is_some());

    // Any terminal size, a split beside it: drawn inside its group's editor or not at all.
    app.press(K::Char('\\'), cmd());
    app.press(K::Char('1'), cmd());
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    assert_eq!(app.hits.groups.len(), 2, "split");
    let mut drawn = 0;
    for (w, h) in [(30, 4), (31, 5), (34, 6), (40, 8), (60, 12), (80, 3), (200, 50)] {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        draw(&mut t, &mut app);
        if let Some(r) = app.hits.hover {
            drawn += 1;
            let group = app.hover.popup.as_ref().unwrap().group;
            let editor = app.hits.group(group).unwrap().editor;
            assert_eq!(r.intersection(editor), r, "{w}x{h}: {r:?} in {editor:?}");
        }
    }
    assert!(drawn >= 2, "{drawn}");
}

/// Resting the mouse on text asks for its hover once the timer fires (no request before);
/// the popup stays while the pointer moves within the word or onto the popup, and closes when it
/// moves off. Moving to another word while an answer is pending cancels it (`$/cancelRequest`)
/// and the next word's answer is the one shown. A click closes the popup.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_hover_on_mouse_rest() {
    use crossterm::event::MouseEventKind as E;
    let dir = tempfile::tempdir().unwrap();
    let text = hover_text();
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", &text)], false).await;
    fake.respond("textDocument/hover", json!({ "contents": "about it" }));
    let mut term = terminal();
    put_cursor(&mut app, 0);
    draw(&mut term, &mut app);
    let (vx, vy) = find_cells(&term, "value;").unwrap();
    app.mouse(E::Moved, vx + 2, vy);
    assert!(app.hover.rest.is_some(), "the timer runs");
    idle(&mut app).await;
    assert!(
        fake.requests("textDocument/hover").is_empty(),
        "nothing before the delay"
    );
    // Moving within the word keeps the timer; firing it early does nothing.
    let deadline = app.hover.rest.as_ref().unwrap().deadline;
    app.mouse(E::Moved, vx + 3, vy);
    assert_eq!(app.hover.rest.as_ref().unwrap().deadline, deadline);
    app.test_hover_tick(deadline - Duration::from_millis(1));
    assert!(app.pending_requests().is_empty());
    app.test_hover_tick(deadline);
    // Where the pointer came to rest on the word.
    assert_eq!(
        app.pending_requests(),
        [("mouse hover", text.find("value").unwrap() + 2)]
    );
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    assert_eq!(last_position(&fake, "textDocument/hover"), (12, 10));
    draw(&mut term, &mut app);
    let r = app.hits.hover.unwrap();
    assert_eq!(r.y + r.height, vy);
    assert_eq!(r.x, vx, "at the word under the pointer (no range from the server)");
    assert_eq!(popup_rows(&term, &app), ["about it"]);
    // Within the word, onto the popup: it stays.
    app.mouse(E::Moved, vx, vy);
    app.mouse(E::Moved, r.x + 2, r.y + 1);
    assert!(app.hover.popup.is_some());
    assert!(app.hover.rest.is_none());
    // Past the line's end: it closes.
    app.mouse(E::Moved, vx + 20, vy);
    assert!(app.hover.popup.is_none());
    assert!(app.hover.rest.is_none(), "no timer off the text");

    // A pending answer for a word the pointer left is cancelled.
    fake.hold("textDocument/hover", true);
    let (ix, iy) = find_cells(&term, "int x").unwrap();
    app.mouse(E::Moved, ix + 1, iy);
    app.test_hover_tick(Instant::now() + crate::hover::DELAY);
    assert_eq!(
        app.pending_requests(),
        [("mouse hover", text.find("int x").unwrap() + 1)]
    );
    assert!(
        app.settle(|_| request_ids(&fake, "textDocument/hover").len() == 2)
            .await
    );
    let first = *request_ids(&fake, "textDocument/hover").last().unwrap();
    app.mouse(E::Moved, vx, vy);
    assert!(app.pending_requests().is_empty(), "cancelled when the pointer moved on");
    app.test_hover_tick(Instant::now() + crate::hover::DELAY);
    assert_eq!(app.pending_requests().len(), 1);
    app.pump();
    assert!(
        app.settle(|_| fake.cancelled().contains(&first)).await,
        "{:?}",
        fake.cancelled()
    );
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    draw(&mut term, &mut app);
    assert_eq!(popup_rows(&term, &app), ["about it"]);
    assert_eq!(app.hits.hover.unwrap().x, vx, "the second word's popup");
    // A click elsewhere closes it.
    click(&mut app, (ix, iy));
    assert!(app.hover.popup.is_none());

    // Moving on to another word before the timer fires restarts it there; the event loop's
    // timer (not the test hook) then asks, no earlier than the delay.
    fake.hold("textDocument/hover", false);
    app.mouse(E::Moved, ix + 1, iy);
    assert!(app.hover.rest.is_some());
    app.mouse(E::Moved, vx + 1, vy);
    let rest = app.hover.rest.clone().unwrap();
    assert_eq!(rest.at.0, text.find("value").unwrap() + 1, "the second word");
    let asked = fake.requests("textDocument/hover").len();
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    assert!(Instant::now() >= rest.deadline);
    assert_eq!(fake.requests("textDocument/hover").len(), asked + 1, "asked once");
    assert_eq!(last_position(&fake, "textDocument/hover"), (12, 9));
    assert!(app.hover.rest.is_none(), "the timer is done");
    assert!(fake.errors().is_empty());
}

/// A newer hover replaces a pending one (`$/cancelRequest` for the old id, only the new answer
/// shown); a key cancels a pending one; an answer that arrives after the cursor moved (no event
/// in between to cancel it) is dropped; an empty answer to the command says so.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_hover_requests_superseded_cancelled_and_stale() {
    let dir = tempfile::tempdir().unwrap();
    let text = hover_text();
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", &text)], false).await;
    let value_at = text.find("value").unwrap();
    put_cursor(&mut app, value_at);
    fake.respond("textDocument/hover", json!({ "contents": "v" }));
    fake.hold("textDocument/hover", true);
    show_hover(&mut app);
    show_hover(&mut app);
    assert!(
        app.settle(|_| request_ids(&fake, "textDocument/hover").len() == 2 && !fake.cancelled().is_empty())
            .await
    );
    let ids = request_ids(&fake, "textDocument/hover");
    assert_eq!(fake.cancelled(), [ids[0]], "the first one, replaced");
    assert_eq!(app.pending_requests(), [("hover", value_at)]);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    assert!(app.pending_requests().is_empty());

    // A key cancels a pending one.
    app.press(K::Esc, M::NONE);
    show_hover(&mut app);
    app.press(K::Right, M::NONE);
    assert!(app.pending_requests().is_empty());
    assert!(
        app.settle(|_| request_ids(&fake, "textDocument/hover").len() == 3)
            .await
    );
    let last = *request_ids(&fake, "textDocument/hover").last().unwrap();
    assert!(app.settle(|_| fake.cancelled().contains(&last)).await);
    idle(&mut app).await;
    assert!(app.hover.popup.is_none());

    // Stale: the cursor moved without an event that would cancel it.
    put_cursor(&mut app, value_at);
    show_hover(&mut app);
    app.pump();
    assert!(
        app.settle(|_| request_ids(&fake, "textDocument/hover").len() == 4)
            .await
    );
    put_cursor(&mut app, value_at + 1);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await);
    idle(&mut app).await;
    assert!(app.hover.popup.is_none(), "the stale answer was dropped");

    // Nothing to show.
    fake.hold("textDocument/hover", false);
    fake.respond("textDocument/hover", Value::Null);
    show_hover(&mut app);
    assert!(
        app.settle(|a| a.message.as_ref().is_some_and(|m| m.text == "No hover information"))
            .await
    );
    assert!(app.hover.popup.is_none());

    // A hover doesn't replace a pending definition (a key, F12 too, drops a pending hover).
    fake.hold("textDocument/hover", true);
    app.press(K::F(12), M::NONE);
    show_hover(&mut app);
    assert_eq!(
        app.pending_requests(),
        [("definition", value_at + 1), ("hover", value_at + 1)]
    );
    // The definition request failed (the fake doesn't know it): "No definition found".
    assert!(
        app.settle(|a| a
            .message
            .as_ref()
            .is_some_and(|m| m.text == "No definition found for 'value'"))
            .await,
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    assert!(fake.errors().is_empty());
}

/// A definition answer that arrives after an edit that left the cursor where it was (Delete)
/// is dropped: the edit cancels it.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_definition_dropped_after_an_edit() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, root) = lsp_project(dir.path(), &[("a.cpp", DEF_A)], false).await;
    let use_at = DEF_A.find("= target").unwrap() + 2;
    put_cursor(&mut app, use_at);
    fake.respond("textDocument/definition", location(&root.join("a.cpp"), 0, 4, 10));
    fake.hold("textDocument/definition", true);
    app.press(K::F(12), M::NONE);
    assert!(
        app.settle(|_| fake.requests("textDocument/definition").len() == 1)
            .await
    );
    let id = *request_ids(&fake, "textDocument/definition").last().unwrap();
    app.press(K::Delete, M::NONE);
    assert_eq!(selection(&app), (use_at, use_at), "the cursor stayed");
    assert!(app.pending_requests().is_empty(), "the edit cancelled it");
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(app.settle(|_| fake.cancelled().contains(&id)).await);
    idle(&mut app).await;
    assert_eq!(selection(&app), (use_at, use_at), "no jump");
    assert!(fake.errors().is_empty());
}

fn location(path: &Path, line: u32, from: u32, to: u32) -> Value {
    json!({
        "uri": uri(path),
        "range": { "start": { "line": line, "character": from }, "end": { "line": line, "character": to } },
    })
}

/// The active view's cursor as (line, byte column).
fn cursor_line_col(app: &App) -> (usize, usize) {
    let buf = app.active_buffer().unwrap();
    let at = app.group().active_tab().unwrap().view.selections.primary().head;
    let line = crate::buffer::pos::line_of(&buf.rope, at);
    (line, at.0 - crate::buffer::pos::line_start(&buf.rope, line).0)
}

const DEF_A: &str = "int target = 1;\nint use = target;\nint other = 2;\n";
const DEF_B: &str = "int 😀x = 1;\n// 😀 here: target\nint z;\n";

/// F12 asks for the definition at the cursor (UTF-16 positions here). One place in the same
/// file: the cursor goes to its start; in a file not open: it's opened there (the column read
/// past an astral char); a `LocationLink` goes to its `targetSelectionRange`; nothing: "No
/// definition found for 'target'". Ctrl+Click (Cmd+Click) moves the cursor and asks too.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_go_to_definition() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, root) = lsp_project(dir.path(), &[("a.cpp", DEF_A), ("b.cpp", DEF_B)], true).await;
    let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
    let use_at = DEF_A.find("= target").unwrap() + 4;
    put_cursor(&mut app, use_at);
    fake.respond("textDocument/definition", location(&a, 0, 4, 10));
    app.press(K::F(12), M::NONE);
    assert!(
        app.settle(|a| cursor_line_col(a) == (0, 4)).await,
        "{:?}",
        cursor_line_col(&app)
    );
    assert_eq!(last_position(&fake, "textDocument/definition"), (1, 12));
    assert_eq!(
        fake.requests("textDocument/definition")[0]["textDocument"]["uri"],
        json!(uri(&a))
    );
    assert_eq!(selection(&app), (4, 4), "a cursor, nothing selected");
    app.press(K::Char('u'), cmd());
    assert_eq!(selection(&app), (use_at, use_at), "Cmd+U goes back");
    app.press(K::F(12), M::NONE);
    assert!(app.settle(|a| cursor_line_col(a) == (0, 4)).await);

    // Another file, not open: opened there; UTF-16 column 6 is after "int 😀".
    fake.respond("textDocument/definition", location(&b, 0, 6, 7));
    app.press(K::F(12), M::NONE);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(b.as_path())).await);
    assert_eq!(cursor_line_col(&app), (0, "int 😀".len()));
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);

    // A link: its selection range (the name), not the whole range.
    put_cursor(&mut app, 0);
    fake.respond(
        "textDocument/definition",
        json!([{
            "targetUri": uri(&b),
            "targetRange": { "start": { "line": 1, "character": 0 }, "end": { "line": 2, "character": 0 } },
            "targetSelectionRange": { "start": { "line": 1, "character": 12 }, "end": { "line": 1, "character": 18 } },
        }]),
    );
    app.press(K::F(12), M::NONE);
    assert!(app.settle(|a| cursor_line_col(a).0 == 1).await);
    assert_eq!(cursor_line_col(&app), (1, "// 😀 here: ".len()));

    // Nothing.
    app.press(K::Char('w'), cmd());
    assert_eq!(active_path(&app).as_deref(), Some(a.as_path()));
    put_cursor(&mut app, use_at);
    fake.respond("textDocument/definition", Value::Null);
    app.press(K::F(12), M::NONE);
    assert!(
        app.settle(|a| a
            .message
            .as_ref()
            .is_some_and(|m| m.text == "No definition found for 'target'"))
            .await,
        "{:?}",
        app.message.as_ref().map(|m| &m.text)
    );

    // Ctrl+Click: the cursor goes to the click, then to the definition.
    fake.respond("textDocument/definition", location(&a, 2, 4, 9));
    let mut term = terminal();
    draw(&mut term, &mut app);
    let (x, y) = find_cells(&term, "target;").unwrap();
    use crossterm::event::{MouseButton, MouseEventKind as E};
    app.mouse_with(E::Down(MouseButton::Left), x + 1, y, M::CONTROL);
    assert!(
        app.settle(|_| fake.requests("textDocument/definition").len() == 6)
            .await
    );
    assert_eq!(
        last_position(&fake, "textDocument/definition"),
        (1, 11),
        "asked at the click"
    );
    app.mouse_with(E::Up(MouseButton::Left), x + 1, y, M::CONTROL);
    assert!(app.settle(|a| cursor_line_col(a) == (2, 4)).await);

    // Ctrl+Shift+Click extends the selection and asks nothing; neither does a plain click.
    let asked = fake.requests("textDocument/definition").len();
    draw(&mut term, &mut app);
    let (x, y) = find_cells(&term, "target;").unwrap();
    app.mouse_with(E::Down(MouseButton::Left), x + 1, y, M::CONTROL | M::SHIFT);
    app.mouse_with(E::Up(MouseButton::Left), x + 1, y, M::CONTROL | M::SHIFT);
    let (anchor, head) = selection(&app);
    assert_ne!(anchor, head, "extended");
    click(&mut app, (x + 2, y));
    idle(&mut app).await;
    assert_eq!(fake.requests("textDocument/definition").len(), asked);
    assert!(fake.errors().is_empty());
}

/// Several places: the Locations tab of the bottom panel shows them grouped by file with each
/// place's line (a file not open read off the UI thread), focused on the first place; Down and
/// Enter open the next one (in a file not open) and focus the editor; a click on a row opens it
/// keeping the list focused; Esc goes back to the editor.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_definitions_list() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, root) = lsp_project(dir.path(), &[("a.cpp", DEF_A), ("b.cpp", DEF_B)], true).await;
    let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
    // a.cpp edited, not saved: its preview comes from the buffer, not the file.
    put_cursor(&mut app, 0);
    app.type_text("X");
    put_cursor(&mut app, DEF_A.find("= target").unwrap() + 5);
    fake.respond(
        "textDocument/definition",
        json!([location(&b, 1, 12, 18), location(&a, 0, 5, 11), location(&b, 0, 6, 7)]),
    );
    app.press(K::F(12), M::NONE);
    assert!(app.settle(|a| a.locations.is_some()).await);
    assert!(app.ui.panel_visible);
    assert_eq!(app.ui.panel_tab, crate::layout::PanelTab::Locations);
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    let l = app.locations.as_ref().unwrap();
    assert_eq!(l.total(), 3);
    assert_eq!(l.list.selected(), Some(&(a.clone(), 1)), "the first place");
    assert!(
        app.settle(|a| a.locations.as_ref().unwrap().unread().is_empty()).await,
        "b.cpp read for its previews"
    );
    let mut term = big_terminal();
    draw(&mut term, &mut app);
    let rows = panel_rows(&term, &app);
    assert_eq!(rows.len(), 5, "{rows:?}");
    assert!(rows[0].starts_with("a.cpp") && rows[0].ends_with('1'), "{rows:?}");
    assert_eq!(rows[1], "Xint target = 1;");
    assert!(rows[2].starts_with("b.cpp") && rows[2].ends_with('2'), "{rows:?}");
    // (A wide char's second cell reads as a space.)
    assert_eq!(rows[3], "int 😀 x = 1;");
    assert_eq!(rows[4], "// 😀  here: target");
    let tabs = screen_row(&term, 0, app.hits.regions.panel.unwrap().y + 1);
    assert!(tabs.contains("LOCATIONS  3 "), "{tabs:?}");
    assert!(tabs.contains("Definitions of 'target'"), "{tabs:?}");
    // The place is highlighted (UTF-16 range read past the astral char).
    let (x, y) = find_cells(&term, "x = 1;").unwrap();
    let buf = term.backend().buffer();
    let matched = app.theme.get("search.match").bg;
    assert_eq!(Some(buf[(x, y)].bg), matched);
    assert_ne!(Some(buf[(x + 1, y)].bg), matched);

    // Down, Enter: b.cpp's first place, the editor focused.
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(b.as_path())).await);
    assert_eq!(cursor_line_col(&app), (0, "int 😀".len()));
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    // A click on a row: opened, the list keeps the keys.
    draw(&mut term, &mut app);
    let panel = app.hits.regions.panel.unwrap();
    assert!(screen_row(&term, panel.x, panel.y + 6).contains("here: target"));
    click(&mut app, (panel.x + 5, panel.y + 6));
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    assert_eq!(cursor_line_col(&app), (1, "// 😀 here: ".len()));
    app.press(K::Esc, M::NONE);
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    // Cmd+Shift+M still switches to Problems; the Locations tab stays in the strip.
    app.press(K::Char('m'), cmd() | M::SHIFT);
    draw(&mut term, &mut app);
    let tabs = screen_row(&term, 0, app.hits.regions.panel.unwrap().y + 1);
    assert!(tabs.contains("PROBLEMS") && tabs.contains("LOCATIONS"), "{tabs:?}");

    // A file read for a replaced list is dropped; one for the current list is taken.
    let generation = app.locations.as_ref().unwrap().generation;
    let preview = |app: &App| {
        let item = &app.locations.as_ref().unwrap().files[1].items[0];
        item.preview.as_ref().unwrap().text.clone()
    };
    assert_eq!(preview(&app), "int 😀x = 1;");
    app.test_locations_read(generation - 1, vec![(b.clone(), ropey::Rope::from_str("old\n"))]);
    assert_eq!(preview(&app), "int 😀x = 1;", "another list's read");
    app.test_locations_read(generation, vec![(b.clone(), ropey::Rope::from_str("new\n"))]);
    assert_eq!(preview(&app), "new");

    // A longer list: the wheel over the panel scrolls it.
    app.press(K::Esc, M::NONE);
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    let many: Vec<Value> = (0..12).map(|i| location(&a, 2, i, i + 1)).collect();
    fake.respond("textDocument/definition", Value::Array(many));
    app.press(K::F(12), M::NONE);
    assert!(
        app.settle(|a| a.locations.as_ref().is_some_and(|l| l.total() == 12))
            .await
    );
    draw(&mut term, &mut app);
    let panel = app.hits.regions.panel.unwrap();
    let rows = panel_rows(&term, &app);
    assert!(rows[0].starts_with("a.cpp"), "{rows:?}");
    use crossterm::event::MouseEventKind as E;
    app.mouse(E::ScrollDown, panel.x + 5, panel.y + 4);
    draw(&mut term, &mut app);
    let scrolled = panel_rows(&term, &app);
    assert!(!scrolled[0].starts_with("a.cpp"), "scrolled: {scrolled:?}");
    assert!(fake.errors().is_empty());
}

/// A server that dies with a hover and a definition pending drops both: no popup, no jump, its
/// crash reported once.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_server_crash_drops_pending_requests() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", DEF_A)], false).await;
    fake.respond("textDocument/hover", json!({ "contents": "x" }));
    fake.hold("textDocument/hover", true);
    fake.respond("textDocument/definition", Value::Null);
    fake.hold("textDocument/definition", true);
    put_cursor(&mut app, 4);
    app.press(K::F(12), M::NONE);
    show_hover(&mut app);
    assert_eq!(app.pending_requests(), [("definition", 4), ("hover", 4)]);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/crash", Value::Null);
    assert!(
        app.settle(|a| matches!(a.lsp.server_for(Language::Cpp).unwrap().state, State::Failed))
            .await
    );
    assert!(app.pending_requests().is_empty());
    assert!(app.hover.popup.is_none());
    assert!(app.message.as_ref().is_some_and(|m| m.text.contains("stopped")));
    // Without a server the commands say so.
    app.press(K::F(12), M::NONE);
    assert_eq!(app.message.as_ref().unwrap().text, "No definition found for 'target'");
    show_hover(&mut app);
    assert_eq!(app.message.as_ref().unwrap().text, "No hover information");
}

/// A real server: hover on `word`'s use in `a` shows `hover` in the popup; F12 there opens
/// `def_file` at its definition (`def_line`, 0-based).
#[allow(clippy::too_many_arguments)]
async fn real_hover_and_definition(
    files: &[(&str, &str)],
    word: &str,
    hover: &str,
    def_file: &str,
    def_line: usize,
    configure: impl FnOnce(&mut App),
    process: &str,
) {
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), files);
    let mut app = open_app(&root, Arc::new(ProcessLauncher));
    configure(&mut app);
    let a = root.join(files[0].0);
    open(&mut app, &a);
    assert!(
        settle_for(&mut app, 30, |a| a
            .lsp
            .server_for(a.active_buffer().unwrap().syntax.language())
            .is_some_and(|s| s.encoding().is_some()))
        .await,
        "{process} initialized"
    );
    let text = files[0].1;
    let at = text.rfind(word).unwrap() + 1;
    put_cursor(&mut app, at);
    let mut term = terminal();
    draw(&mut term, &mut app);
    // The server may answer nothing until it has read the file: ask until it shows something.
    let t = Instant::now();
    while app.hover.popup.is_none() && t.elapsed() < Duration::from_secs(60) {
        show_hover(&mut app);
        settle_for(&mut app, 3, |a| a.hover.popup.is_some()).await;
    }
    draw(&mut term, &mut app);
    let rows = popup_rows(&term, &app).join("\n");
    assert!(rows.contains(hover), "{process} hover: {rows}");

    app.press(K::F(12), M::NONE);
    let def = root.join(def_file);
    assert!(
        settle_for(&mut app, 60, |a| active_path(a).as_deref() == Some(def.as_path())).await,
        "{process} went to the definition; message {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
    assert_eq!(cursor_line_col(&app).0, def_line, "{process}");
    app.press(K::Char('q'), cmd());
    app.shutdown_servers().await;
    if let Some(n) = children_named(process) {
        assert_eq!(n, 0, "no {process} left");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_clangd_hover_and_definition() {
    if which("clangd").is_none() {
        eprintln!("milestone_9_real_clangd_hover_and_definition: skipped, clangd isn't on PATH");
        return;
    }
    real_hover_and_definition(
        &[
            ("a.cpp", "#include \"b.h\"\nint main() {\n  return answer(1);\n}\n"),
            (
                "b.h",
                "// The answer.\ninline int answer(int x) {\n  return 41 + x;\n}\n",
            ),
        ],
        "answer",
        "int answer(int x)",
        "b.h",
        1,
        |_| {},
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_ty_hover_and_definition() {
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/ty");
    let Some(ty) = which("ty").or_else(|| venv.is_file().then_some(venv)) else {
        eprintln!("milestone_9_real_ty_hover_and_definition: skipped, ty isn't on PATH or in .venv/bin");
        return;
    };
    let configure = |app: &mut App| app.lsp.builtins.ty = ServerConfig::new(ty.to_str().unwrap(), &["server"]);
    real_hover_and_definition(
        &[
            ("a.py", "from b import answer\n\nprint(answer(1))\n"),
            ("b.py", "\ndef answer(x: int) -> int:\n    return 41 + x\n"),
        ],
        "answer",
        "def answer(x: int) -> int",
        "b.py",
        1,
        configure,
        "ty",
    )
    .await;
}
