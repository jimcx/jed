//! Milestone 9 end to end (9a, the `lsp` core; 9b, document sync; 9c, diagnostics; 9d, the
//! bottom panel and its Problems list; 9e, hover and go to definition; 9f, completion; 9g, find
//! references and rename symbol; 9h, format document and the done-when projects).
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
//!
//! 9f: typing a word asks for completions once the quick-suggestion timer fires (in code, not in
//! a comment or string, not with `quick_suggestions` off); a trigger character (`.`, `>`) asks at
//! once with `triggerKind` 2 (UTF-16 positions past an astral char); Ctrl+Space (also as Ctrl+@)
//! asks, "No suggestions." for an empty or failed answer or no server. The list is drawn below
//! the word with the labels lined up under it, kind icons and the typed chars in their colours,
//! the selected row's detail; typing filters it (fuzzy, by score then `sortText`) without asking
//! again unless the list is incomplete (`triggerKind` 3, the answer replacing it); Up / Down
//! wrap, PageUp / PageDown, the wheel scrolls it, a click inserts a row; Enter and Tab insert,
//! Esc, a space, Backspace or Home past the word's start, a click or the wheel elsewhere close
//! it. Inserting: `textEdit` (an `InsertReplaceEdit`'s insert range), `additionalTextEdits` in the
//! same undo step, a snippet with its placeholder selected, at every Cmd+D cursor (each replacing
//! its own word), next to auto-closed pairs (the closer still typed over), commit characters;
//! selecting resolves an item (its detail drawn), an item inserted before its resolve answered
//! gets the resolve's additional edits after (not once the buffer was edited again). Ranges are
//! read where the request was asked and followed through typing since, at every cursor: one that
//! also replaces the `.` before the word, one that runs past the cursor. A read-only buffer gets
//! nothing inserted. A newer request cancels the pending one, typing in the word keeps it,
//! leaving the word drops it and its answer; a quick suggestion's error says nothing. At ~63k cursors in 1 MB a key
//! with the list and inserting stay within M6's keystroke / line-command budgets (release). Real
//! clangd and ty list a struct member / module attribute after `.`.
//!
//! 9g: Shift+F12 asks for the references at the cursor (`includeDeclaration`, UTF-16 positions)
//! and lists them in the panel's LOCATIONS tab ("References to 'target'", previews, focused on
//! the first; one reference too), Enter opens one; none, or no server, says "No references found
//! for 'target'"; an answer after the cursor moved is dropped. F2 in the editor (the explorer's
//! F2 still renames files) asks `prepareRename`: its range's text, a placeholder or (for
//! `defaultBehavior`, or a server without prepare support) the word fills a prompt, selected;
//! `null` or an error refuses with a message, as does a read-only buffer or no word. Enter sends
//! `textDocument/rename` with the new name (Esc or the same name sends nothing); its
//! `WorkspaceEdit` (`documentChanges` with versions, or `changes`) is applied as one undo step
//! per buffer: the open file at three cursors and in a split's second view (all following the
//! text), another open file, and a file not open loaded into a tab beside the active one and
//! left unsaved (undo goes back to the file); the server's copies follow. File operations, a
//! read-only or changed (version) open file refuse the whole edit; `null` says "No result."; an
//! error "Rename failed: …"; an answer after the cursor moved is dropped. A file not open that is
//! missing or not valid UTF-8 isn't edited ("Rename not applied to …"), a CRLF one is; an emptied
//! name sends nothing, a buffer changed while the prompt was open cancels; a split's view follows
//! at once and typing right after the rename is an undo step of its own. Real clangd (with a
//! compilation database, for its index) and ty list references in two files and rename a
//! function across them.
//!
//! 9h: Shift+Alt+F (Linux Ctrl+Shift+I) sends `textDocument/formatting` (no position, the
//! settings' `tabSize` / `insertSpaces`); a whole-document answer is applied as only what it
//! changes (one undo step, cursors before it kept, after it shifted; the server's copy follows by
//! ranges), several edits as one undo step too; an error says "Format Document failed: …", `null`
//! changes nothing, an answer after an edit or a cursor move is dropped, a read-only buffer sends
//! nothing (and one made read-only before the answer refuses it), no server or no provider
//! says "There is no formatter for 'cpp' files installed.". Done when, against real servers
//! (skipped when absent; every `didChange` sent checked to carry ranges): a C++ project with
//! `compile_commands.json` (clangd) and an SCons project (ty, the extensionless `SConstruct` with
//! jed's SCons `__builtins__.pyi` on ty's search path, written outside the workspace) each get an
//! error typed in, followed through an edit before it and cleared by undo; completion of a member
//! (`env.Program` from the stub for SCons); hover; go to definition in another file; references
//! across the files; rename across them; format document (clangd's LLVM style; ty has no
//! formatter, which jed says).

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
    app.test_plain_whitespace();
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
    app.lsp.builtins.clangd = ServerConfig::new("jed-test-no-such-language-server", &[]);
    open(&mut app, &root.join("a.cpp"));
    assert!(app.settle(|a| a.message.is_some()).await);
    let message = app.message.take().unwrap();
    assert_eq!(message.level, Level::Error);
    assert_eq!(
        message.text,
        "Language server jed-test-no-such-language-server not found: install it, or set [lsp.cpp] command in .workspace"
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

/// Squiggles under the popup don't show through it: its cells take none of the text's underline.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_hover_popup_hides_squiggles_under_it() {
    let dir = tempfile::tempdir().unwrap();
    let text = hover_text();
    let (mut app, fake, root) = lsp_project(dir.path(), &[("a.cpp", &text)], false).await;
    let path = root.join("a.cpp");
    // Every comment line above `value` squiggled from end to end.
    let diags: Vec<Value> = (0..12).map(|line| diag(line, 0, 10, 1, "bad")).collect();
    publish(&app, Language::Cpp, &path, Some(1), json!(diags));
    assert!(app.settle(|a| marks(a, &path).len() == 12).await);
    fake.respond("textDocument/hover", json!({ "contents": "w".repeat(40) }));
    put_cursor(&mut app, text.find("value").unwrap());
    show_hover(&mut app);
    assert!(app.settle(|a| a.hover.popup.is_some()).await);
    let mut term = terminal();
    draw(&mut term, &mut app);
    let r = app.hits.hover.expect("popup drawn");
    let underlined: Vec<(u16, u16)> = (r.y..r.y + r.height)
        .flat_map(|y| (r.x..r.x + r.width).map(move |x| (x, y)))
        .filter(|&(x, y)| squiggle(&term, x, y).is_some())
        .collect();
    // The popup starts at `value`'s column 8, over the comments' last chars: left of it they show.
    assert!(squiggle(&term, r.x - 1, r.y + 1).is_some(), "squiggled beside {r:?}");
    assert_eq!(underlined, [], "no squiggle in the popup {r:?}");
    app.shutdown_servers().await;
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

// ---- 9f: completion ----

fn completion_list(items: Value, incomplete: bool) -> Value {
    json!({ "isIncomplete": incomplete, "items": items })
}

/// The labels of the list's rows, best first (empty when there's no list).
fn shown(app: &App) -> Vec<String> {
    app.completion.popup.as_ref().map_or_else(Vec::new, |p| {
        p.shown.iter().map(|&i| p.items[i].label.clone()).collect()
    })
}

fn selected_label(app: &App) -> Option<String> {
    let p = app.completion.popup.as_ref()?;
    p.selected_item().map(|i| i.label.clone())
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn ctrl_space(app: &mut App) {
    app.press(K::Char(' '), M::CONTROL);
}

fn list_shown(app: &App) -> bool {
    app.completion.popup.as_ref().is_some_and(|p| !p.shown.is_empty())
}

/// The list's rows as drawn (inside its border), trailing spaces trimmed.
fn completion_rows(term: &ratatui::Terminal<ratatui::backend::TestBackend>, app: &App) -> Vec<String> {
    let h = app.hits.completion.expect("list drawn");
    let buf = term.backend().buffer();
    (h.rows.y..h.rows.y + h.rows.height)
        .map(|y| {
            (h.rows.x..h.rows.x + h.rows.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// Waits until the fake got `n` completion requests.
async fn asked(app: &mut App, fake: &FakeLauncher, n: usize) {
    let got = app
        .settle(|_| fake.requests("textDocument/completion").len() >= n)
        .await;
    assert!(got, "{n} completion requests");
}

/// The last completion request's `context`.
fn last_context(fake: &FakeLauncher) -> Value {
    fake.requests("textDocument/completion").last().expect("a request")["context"].clone()
}

/// Typing a word asks once the quick-suggestion timer fires (code, not before it), at the cursor;
/// the list is drawn under the word, labels lined up with it, kind icons in their colours, the
/// typed chars highlighted, the selected row with its detail. Typing on filters it (no new
/// request), Up / Down wrap, Enter inserts the selected item (one undo step back to what was
/// typed); Tab inserts too; Esc only closes it; a space, Backspace past the word's start, Home
/// and a click elsewhere close it; Ctrl+Space (also as a legacy terminal's Ctrl+@) asks again,
/// listing everything sorted by `sortText` with an empty word.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_filters_and_inserts() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n    \n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    fake.respond(
        "textDocument/completion",
        completion_list(
            json!([
                { "label": "alpha", "kind": 5, "detail": "int alpha", "sortText": "1" },
                { "label": "alphabet", "kind": 3, "detail": "void alphabet()", "sortText": "2" },
                { "label": "beta", "kind": 6, "sortText": "0" },
                { "label": "Alpine", "kind": 7, "sortText": "3" },
            ]),
            false,
        ),
    );
    let mut term = terminal();
    put_cursor(&mut app, 17);
    draw(&mut term, &mut app);
    app.type_text("al");
    assert!(app.completion.quick.is_some(), "the timer runs");
    assert!(
        fake.requests("textDocument/completion").is_empty(),
        "nothing before it fires"
    );
    assert!(app.settle(list_shown).await);
    assert_eq!(fake.requests("textDocument/completion").len(), 1);
    assert_eq!(last_position(&fake, "textDocument/completion"), (1, 6));
    assert_eq!(last_context(&fake), json!({ "triggerKind": 1 }));
    assert_eq!(shown(&app), ["alpha", "alphabet", "Alpine"]);

    draw(&mut term, &mut app);
    let rows = completion_rows(&term, &app);
    assert_eq!(rows.len(), 3);
    assert!(
        rows[0].starts_with("d alpha ") && rows[0].ends_with("int alpha"),
        "{rows:?}"
    );
    assert_eq!(rows[1..], ["f alphabet", "C Alpine"], "detail only on the selected row");
    let h = app.hits.completion.unwrap();
    let text_rect = app.hits.group(app.groups.active).unwrap().text;
    assert_eq!(h.rect.y, text_rect.y + 2, "just below the cursor's line");
    assert_eq!(h.rows.x + 2, text_rect.x + 4, "the labels under the word");
    let buf = term.backend().buffer();
    let theme = &app.theme;
    let (lx, ly) = (h.rows.x + 2, h.rows.y);
    let highlight = theme.get("editorSuggestWidget.highlightForeground").fg;
    assert_eq!(buf[(lx, ly)].fg, highlight.unwrap(), "a typed");
    assert_eq!(buf[(lx + 1, ly)].fg, highlight.unwrap(), "l typed");
    assert_ne!(buf[(lx + 2, ly)].fg, highlight.unwrap(), "p not");
    assert_eq!(
        buf[(lx + 2, ly)].bg,
        theme.get("editorSuggestWidget.selectedBackground").bg.unwrap()
    );
    assert_eq!(
        buf[(lx, ly + 1)].bg,
        theme.get("editorSuggestWidget.background").bg.unwrap()
    );
    assert_eq!(
        buf[(h.rows.x, ly)].fg,
        theme.get("symbolIcon.variableForeground").fg.unwrap()
    );
    assert_eq!(
        buf[(h.rows.x, ly + 1)].fg,
        theme.get("symbolIcon.functionForeground").fg.unwrap()
    );
    assert_eq!(app.hits.hit(lx, ly), crate::layout::Hit::Completion);

    // Typing on filters; no new request.
    app.type_text("ph");
    assert_eq!(shown(&app), ["alpha", "alphabet"]);
    app.type_text("a");
    assert_eq!(shown(&app), ["alpha", "alphabet"]);
    app.press(K::Down, M::NONE);
    assert_eq!(selected_label(&app).as_deref(), Some("alphabet"));
    app.press(K::Down, M::NONE);
    assert_eq!(selected_label(&app).as_deref(), Some("alpha"), "wraps");
    app.press(K::Up, M::NONE);
    assert_eq!(selected_label(&app).as_deref(), Some("alphabet"), "wraps back");
    idle(&mut app).await;
    assert_eq!(fake.requests("textDocument/completion").len(), 1);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "int main() {\n    alphabet\n}\n");
    assert_eq!(selection(&app), (25, 25));
    assert!(app.completion.popup.is_none());
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "int main() {\n    alpha\n}\n", "one undo step");
    assert_eq!(selection(&app), (22, 22));

    // Ctrl+Space asks again; Down + Tab inserts.
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    assert_eq!(fake.requests("textDocument/completion").len(), 2);
    app.press(K::Down, M::NONE);
    app.press(K::Tab, M::NONE);
    assert_eq!(text(&app), "int main() {\n    alphabet\n}\n");
    app.press(K::Char('z'), cmd());

    // Esc only closes it.
    app.press(K::Char('@'), M::CONTROL); // a legacy terminal's Ctrl+Space
    assert!(app.settle(list_shown).await);
    app.press(K::Esc, M::NONE);
    assert!(app.completion.popup.is_none());
    assert_eq!((text(&app).len(), selection(&app)), (src.len() + 5, (22, 22)));

    // Left within the word keeps it (filtered by less), as does Home to its start (everything);
    // Home again leaves the word: closed.
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    app.press(K::Left, M::NONE);
    assert_eq!(shown(&app), ["alpha", "alphabet"]);
    app.press(K::Home, M::NONE);
    assert_eq!(shown(&app).len(), 4);
    app.press(K::Home, M::NONE);
    assert!(app.completion.popup.is_none());
    app.press(K::End, M::NONE);

    // A space closes it (and is typed).
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    app.type_text(" ");
    assert!(app.completion.popup.is_none());
    assert_eq!(text(&app), "int main() {\n    alpha \n}\n");

    // An empty word: everything, by sortText; Backspace past its start closes it.
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    assert_eq!(shown(&app), ["beta", "alpha", "alphabet", "Alpine"]);
    app.press(K::Backspace, M::NONE);
    assert!(app.completion.popup.is_none());

    // A click elsewhere closes it.
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    draw(&mut term, &mut app);
    click(&mut app, (text_rect.x + 1, text_rect.y));
    assert!(app.completion.popup.is_none());
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

fn range(l0: u32, c0: u32, l1: u32, c1: u32) -> Value {
    json!({ "start": { "line": l0, "character": c0 }, "end": { "line": l1, "character": c1 } })
}

/// A trigger character asks at once (`triggerKind` 2 and the char; UTF-16 positions past an
/// astral char); an item's `textEdit` and `additionalTextEdits` (an include at the top) go in as
/// one undo step; an `InsertReplaceEdit` uses its insert range; a snippet's placeholder is
/// selected (typing replaces it). Selecting an item resolves it (its detail is drawn); an item
/// inserted before its resolve answered gets its additional edits from the answer (own step).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_trigger_characters_and_edits() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  😀s\n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], true).await;
    fake.respond(
        "textDocument/completion",
        completion_list(
            json!([
                {
                    "label": "member", "kind": 5,
                    "textEdit": { "range": range(1, 6, 1, 6), "newText": "member" },
                    "additionalTextEdits": [{ "range": range(0, 0, 0, 0), "newText": "#include <s.h>\n" }],
                },
                {
                    "label": "method", "kind": 2, "insertTextFormat": 2,
                    "textEdit": { "insert": range(1, 6, 1, 6), "replace": range(1, 6, 1, 8), "newText": "method(${1:int n})$0" },
                },
            ]),
            false,
        ),
    );
    put_cursor(&mut app, 20);
    app.type_text(".");
    assert!(app.completion.quick.is_none(), "no timer");
    asked(&mut app, &fake, 1).await;
    assert_eq!(
        last_context(&fake),
        json!({ "triggerKind": 2, "triggerCharacter": "." }),
        "asked at once"
    );
    assert_eq!(last_position(&fake, "textDocument/completion"), (1, 6), "UTF-16");
    assert!(app.settle(list_shown).await);
    assert_eq!(shown(&app), ["member", "method"]);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "#include <s.h>\nint main() {\n  😀s.member\n}\n");
    let after_member = "#include <s.h>\nint main() {\n  😀s.member".len();
    assert_eq!(selection(&app), (after_member, after_member));
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "int main() {\n  😀s.\n}\n", "one step: the include too");

    // Ctrl+Space at the trigger, then typing filters; the snippet's placeholder is selected.
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    app.type_text("met");
    assert_eq!(shown(&app), ["method"]);
    app.press(K::Tab, M::NONE);
    assert_eq!(
        text(&app),
        "int main() {\n  😀s.method(int n)\n}\n",
        "the insert range, not replace"
    );
    let open = "int main() {\n  😀s.method(".len();
    assert_eq!(selection(&app), (open, open + "int n".len()));
    app.type_text("5");
    assert_eq!(text(&app), "int main() {\n  😀s.method(5)\n}\n");
    app.press(K::Char('z'), cmd());
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "int main() {\n  😀s.met\n}\n");

    // Resolve: the selected item is resolved; its detail is drawn.
    fake.respond(
        "textDocument/completion",
        completion_list(
            json!([{ "label": "metric", "kind": 6 }, { "label": "meter", "kind": 6 }]),
            false,
        ),
    );
    fake.respond(
        "completionItem/resolve",
        json!({ "label": "x", "detail": "resolved detail" }),
    );
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    assert!(
        app.settle(|a| a
            .completion
            .popup
            .as_ref()
            .unwrap()
            .selected_item()
            .unwrap()
            .detail
            .is_some())
            .await
    );
    let resolved = fake.requests("completionItem/resolve");
    assert_eq!(resolved.last().unwrap()["label"], "meter", "the selected item as sent");
    let mut term = terminal();
    draw(&mut term, &mut app);
    assert!(completion_rows(&term, &app)[0].ends_with("resolved detail"));
    app.press(K::Esc, M::NONE);

    // An item inserted before its resolve answered: the answer's additional edits go in after.
    fake.respond(
        "completionItem/resolve",
        json!({ "label": "x", "additionalTextEdits": [{ "range": range(0, 0, 0, 0), "newText": "#include <m.h>\n" }] }),
    );
    fake.hold("completionItem/resolve", true);
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "int main() {\n  😀s.meter\n}\n");
    assert!(
        app.settle(|a| a.pending_requests().iter().any(|(k, _)| *k == "accepted resolve"))
            .await
    );
    fake.hold("completionItem/resolve", false);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(app.settle(|a| text(a).starts_with("#include <m.h>\n")).await);
    assert_eq!(text(&app), "#include <m.h>\nint main() {\n  😀s.meter\n}\n");
    let end = text(&app).find("meter").unwrap() + 5;
    assert_eq!(selection(&app), (end, end), "the cursor follows");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "int main() {\n  😀s.meter\n}\n", "its own step");
    // `>` is a trigger character too (the server decides what it means).
    app.press(K::End, M::NONE);
    let n = fake.requests("textDocument/completion").len();
    app.type_text("->");
    asked(&mut app, &fake, n + 1).await;
    assert_eq!(
        last_context(&fake),
        json!({ "triggerKind": 2, "triggerCharacter": ">" })
    );
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

/// An incomplete list asks again (`triggerKind` 3) each time the word changes, the new answer
/// replacing the list; a list with nothing matching waits for it.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_incomplete_lists_ask_again() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  \n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "one" }, { "label": "other" }]), true),
    );
    put_cursor(&mut app, 15);
    app.type_text("o");
    assert!(app.settle(list_shown).await);
    assert_eq!(shown(&app), ["one", "other"]);
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "only" }, { "label": "onward" }]), true),
    );
    app.type_text("n");
    assert_eq!(shown(&app), ["one"], "filtered at once");
    asked(&mut app, &fake, 2).await;
    assert_eq!(last_context(&fake), json!({ "triggerKind": 3 }));
    assert!(
        app.settle(|a| shown(a) == ["only", "onward"]).await,
        "{:?}",
        shown(&app)
    );
    // Nothing in the list matches "onx": it waits (hidden) for the next answer.
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "onyx" }, { "label": "onxbox" }]), false),
    );
    app.type_text("x");
    assert!(shown(&app).is_empty());
    asked(&mut app, &fake, 3).await;
    assert!(
        app.settle(|a| shown(a) == ["onxbox", "onyx"]).await,
        "{:?}",
        shown(&app)
    );
    // Complete now: typing on asks nothing more.
    app.type_text("b");
    idle(&mut app).await;
    assert_eq!(fake.requests("textDocument/completion").len(), 3);
    assert_eq!(shown(&app), ["onxbox"]);
}

/// A newer request cancels the pending one (`$/cancelRequest`); typing on in the word keeps a
/// pending request (its answer is filtered by what was typed since); leaving the word cancels
/// it and a late answer shows nothing; an error answer to Ctrl+Space says "No suggestions.", as
/// does an empty one or a buffer no server has.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_requests_superseded_and_stale() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  \n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "xylophone" }, { "label": "abc" }]), false),
    );
    fake.hold("textDocument/completion", true);
    put_cursor(&mut app, 15);
    ctrl_space(&mut app);
    asked(&mut app, &fake, 1).await;
    ctrl_space(&mut app);
    asked(&mut app, &fake, 2).await;
    let ids = request_ids(&fake, "textDocument/completion");
    assert!(
        app.settle(|_| fake.cancelled() == ids[..1]).await,
        "the first cancelled"
    );
    assert_eq!(app.pending_requests(), [("completion", 15)]);
    // Typing a word char keeps it.
    app.type_text("x");
    assert_eq!(app.pending_requests().len(), 1);
    assert!(app.completion.quick.is_none(), "no quick request while one is pending");
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(app.settle(list_shown).await);
    assert_eq!(shown(&app), ["xylophone"], "filtered by the x typed since");
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "int main() {\n  xylophone\n}\n");

    // Leaving the word drops the pending request; its late answer shows nothing.
    app.type_text(" ");
    ctrl_space(&mut app);
    asked(&mut app, &fake, 3).await;
    app.press(K::Left, M::NONE);
    assert!(app.pending_requests().is_empty());
    let ids = request_ids(&fake, "textDocument/completion");
    assert!(app.settle(|_| fake.cancelled().contains(&ids[2])).await);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert!(app.completion.popup.is_none());
    app.type_text("q");
    assert!(
        app.completion.quick.is_some(),
        "a dropped request doesn't block quick suggestions"
    );
    app.press(K::Backspace, M::NONE);

    // An empty answer to Ctrl+Space says so; an error answer too.
    fake.hold("textDocument/completion", false);
    fake.respond("textDocument/completion", json!([]));
    ctrl_space(&mut app);
    assert!(
        app.settle(|a| a.message.as_ref().is_some_and(|m| m.text == "No suggestions."))
            .await
    );
    assert!(app.completion.popup.is_none());
    // A plain text buffer has no server.
    app.test_open(Buffer::from_str("plain\n"));
    ctrl_space(&mut app);
    assert_eq!(app.message.as_ref().map(|m| m.text.as_str()), Some("No suggestions."));
    app.type_text("abc");
    assert!(app.completion.quick.is_none(), "no server: no timer");
}

/// At Cmd+D cursors the item goes in at every cursor (each replacing its own word: VS Code
/// replays the edit), a snippet's cursor inside its parens at each, one undo step; a click on a
/// row inserts that row's item; the wheel over the list scrolls it.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_at_every_cursor_and_by_mouse() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  fo;\n  fo;\n  f;\n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    let mut items = vec![
        json!({ "label": "foobar", "insertTextFormat": 2, "insertText": "foobar($0)", "sortText": "0" }),
        json!({ "label": "fox", "sortText": "1" }),
    ];
    items.extend((0..20).map(|i| json!({ "label": format!("fill{i:02}"), "sortText": format!("z{i:02}") })));
    fake.respond("textDocument/completion", completion_list(Value::Array(items), false));
    put_cursor(&mut app, 16);
    app.press(K::Char('d'), cmd());
    app.press(K::Char('d'), cmd());
    app.press(K::Right, M::NONE);
    {
        use crate::buffer::pos::ByteIdx;
        use crate::selection::{Selection, Selections};
        let (_, view) = app.active_mut().unwrap();
        let mut list: Vec<Selection> = view.selections.iter().copied().collect();
        // A third cursor after the lone `f` (one char to replace there, not the space before).
        list.push(Selection::cursor(ByteIdx(src.find("f;").unwrap() + 1)));
        view.selections = Selections::new(list, 0);
    }
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    assert_eq!(shown(&app), ["foobar", "fox"], "filtered by the primary's word");
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "int main() {\n  foobar();\n  foobar();\n  foobar();\n}\n");
    let heads: Vec<usize> = app
        .group()
        .active_tab()
        .unwrap()
        .view
        .selections
        .iter()
        .map(|s| s.head.0)
        .collect();
    let line = |n: usize| 13 + n * "  foobar();\n".len();
    assert_eq!(
        heads,
        [line(0) + 9, line(1) + 9, line(2) + 9],
        "inside each pair of parens"
    );
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), src, "one undo step");
    assert_eq!(app.group().active_tab().unwrap().view.selections.len(), 3);

    // By mouse: the wheel scrolls the list, a click inserts the row's item.
    let mut term = terminal();
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    draw(&mut term, &mut app);
    let h = app.hits.completion.unwrap();
    use crossterm::event::MouseEventKind as E;
    app.mouse(E::ScrollDown, h.rows.x + 2, h.rows.y);
    assert_eq!(
        app.completion.popup.as_ref().unwrap().top,
        0,
        "two items: nothing to scroll"
    );
    click(&mut app, (h.rows.x + 3, h.rows.y + 1));
    assert_eq!(text(&app), "int main() {\n  fox;\n  fox;\n  fox;\n}\n");
    assert!(app.completion.popup.is_none());
    app.press(K::Char('z'), cmd());

    // A long list scrolls under the wheel; the selection stays.
    app.press(K::Esc, M::NONE);
    put_cursor(&mut app, src.len() - 2);
    app.press(K::Enter, M::NONE);
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    let mut term = big_terminal();
    draw(&mut term, &mut app);
    let h = app.hits.completion.unwrap();
    assert_eq!(h.rows.height as usize, crate::completion::MAX_ROWS);
    let rows = completion_rows(&term, &app);
    assert!(rows[0].starts_with("t foobar"), "{rows:?}");
    let buf = term.backend().buffer();
    assert_eq!(
        buf[(h.rect.x + h.rect.width - 3, h.rect.y + h.rect.height - 1)].symbol(),
        "…"
    );
    app.mouse(E::ScrollDown, h.rows.x + 2, h.rows.y);
    assert_eq!(app.completion.popup.as_ref().unwrap().top, 3);
    assert_eq!(selected_label(&app).as_deref(), Some("foobar"));
    draw(&mut term, &mut app);
    assert!(completion_rows(&term, &app)[0].starts_with("t fill01"));
    // PageDown moves a page of rows; Up from the top wraps to the last.
    app.press(K::PageDown, M::NONE);
    assert_eq!(selected_label(&app).as_deref(), Some("fill10"));
    app.press(K::PageUp, M::NONE);
    app.press(K::Up, M::NONE);
    assert_eq!(selected_label(&app).as_deref(), Some("fill19"));
    // The wheel elsewhere closes it.
    app.mouse(E::ScrollDown, 1, 1);
    assert!(app.completion.popup.is_none());
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

/// Completion and auto-close / auto-indent: an item inserted inside an auto-closed pair keeps the
/// pair (its closer is typed over); a snippet's parens go inside it; a commit character inserts
/// the item and then itself (auto-closing); Enter with the list shown inserts, without it
/// breaks the line with its indentation.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_with_auto_close_and_indent() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  \n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "foo" }]), false),
    );
    put_cursor(&mut app, 15);
    app.type_text("print(");
    assert_eq!(text(&app), "int main() {\n  print()\n}\n", "auto-closed");
    app.type_text("fo");
    assert!(app.settle(list_shown).await);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "int main() {\n  print(foo)\n}\n");
    app.type_text(")");
    assert_eq!(
        text(&app),
        "int main() {\n  print(foo)\n}\n",
        "typed over the auto-closed paren"
    );
    assert_eq!(selection(&app).0, "int main() {\n  print(foo)".len());
    app.type_text(";");
    app.press(K::Enter, M::NONE);
    assert_eq!(
        text(&app),
        "int main() {\n  print(foo);\n  \n}\n",
        "Enter: a new line, indented"
    );

    // A snippet inside an auto-closed pair.
    fake.respond(
        "textDocument/completion",
        completion_list(
            json!([{ "label": "foo", "insertTextFormat": 2, "insertText": "foo($0)" }]),
            false,
        ),
    );
    app.type_text("print(fo");
    assert!(app.settle(list_shown).await);
    app.press(K::Tab, M::NONE);
    assert_eq!(text(&app), "int main() {\n  print(foo);\n  print(foo())\n}\n");
    assert_eq!(selection(&app).0, "int main() {\n  print(foo);\n  print(foo(".len());
    app.press(K::End, M::NONE);
    app.type_text(";");

    // A commit character: inserts the item, then itself.
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "foo", "commitCharacters": ["("] }]), false),
    );
    app.press(K::Enter, M::NONE);
    app.type_text("fo");
    assert!(app.settle(list_shown).await);
    app.type_text("(");
    assert_eq!(
        text(&app),
        "int main() {\n  print(foo);\n  print(foo());\n  foo()\n}\n",
        "foo, then ( auto-closed"
    );
    assert!(app.completion.popup.is_none());
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

/// Inserting an item at ~63k cursors (Cmd+Shift+L in a 1 MB file) stays within M6's
/// line-command budget, and so does typing with the list shown (release); one undo step.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_at_many_cursors_stays_fast() {
    /// M6's `LINE_COMMAND_BUDGET`.
    const LINE_COMMAND_BUDGET: Duration = Duration::from_millis(80);
    /// M6's `KEYSTROKE_BUDGET`.
    const KEYSTROKE_BUDGET: Duration = Duration::from_millis(16);
    let size = if cfg!(debug_assertions) { 250_000 } else { 1_000_000 };
    let mut src = String::new();
    let mut i = 0;
    while src.len() < size {
        src.push_str(&format!("int item = value_{i}; // item\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("big.cpp", &src)]);
    let path = root.join("big.cpp");
    let fake = FakeLauncher::new(FakeOptions::default());
    let mut app = open_app(&root, fake.clone());
    app.settings.syntax_max_bytes = 1000; // time completion, not tree-sitter
    open_synced(&mut app, &fake, &path).await;
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "items" }, { "label": "itemize" }]), false),
    );
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    for _ in 0..5 {
        app.press(K::Right, M::NONE);
    }
    app.press(K::Char('l'), cmd() | M::SHIFT);
    app.press(K::Right, M::NONE);
    let count = app.group().active_tab().unwrap().view.selections.len();
    assert_eq!(count, 2 * i);
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    let t = Instant::now();
    app.press(K::Char('i'), M::NONE);
    app.pump();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let key = t.elapsed();
    assert_eq!(shown(&app), ["itemize"]);
    let t = Instant::now();
    app.press(K::Enter, M::NONE);
    app.pump();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let accept = t.elapsed();
    println!("completion at {count} cursors: a key with the list {key:?}, inserting {accept:?}");
    assert!(text(&app).starts_with("int itemize = value_0; // itemize\n"));
    in_sync(&mut app, &fake, &path, "after inserting").await;
    app.press(K::Char('z'), cmd());
    assert!(
        text(&app).starts_with("int itemi = value_0; // itemi\n"),
        "one undo step"
    );
    if !cfg!(debug_assertions) {
        assert!(key < KEYSTROKE_BUDGET, "a key with the list too slow: {key:?}");
        assert!(accept < LINE_COMMAND_BUDGET, "inserting too slow: {accept:?}");
    }
    app.shutdown_servers().await;
}

/// A real server: typing `.` after `obj` in `a` lists `member`; Enter inserts it.
async fn real_completion(file: (&str, &str), obj: &str, member: &str, configure: impl FnOnce(&mut App), process: &str) {
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[file]);
    let mut app = open_app(&root, Arc::new(ProcessLauncher));
    configure(&mut app);
    open(&mut app, &root.join(file.0));
    assert!(
        settle_for(&mut app, 30, |a| a
            .lsp
            .server_for(a.active_buffer().unwrap().syntax.language())
            .is_some_and(|s| s.encoding().is_some()))
        .await,
        "{process} initialized"
    );
    let at = file.1.rfind(obj).unwrap() + obj.len();
    put_cursor(&mut app, at);
    // The server may answer nothing until it has read the file: type the dot until it lists.
    let t = Instant::now();
    while !shown(&app).iter().any(|l| l.trim() == member) && t.elapsed() < Duration::from_secs(60) {
        app.press(K::Esc, M::NONE);
        put_cursor(&mut app, at);
        if text(&app)[at..].starts_with('.') {
            app.press(K::Delete, M::NONE);
        }
        app.type_text(".");
        settle_for(&mut app, 5, |a| shown(a).iter().any(|l| l.trim() == member)).await;
    }
    assert!(
        shown(&app).iter().any(|l| l.trim() == member),
        "{process}: {:?}",
        shown(&app)
    );
    app.type_text(&member[..2]);
    let p = app.completion.popup.as_mut().unwrap();
    let row = p.shown.iter().position(|&i| p.items[i].label.trim() == member).unwrap();
    p.selected = row;
    app.press(K::Enter, M::NONE);
    let want = format!("{obj}.{member}");
    assert!(text(&app).contains(&want), "{process}: {}", text(&app));
    app.press(K::Char('q'), cmd());
    app.shutdown_servers().await;
    if let Some(n) = children_named(process) {
        assert_eq!(n, 0, "no {process} left");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_clangd_completion() {
    if which("clangd").is_none() {
        eprintln!("milestone_9_real_clangd_completion: skipped, clangd isn't on PATH");
        return;
    }
    real_completion(
        (
            "a.cpp",
            "struct Point {\n  int xcoord;\n  int ycoord;\n};\nint main() {\n  Point pt;\n  pt\n}\n",
        ),
        "pt",
        "ycoord",
        |_| {},
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_ty_completion() {
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/ty");
    let Some(ty) = which("ty").or_else(|| venv.is_file().then_some(venv)) else {
        eprintln!("milestone_9_real_ty_completion: skipped, ty isn't on PATH or in .venv/bin");
        return;
    };
    let configure = |app: &mut App| app.lsp.builtins.ty = ServerConfig::new(ty.to_str().unwrap(), &["server"]);
    real_completion(("a.py", "import os\n\nos\n"), "os", "getcwd", configure, "ty").await;
}

/// Quick suggestions: only in code (typing in a comment or string asks nothing when the timer
/// fires), not after typing moved on before it fired (the timer restarts), and not at all with
/// `quick_suggestions = false`.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_quick_suggestions_only_in_code() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  // note \n  const char *s = \"\";\n  \n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "abc" }]), false),
    );
    assert!(app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await);
    let later = || Instant::now() + crate::completion::QUICK_DELAY;
    // In a comment.
    put_cursor(&mut app, src.find("note").unwrap() + 5);
    app.type_text("ab");
    assert!(app.completion.quick.is_some());
    app.pump(); // the loop turn re-parses
    app.test_completion_tick(later());
    assert!(
        app.completion.quick.is_none() && app.pending_requests().is_empty(),
        "comment"
    );
    // In a string.
    put_cursor(&mut app, src.find("\"\"").unwrap() + 1 + 2);
    app.press(K::Left, M::NONE);
    app.press(K::Left, M::NONE);
    app.type_text("ab");
    app.pump();
    app.test_completion_tick(later());
    assert!(app.pending_requests().is_empty(), "string");
    // In code: asks.
    let code = text(&app).find("\n  \n").unwrap() + 3;
    put_cursor(&mut app, code);
    app.type_text("a");
    app.pump();
    app.test_completion_tick(Instant::now());
    assert!(app.pending_requests().is_empty(), "not before the deadline");
    app.type_text("b");
    app.pump();
    app.test_completion_tick(later());
    assert_eq!(app.pending_requests(), [("completion", code + 2)]);
    assert!(app.settle(list_shown).await);
    app.press(K::Esc, M::NONE);
    // Off.
    app.settings.quick_suggestions = false;
    app.type_text("c");
    assert!(app.completion.quick.is_none());
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await, "Ctrl+Space still asks");
}

/// An item's range read at every cursor after typing since the request: a `textEdit` that also
/// replaces the `.` before the word (`->foo`) at two cursors with the primary on the later line
/// (the typing on the earlier one moves its word), its `additionalTextEdits` mapped through that
/// typing; a range that runs past the cursor (`fo|x`) replaces the rest of the word at each
/// cursor, also after a char was typed in the word.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_ranges_follow_typing() {
    use crate::buffer::pos::ByteIdx;
    use crate::selection::{Selection, Selections};
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  s.\n  t.\n  fox;\n  fox;\n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    fake.respond(
        "textDocument/completion",
        completion_list(
            json!([{
                "label": "foo", "filterText": ".foo",
                "textEdit": { "range": range(2, 3, 2, 4), "newText": "->foo" },
                "additionalTextEdits": [{ "range": range(2, 2, 2, 2), "newText": "/*a*/" }],
            }]),
            false,
        ),
    );
    let set = |app: &mut App, cursors: &[usize], primary: usize| {
        let (_, view) = app.active_mut().unwrap();
        let list = cursors.iter().map(|&c| Selection::cursor(ByteIdx(c))).collect();
        view.selections = Selections::new(list, primary);
    };
    let dot = |n: usize| src.match_indices('.').nth(n).unwrap().0 + 1;
    set(&mut app, &[dot(0), dot(1)], 1);
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    assert_eq!(
        last_position(&fake, "textDocument/completion"),
        (2, 4),
        "at the primary"
    );
    app.type_text("f");
    assert_eq!(shown(&app), ["foo"], "`.f` matches `.foo`");
    app.press(K::Enter, M::NONE);
    assert_eq!(
        text(&app),
        "int main() {\n  s->foo\n  /*a*/t->foo\n  fox;\n  fox;\n}\n",
        "the dot replaced at both; the import where the server meant it"
    );
    app.press(K::Char('z'), cmd());
    assert_eq!(
        text(&app),
        "int main() {\n  s.f\n  t.f\n  fox;\n  fox;\n}\n",
        "one step"
    );

    // A range past the cursor: `fo|x` → `foobar` (the `x` too), after typing an `o`.
    fake.respond(
        "textDocument/completion",
        completion_list(
            json!([{ "label": "foobar", "textEdit": { "range": range(3, 2, 3, 5), "newText": "foobar" } }]),
            false,
        ),
    );
    let fox = |app: &App, n: usize| text(app).match_indices("fox").nth(n).unwrap().0 + 2;
    let cursors = [fox(&app, 0), fox(&app, 1)];
    set(&mut app, &cursors, 0);
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    app.type_text("o");
    assert_eq!(text(&app), "int main() {\n  s.f\n  t.f\n  foox;\n  foox;\n}\n");
    assert_eq!(shown(&app), ["foobar"]);
    app.press(K::Tab, M::NONE);
    assert_eq!(text(&app), "int main() {\n  s.f\n  t.f\n  foobar;\n  foobar;\n}\n");
    let heads: Vec<usize> = app
        .group()
        .active_tab()
        .unwrap()
        .view
        .selections
        .iter()
        .map(|s| s.head.0)
        .collect();
    let ends: Vec<usize> = text(&app).match_indices("foobar").map(|(i, _)| i + 6).collect();
    assert_eq!(heads, ends, "after each");
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

/// An error answer to Ctrl+Space says "No suggestions.", one to a quick suggestion says
/// nothing; the quick timer asks nothing once the text changed since the word char (Backspace);
/// a resolve answer for an inserted item is dropped once the buffer was edited after it; a
/// read-only buffer gets nothing inserted.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_completion_errors_late_resolves_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let src = "int main() {\n  \n}\n";
    let (mut app, fake, _root) = lsp_project(dir.path(), &[("a.cpp", src)], false).await;
    let later = || Instant::now() + crate::completion::QUICK_DELAY;
    put_cursor(&mut app, 15);
    // Unscripted, the fake answers completion with an error.
    app.type_text("ab");
    app.test_completion_tick(later());
    assert_eq!(app.pending_requests(), [("completion", 17)]);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await);
    assert!(app.message.is_none(), "a quick suggestion's error says nothing");
    ctrl_space(&mut app);
    assert!(
        app.settle(|a| a.message.as_ref().is_some_and(|m| m.text == "No suggestions."))
            .await
    );
    assert!(app.completion.popup.is_none());

    // The text changed after the word char: the timer asks nothing.
    app.type_text("c");
    assert!(app.completion.quick.is_some());
    app.press(K::Backspace, M::NONE);
    app.test_completion_tick(later());
    assert!(app.pending_requests().is_empty());
    assert_eq!(fake.requests("textDocument/completion").len(), 2);

    // An item's resolve answer after the buffer was edited again: its edits are dropped.
    fake.respond(
        "textDocument/completion",
        completion_list(json!([{ "label": "abacus" }]), false),
    );
    fake.respond(
        "completionItem/resolve",
        json!({ "label": "abacus", "additionalTextEdits": [{ "range": range(0, 0, 0, 0), "newText": "#include <a.h>\n" }] }),
    );
    fake.hold("completionItem/resolve", true);
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "int main() {\n  abacus\n}\n");
    assert!(
        app.settle(|a| a.pending_requests().iter().any(|(k, _)| *k == "accepted resolve"))
            .await
    );
    app.type_text(";");
    assert!(app.pending_requests().is_empty(), "the edit made it stale");
    fake.hold("completionItem/resolve", false);
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert_eq!(text(&app), "int main() {\n  abacus;\n}\n", "no include");

    // Read-only: the list may show, but nothing goes in.
    app.active_mut().unwrap().0.read_only = Some("test".into());
    put_cursor(&mut app, "int main() {\n  aba".len());
    ctrl_space(&mut app);
    assert!(app.settle(list_shown).await);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "int main() {\n  abacus;\n}\n");
    assert!(app.completion.popup.is_none());
    assert_eq!(app.message.as_ref().map(|m| m.text.as_str()), Some("Read-only: test"));
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

// ---- 9g: find references and rename symbol ----

/// Every selection of the active view as (anchor, head) byte offsets.
fn all_selections(app: &App) -> Vec<(usize, usize)> {
    let view = &app.group().active_tab().unwrap().view;
    view.selections.iter().map(|s| (s.anchor.0, s.head.0)).collect()
}

fn set_cursors(app: &mut App, at: &[usize], primary: usize) {
    use crate::buffer::pos::ByteIdx;
    use crate::selection::{Selection, Selections};
    let (_, view) = app.active_mut().unwrap();
    let list = at.iter().map(|&p| Selection::cursor(ByteIdx(p))).collect();
    view.selections = Selections::new(list, primary);
}

fn message(app: &App) -> Option<String> {
    app.message.as_ref().map(|m| m.text.clone())
}

/// The open rename prompt's kind and text.
fn rename_prompt(app: &App) -> Option<(crate::app::InputKind, String, bool)> {
    match &app.prompt {
        Some(Prompt::Input(i)) => Some((i.kind.clone(), i.field.text.clone(), i.field.all_selected)),
        _ => None,
    }
}

fn text_edit(line: u32, from: u32, to: u32, text: &str) -> Value {
    json!({
        "range": { "start": { "line": line, "character": from }, "end": { "line": line, "character": to } },
        "newText": text,
    })
}

fn doc_edit(path: &Path, version: Option<i32>, edits: Vec<Value>) -> Value {
    json!({ "textDocument": { "uri": uri(path), "version": version }, "edits": edits })
}

/// Shift+F12 asks for the references at the cursor (`includeDeclaration`, UTF-16 positions) and
/// shows them in the Locations list titled "References to 'target'", focused on the first, even
/// a single one; Enter opens one. None says "No references found for 'target'"; an answer
/// arriving after the cursor moved is dropped; without a server it says so at once.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_find_references() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, fake, root) = lsp_project(dir.path(), &[("a.cpp", DEF_A), ("b.cpp", DEF_B)], true).await;
    let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
    let use_at = DEF_A.find("= target").unwrap() + 4;
    put_cursor(&mut app, use_at);
    fake.respond(
        "textDocument/references",
        json!([location(&a, 0, 4, 10), location(&b, 1, 12, 18), location(&a, 1, 10, 16)]),
    );
    app.press(K::F(12), M::SHIFT);
    assert_eq!(app.pending_requests(), [("references", use_at)]);
    assert!(app.settle(|a| a.locations.is_some()).await);
    let params = fake.requests("textDocument/references").pop().unwrap();
    assert_eq!(params["context"]["includeDeclaration"], json!(true));
    assert_eq!(last_position(&fake, "textDocument/references"), (1, 12));
    let l = app.locations.as_ref().unwrap();
    assert_eq!(l.title, "References to 'target'");
    assert_eq!(l.total(), 3);
    assert_eq!(l.list.selected(), Some(&(a.clone(), 1)));
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    assert!(app.settle(|a| a.locations.as_ref().unwrap().unread().is_empty()).await);
    let mut term = big_terminal();
    draw(&mut term, &mut app);
    let rows = panel_rows(&term, &app);
    assert_eq!(rows.len(), 5, "{rows:?}");
    assert_eq!(rows[1], "int target = 1;");
    assert_eq!(rows[2], "int use = target;");
    assert_eq!(rows[4], "// 😀  here: target");
    let tabs = screen_row(&term, 0, app.hits.regions.panel.unwrap().y + 1);
    assert!(tabs.contains("References to 'target'"), "{tabs:?}");
    // The last one: b.cpp, opened at the place.
    app.press(K::End, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| active_path(a).as_deref() == Some(b.as_path())).await);
    assert_eq!(cursor_line_col(&app), (1, "// 😀 here: ".len()));
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);

    // One reference: still the list.
    app.press(K::PageUp, M::CONTROL);
    assert_eq!(active_path(&app).as_deref(), Some(a.as_path()));
    put_cursor(&mut app, 5);
    fake.respond("textDocument/references", json!([location(&a, 0, 4, 10)]));
    app.press(K::F(12), M::SHIFT);
    assert!(
        app.settle(|a| a.locations.as_ref().is_some_and(|l| l.total() == 1))
            .await
    );
    assert_eq!(app.ui.focus, crate::app::Focus::Panel);
    app.press(K::Esc, M::NONE);

    // None.
    fake.respond("textDocument/references", json!([]));
    app.press(K::F(12), M::SHIFT);
    assert!(
        app.settle(|a| message(a).as_deref() == Some("No references found for 'target'"))
            .await
    );

    // Stale: the cursor moved before the answer.
    fake.respond(
        "textDocument/references",
        json!([location(&a, 2, 4, 9), location(&a, 0, 4, 10)]),
    );
    fake.hold("textDocument/references", true);
    app.press(K::F(12), M::SHIFT);
    assert!(
        app.settle(|_| fake.requests("textDocument/references").len() == 4)
            .await
    );
    app.press(K::Right, M::NONE);
    assert!(app.pending_requests().is_empty(), "moving cancelled it");
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert_eq!(app.locations.as_ref().unwrap().total(), 1, "the old list stays");
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);

    // No server: said at once.
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/crash", Value::Null);
    assert!(
        app.settle(|a| matches!(a.lsp.server_for(Language::Cpp).unwrap().state, State::Failed))
            .await
    );
    app.press(K::F(12), M::SHIFT);
    assert_eq!(message(&app).as_deref(), Some("No references found for 'target'"));
    assert!(fake.errors().is_empty());
}

const REN_B: &str = "int f() { return target; }\n";
const REN_C: &str = "// target\nint g = target + target;\n";

/// F2: prepareRename's range fills the prompt ("target", all selected; the explorer's F2 still
/// renames files); typing replaces it, Enter sends `textDocument/rename` with the new name at the
/// cursor. The answer's `documentChanges` are applied: the open a.cpp (three cursors, and a
/// second view in a split, all following the text) and b.cpp, each one undo step, and c.cpp,
/// not open, loaded off the UI thread into a tab beside the active one and left unsaved (the
/// file on disk unchanged), its undo going back to the file; the server's copies follow.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_rename_across_files() {
    let dir = tempfile::tempdir().unwrap();
    let files = [("a.cpp", DEF_A), ("b.cpp", REN_B), ("c.cpp", REN_C)];
    let (mut app, fake, root) = lsp_project(dir.path(), &files, false).await;
    let (a, b, c) = (root.join("a.cpp"), root.join("b.cpp"), root.join("c.cpp"));
    open_synced(&mut app, &fake, &b).await;
    app.press(K::PageUp, M::CONTROL);
    assert_eq!(active_path(&app).as_deref(), Some(a.as_path()));
    // A second view of a.cpp, its cursor on line 1's ';'.
    app.press(K::Char('\\'), cmd());
    let semi = DEF_A.find(';').unwrap() + DEF_A[DEF_A.find(';').unwrap() + 1..].find(';').unwrap() + 1;
    put_cursor(&mut app, semi);
    app.press(K::Char('1'), cmd());
    let line1 = DEF_A.find('\n').unwrap() + 1;
    set_cursors(&mut app, &[5, 12, line1 + 12], 0);

    // The explorer's F2 is still the file rename.
    app.press(K::Char('e'), cmd() | M::SHIFT);
    app.press(K::F(2), M::NONE);
    assert!(matches!(
        rename_prompt(&app),
        Some((crate::app::InputKind::Rename { .. }, _, _))
    ));
    app.press(K::Esc, M::NONE);
    app.press(K::Char('1'), cmd());
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);

    fake.respond(
        "textDocument/prepareRename",
        json!({ "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 10 } }),
    );
    app.press(K::F(2), M::NONE);
    assert_eq!(app.pending_requests(), [("prepare rename", 5)]);
    assert!(app.settle(|a| a.prompt.is_some()).await);
    let (kind, name, selected) = rename_prompt(&app).unwrap();
    assert!(matches!(kind, crate::app::InputKind::RenameSymbol { .. }), "{kind:?}");
    assert_eq!((name.as_str(), selected), ("target", true));
    assert_eq!(last_position(&fake, "textDocument/prepareRename"), (0, 5));
    let mut term = big_terminal();
    draw(&mut term, &mut app);
    assert!(find_cells(&term, "Rename 'target' to").is_some());

    let id_a = buffer_id(&app, &a);
    let version = app.lsp.version(id_a);
    assert!(version.is_some());
    fake.respond(
        "textDocument/rename",
        json!({ "documentChanges": [
            doc_edit(&a, version, vec![text_edit(1, 10, 16, "renamed"), text_edit(0, 4, 10, "renamed")]),
            doc_edit(&b, None, vec![text_edit(0, 17, 23, "renamed")]),
            doc_edit(&c, None, vec![text_edit(0, 3, 9, "renamed"), text_edit(1, 8, 14, "renamed"), text_edit(1, 17, 23, "renamed")]),
        ]}),
    );
    app.type_text("renamed");
    app.press(K::Enter, M::NONE);
    assert!(app.prompt.is_none());
    assert_eq!(app.pending_requests(), [("rename", 5)]);
    let renamed_a = "int renamed = 1;\nint use = renamed;\nint other = 2;\n";
    assert!(app.settle(|a| text(a) == renamed_a).await, "{}", text(&app));
    let params = fake.requests("textDocument/rename").pop().unwrap();
    assert_eq!(params["newName"], json!("renamed"));
    assert_eq!(
        (
            params["position"]["line"].clone(),
            params["position"]["character"].clone()
        ),
        (json!(0), json!(5))
    );
    // The cursors followed: inside the word kept their offset, the others moved with the text.
    assert_eq!(
        all_selections(&app),
        [(5, 5), (13, 13), (line1 + 1 + 12, line1 + 1 + 12)]
    );
    // c.cpp loads into a tab beside a.cpp, which stays active.
    assert!(app.settle(|a| a.buffers.find_path(&c).is_some()).await);
    let renamed_c = "// renamed\nint g = renamed + renamed;\n";
    let id_b = buffer_id(&app, &b);
    let id_c = buffer_id(&app, &c);
    assert_eq!(app.buffers.get(id_c).unwrap().rope.to_string(), renamed_c);
    assert_eq!(
        app.buffers.get(id_b).unwrap().rope.to_string(),
        "int f() { return renamed; }\n"
    );
    assert_eq!(active_path(&app).as_deref(), Some(a.as_path()));
    let tabs: Vec<_> = app.group().tabs.iter().filter_map(|t| t.buffer()).collect();
    assert_eq!(tabs, [id_a, id_c, id_b]);
    assert!(app.buffers.get(id_c).unwrap().is_dirty());
    assert_eq!(fs::read_to_string(&c).unwrap(), REN_C, "not saved");
    assert_eq!(
        message(&app).as_deref(),
        Some("Renamed 'target' to 'renamed': 6 edits in 3 files")
    );
    // The split's view of a.cpp followed too.
    let other = app.groups.ids()[1];
    let view = &app.groups.get(other).unwrap().active_tab().unwrap().view;
    assert_eq!(view.selections.primary().head.0, semi + 2);
    for path in [&a, &b, &c] {
        in_sync(&mut app, &fake, path, "after the rename").await;
    }

    // Undo: one step per buffer, only that buffer.
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), DEF_A);
    assert_eq!(all_selections(&app), [(5, 5), (12, 12), (line1 + 12, line1 + 12)]);
    assert_eq!(
        app.buffers.get(id_b).unwrap().rope.to_string(),
        "int f() { return renamed; }\n"
    );
    app.press(K::PageDown, M::CONTROL);
    assert_eq!(active_path(&app).as_deref(), Some(c.as_path()));
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), REN_C);
    assert!(!app.buffers.get(id_c).unwrap().is_dirty(), "back to the file");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), REN_C, "nothing more to undo");
    for path in [&a, &c] {
        in_sync(&mut app, &fake, path, "after undo").await;
    }
}

/// What stops a rename: prepareRename's `null` ("The element can't be renamed.") or error (its
/// message); Esc or the same name sends nothing; a read-only buffer refuses F2; an answer with
/// a file operation, a read-only or changed (version) open file refuses the whole edit; one
/// arriving after the cursor moved is dropped; `null` says "No result."; an error says "Rename
/// failed: …"; a placeholder fills the prompt.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_rename_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let files = [("a.cpp", DEF_A), ("b.cpp", REN_B)];
    let (mut app, fake, root) = lsp_project(dir.path(), &files, false).await;
    let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
    put_cursor(&mut app, 5);

    // Unscripted, prepareRename fails with the fake's message.
    app.press(K::F(2), M::NONE);
    assert!(
        app.settle(|a| message(a).as_deref() == Some("fake doesn't know textDocument/prepareRename"))
            .await
    );
    fake.respond("textDocument/prepareRename", Value::Null);
    app.press(K::F(2), M::NONE);
    assert!(
        app.settle(|a| message(a).as_deref() == Some("The element can't be renamed."))
            .await
    );
    assert!(app.prompt.is_none());

    // A placeholder fills the prompt; Esc and the same name send nothing.
    fake.respond(
        "textDocument/prepareRename",
        json!({ "range": { "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 10 } }, "placeholder": "tar" }),
    );
    app.press(K::F(2), M::NONE);
    assert!(app.settle(|a| a.prompt.is_some()).await);
    assert_eq!(rename_prompt(&app).unwrap().1, "tar");
    app.press(K::Esc, M::NONE);
    assert!(app.prompt.is_none());
    app.press(K::F(2), M::NONE);
    assert!(app.settle(|a| a.prompt.is_some()).await);
    app.press(K::Enter, M::NONE);
    assert!(app.prompt.is_none());
    idle(&mut app).await;
    assert!(fake.requests("textDocument/rename").is_empty());
    // `defaultBehavior`: the word.
    fake.respond("textDocument/prepareRename", json!({ "defaultBehavior": true }));
    app.press(K::F(2), M::NONE);
    assert!(app.settle(|a| a.prompt.is_some()).await);
    assert_eq!(rename_prompt(&app).unwrap().1, "target");

    // Unscripted, the rename fails.
    app.type_text("x");
    app.press(K::Enter, M::NONE);
    assert!(
        app.settle(|a| message(a).as_deref() == Some("Rename failed: fake doesn't know textDocument/rename"))
            .await
    );
    async fn rename(app: &mut App, name: &str) {
        app.press(K::F(2), M::NONE);
        assert!(app.settle(|a| a.prompt.is_some()).await);
        app.type_text(name);
    }
    async fn submit(app: &mut App, fake: &FakeLauncher) {
        let n = fake.requests("textDocument/rename").len();
        app.press(K::Enter, M::NONE);
        assert!(
            app.settle(|_| fake.requests("textDocument/rename").len() == n + 1)
                .await
        );
        assert!(app.settle(|a| a.pending_requests().is_empty()).await);
    }

    // `null`.
    fake.respond("textDocument/rename", Value::Null);
    rename(&mut app, "x").await;
    submit(&mut app, &fake).await;
    assert_eq!(message(&app).as_deref(), Some("No result."));

    // A file operation refuses it all.
    fake.respond(
        "textDocument/rename",
        json!({ "documentChanges": [
            doc_edit(&a, None, vec![text_edit(0, 4, 10, "x")]),
            { "kind": "create", "uri": uri(&root.join("new.cpp")) },
        ]}),
    );
    rename(&mut app, "x").await;
    submit(&mut app, &fake).await;
    assert_eq!(
        message(&app).as_deref(),
        Some("Rename failed: file operations (create) aren't supported")
    );
    assert_eq!(text(&app), DEF_A);

    // A read-only open file refuses it all (nothing changes in a.cpp either).
    open_synced(&mut app, &fake, &b).await;
    let id_b = buffer_id(&app, &b);
    app.buffers.get_mut(id_b).unwrap().read_only = Some("test".into());
    app.press(K::PageUp, M::CONTROL);
    put_cursor(&mut app, 5);
    fake.respond(
        "textDocument/rename",
        json!({ "changes": {
            uri(&a): [text_edit(0, 4, 10, "x")],
            uri(&b): [text_edit(0, 17, 23, "x")],
        }}),
    );
    rename(&mut app, "x").await;
    submit(&mut app, &fake).await;
    assert_eq!(
        message(&app).as_deref(),
        Some("Rename failed: b.cpp is read-only (test)")
    );
    assert_eq!(text(&app), DEF_A);
    // F2 in a read-only buffer.
    app.press(K::PageDown, M::CONTROL);
    let asked = fake.requests("textDocument/prepareRename").len();
    app.press(K::F(2), M::NONE);
    assert_eq!(message(&app).as_deref(), Some("Read-only: test"));
    assert!(app.prompt.is_none());
    app.buffers.get_mut(id_b).unwrap().read_only = None;
    app.press(K::PageUp, M::CONTROL);
    put_cursor(&mut app, 5);

    // An edit for another version of an open file.
    fake.respond(
        "textDocument/rename",
        json!({ "documentChanges": [doc_edit(&a, Some(99), vec![text_edit(0, 4, 10, "x")])] }),
    );
    rename(&mut app, "x").await;
    submit(&mut app, &fake).await;
    assert_eq!(
        message(&app).as_deref(),
        Some("Rename failed: a.cpp changed since it was asked")
    );
    assert_eq!(text(&app), DEF_A);
    assert_eq!(
        fake.requests("textDocument/prepareRename").len(),
        asked + 1,
        "none from the read-only buffer"
    );

    // Stale: the cursor moved after Enter.
    fake.respond(
        "textDocument/rename",
        json!({ "changes": { uri(&a): [text_edit(0, 4, 10, "x")] } }),
    );
    fake.hold("textDocument/rename", true);
    rename(&mut app, "x").await;
    app.press(K::Enter, M::NONE);
    assert_eq!(app.pending_requests(), [("rename", 5)]);
    app.press(K::Left, M::NONE);
    assert!(app.pending_requests().is_empty());
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert_eq!(text(&app), DEF_A, "dropped");
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

/// A server without prepareRename: F2 opens the prompt with the word at the cursor at once, and
/// says "The element can't be renamed." away from any word; edits in `changes` (UTF-16) apply.
/// A split's view of the file follows as soon as the answer is applied (not at the next key);
/// typing right after the rename is an undo step of its own.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_rename_without_prepare() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", "int 😀 = target;\n")]);
    let fake = FakeLauncher::new(FakeOptions {
        no_prepare_rename: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    let a = root.join("a.cpp");
    open_synced(&mut app, &fake, &a).await;
    // A second view in a split, its cursor after the ';'.
    app.press(K::Char('\\'), cmd());
    put_cursor(&mut app, "int 😀 = target;".len());
    app.press(K::Char('1'), cmd());
    put_cursor(&mut app, "int 😀 = tar".len());
    app.press(K::F(2), M::NONE);
    assert_eq!(rename_prompt(&app).unwrap().1, "target");
    fake.respond(
        "textDocument/rename",
        json!({ "changes": { uri(&a): [text_edit(0, 9, 15, "goal")] } }),
    );
    app.press(K::Char('g'), M::NONE);
    app.type_text("oal");
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| text(a) == "int 😀 = goal;\n").await, "{}", text(&app));
    let other = app.groups.ids()[1];
    let view = &app.groups.get(other).unwrap().active_tab().unwrap().view;
    assert_eq!(
        view.selections.primary().head.0,
        "int 😀 = goal;".len(),
        "the split followed"
    );
    assert_eq!(
        message(&app).as_deref(),
        Some("Renamed 'target' to 'goal': 1 edit in 1 file")
    );
    assert_eq!(last_position(&fake, "textDocument/rename"), (0, 12));
    assert!(fake.requests("textDocument/prepareRename").is_empty());
    assert_eq!(selection(&app).1, "int 😀 = goa".len());
    // Typing at once doesn't join the rename's undo step.
    app.type_text("s");
    assert_eq!(text(&app), "int 😀 = goasl;\n");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "int 😀 = goal;\n");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "int 😀 = target;\n");
    put_cursor(&mut app, "int 😀 ".len());
    app.press(K::F(2), M::NONE);
    assert!(app.prompt.is_none());
    assert_eq!(message(&app).as_deref(), Some("The element can't be renamed."));
}

/// Files not open: a CRLF one gets its edits; a missing one isn't created ("Rename not applied
/// to …"), nor is an invalid UTF-8 one (read-only) edited. An emptied name sends nothing; a
/// buffer changed while the prompt was open cancels the rename.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_rename_files_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let crlf_text = "int x = target;\r\nint y = target;\r\n";
    let files = [("a.cpp", DEF_A), ("crlf.cpp", crlf_text)];
    let (mut app, fake, root) = lsp_project(dir.path(), &files, false).await;
    let (a, crlf, bad, gone) = (
        root.join("a.cpp"),
        root.join("crlf.cpp"),
        root.join("bad.cpp"),
        root.join("gone.cpp"),
    );
    fs::write(&bad, b"int target = 1; // \xff\n").unwrap();
    put_cursor(&mut app, 5);
    fake.respond("textDocument/prepareRename", json!({ "defaultBehavior": true }));
    async fn prompt(app: &mut App) {
        app.press(K::F(2), M::NONE);
        assert!(app.settle(|a| a.prompt.is_some()).await);
    }

    // An emptied name sends nothing.
    prompt(&mut app).await;
    app.press(K::Backspace, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.prompt.is_none());
    idle(&mut app).await;
    assert!(fake.requests("textDocument/rename").is_empty());

    // The buffer changed while the prompt was open (as a background edit could).
    prompt(&mut app).await;
    app.type_text("x");
    {
        use crate::buffer::change::ChangeSet;
        use crate::buffer::pos::ByteIdx;
        use crate::buffer::undo::EditKind;
        let id = buffer_id(&app, &a);
        let buf = app.buffers.get_mut(id).unwrap();
        let mut set = ChangeSet::with_capacity(1, 1);
        let end = ByteIdx(buf.rope.len());
        set.push(end, end, "\n");
        let sels = crate::selection::Selections::single(crate::selection::Selection::cursor(ByteIdx(5)));
        buf.edit(set, sels.clone(), sels, EditKind::Other);
    }
    app.press(K::Enter, M::NONE);
    assert_eq!(message(&app).as_deref(), Some("The file changed: rename cancelled"));
    idle(&mut app).await;
    assert!(fake.requests("textDocument/rename").is_empty());
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), DEF_A);
    put_cursor(&mut app, 5);

    async fn rename_in(app: &mut App, fake: &FakeLauncher, path: &Path, edits: Vec<Value>) {
        fake.respond("textDocument/rename", json!({ "changes": { uri(path): edits } }));
        prompt(app).await;
        app.type_text("renamed");
        app.press(K::Enter, M::NONE);
    }

    // Missing: nothing created.
    rename_in(&mut app, &fake, &gone, vec![text_edit(0, 4, 10, "renamed")]).await;
    let failed = format!("Rename not applied to {}: ", gone.display());
    assert!(
        app.settle(|a| message(a).is_some_and(|m| m.starts_with(&failed))).await,
        "{:?}",
        message(&app)
    );
    assert!(app.buffers.find_path(&gone).is_none());
    assert!(!gone.exists());

    // Invalid UTF-8: loaded read-only, not edited.
    rename_in(&mut app, &fake, &bad, vec![text_edit(0, 4, 10, "renamed")]).await;
    let failed = format!("Rename not applied to {}: read-only (not valid UTF-8)", bad.display());
    assert!(
        app.settle(|a| message(a).as_deref() == Some(failed.as_str())).await,
        "{:?}",
        message(&app)
    );
    let id_bad = buffer_id(&app, &bad);
    assert!(
        app.buffers
            .get(id_bad)
            .unwrap()
            .rope
            .to_string()
            .starts_with("int target = 1;")
    );
    assert!(!app.buffers.get(id_bad).unwrap().is_dirty());

    // CRLF: edited, the line breaks kept.
    let edits = vec![text_edit(0, 8, 14, "renamed"), text_edit(1, 8, 14, "renamed")];
    rename_in(&mut app, &fake, &crlf, edits).await;
    let want = "int x = renamed;\r\nint y = renamed;\r\n";
    assert!(
        app.settle(|a| a
            .buffers
            .find_path(&crlf)
            .is_some_and(|id| a.buffers.get(id).unwrap().rope == want))
            .await,
        "{:?}",
        message(&app)
    );
    assert_eq!(active_path(&app).as_deref(), Some(a.as_path()));
    assert_eq!(text(&app), DEF_A);
    in_sync(&mut app, &fake, &crlf, "after the rename").await;
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
}

/// A real server: Shift+F12 on `word`'s use in the first file lists its references in both
/// files; F2 renames it to `new`, the first file edited and the other (not open) loaded with
/// its edit, unsaved: `expect` is each file's text after.
async fn real_references_and_rename(
    files: &[(&str, &str)],
    word: &str,
    new: &str,
    expect: &[(&str, &str)],
    configure: impl FnOnce(&mut App),
    process: &str,
) {
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), files);
    // clangd finds references in other files (and renames there) through its background index,
    // which needs a compilation database.
    let commands: Vec<Value> = files
        .iter()
        .filter(|(name, _)| name.ends_with(".cpp"))
        .map(|(name, _)| json!({ "directory": root, "file": name, "arguments": ["clang++", "-c", name] }))
        .collect();
    if !commands.is_empty() {
        fs::write(root.join("compile_commands.json"), Value::Array(commands).to_string()).unwrap();
    }
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
    let at = files[0].1.rfind(word).unwrap() + 1;
    put_cursor(&mut app, at);
    // The server may know nothing until it has read the files: ask until both are listed.
    let both = |a: &App| a.locations.as_ref().is_some_and(|l| l.files.len() == 2);
    let t = Instant::now();
    while !both(&app) && t.elapsed() < Duration::from_secs(60) {
        app.press(K::F(12), M::SHIFT);
        settle_for(&mut app, 3, both).await;
        app.press(K::Char('1'), cmd());
        put_cursor(&mut app, at);
    }
    assert!(
        both(&app),
        "{process} references: {:?}; message {:?}",
        app.locations.as_ref().map(|l| &l.files),
        message(&app)
    );
    assert_eq!(app.locations.as_ref().unwrap().title, format!("References to '{word}'"));

    app.press(K::F(2), M::NONE);
    assert!(
        settle_for(&mut app, 30, |a| a.prompt.is_some()).await,
        "{process} rename prompt; message {:?}",
        message(&app)
    );
    assert_eq!(rename_prompt(&app).unwrap().1, word);
    app.type_text(new);
    app.press(K::Enter, M::NONE);
    let done = |app: &App| {
        expect.iter().all(|(name, text)| {
            app.buffers
                .find_path(&root.join(name))
                .is_some_and(|id| app.buffers.get(id).unwrap().rope == *text)
        })
    };
    assert!(
        settle_for(&mut app, 30, done).await,
        "{process} renamed; message {:?}; texts {:?}",
        message(&app),
        app.buffers.iter().map(|(_, b)| b.rope.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(active_path(&app).as_deref(), Some(a.as_path()));
    for (name, text) in &files[1..] {
        assert_eq!(fs::read_to_string(root.join(name)).unwrap(), *text, "not saved");
    }
    app.shutdown_servers().await;
    if let Some(n) = children_named(process) {
        assert_eq!(n, 0, "no {process} left");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_clangd_references_and_rename() {
    if which("clangd").is_none() {
        eprintln!("milestone_9_real_clangd_references_and_rename: skipped, clangd isn't on PATH");
        return;
    }
    real_references_and_rename(
        &[
            ("a.cpp", "#include \"b.h\"\nint main() {\n  return answer(1);\n}\n"),
            ("b.h", "inline int answer(int x) {\n  return 41 + x;\n}\n"),
        ],
        "answer",
        "solution",
        &[
            ("a.cpp", "#include \"b.h\"\nint main() {\n  return solution(1);\n}\n"),
            ("b.h", "inline int solution(int x) {\n  return 41 + x;\n}\n"),
        ],
        |_| {},
        "clangd",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_real_ty_references_and_rename() {
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/ty");
    let Some(ty) = which("ty").or_else(|| venv.is_file().then_some(venv)) else {
        eprintln!("milestone_9_real_ty_references_and_rename: skipped, ty isn't on PATH or in .venv/bin");
        return;
    };
    let configure = |app: &mut App| app.lsp.builtins.ty = ServerConfig::new(ty.to_str().unwrap(), &["server"]);
    real_references_and_rename(
        &[
            ("a.py", "from b import answer\n\nprint(answer(1))\n"),
            ("b.py", "\ndef answer(x: int) -> int:\n    return 41 + x\n"),
        ],
        "answer",
        "solution",
        &[
            ("a.py", "from b import solution\n\nprint(solution(1))\n"),
            ("b.py", "\ndef solution(x: int) -> int:\n    return 41 + x\n"),
        ],
        configure,
        "ty",
    )
    .await;
}

/// Shift+Alt+F on macOS, Ctrl+Shift+I on Linux (VS Code's).
fn format_document(app: &mut App) {
    if cfg!(target_os = "macos") {
        app.press(K::Char('f'), M::SHIFT | M::ALT);
    } else {
        app.press(K::Char('i'), M::CONTROL | M::SHIFT);
    }
}

fn cursor_offsets(app: &App) -> Vec<usize> {
    app.group()
        .active_tab()
        .unwrap()
        .view
        .selections
        .iter()
        .map(|s| s.head.0)
        .collect()
}

const UNFORMATTED: &str = "int main() {\nreturn 1;\n}\n// 😀 end\n";
const FORMATTED: &str = "int main() {\n    return 1;\n}\n// 😀 end\n";

/// Format document sends `textDocument/formatting` (no position; `tabSize` / `insertSpaces` from
/// the `.workspace` settings); an answer replacing the whole document is applied as only what
/// changed: one undo step, cursors before the change kept and after it shifted, the server's copy
/// following by incremental `didChange`s; several edits are one undo step too. An error says
/// "Format Document failed: …"; `null` changes nothing; an answer after the text or the cursor
/// changed is dropped (the request cancelled); a read-only buffer sends nothing, and an answer
/// reaching a buffer made read-only meanwhile is refused; a server without formatting, or no
/// server, says "There is no formatter for 'cpp' files installed."
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_format_document() {
    let dir = tempfile::tempdir().unwrap();
    // Not the defaults: the settings reach the request.
    let settings = "[settings]\ntab_width = 2\ninsert_spaces = false\n";
    let files = [("a.cpp", UNFORMATTED), (".workspace", settings)];
    let (mut app, fake, root) = lsp_project(dir.path(), &files, true).await;
    let a = root.join("a.cpp");
    let end_line = UNFORMATTED.find("// ").unwrap();
    set_cursors(&mut app, &[5, 13, end_line + 3], 1);

    // Unscripted: the fake answers MethodNotFound.
    format_document(&mut app);
    assert_eq!(app.pending_requests(), [("format", 13)]);
    assert!(
        app.settle(|a| message(a).is_some_and(|m| m.starts_with("Format Document failed: ")))
            .await,
        "{:?}",
        message(&app)
    );
    let params = fake.requests("textDocument/formatting").pop().unwrap();
    assert_eq!(params["textDocument"]["uri"], json!(uri(&a)));
    assert_eq!(params["options"], json!({ "tabSize": 2, "insertSpaces": false }));
    assert!(params.get("position").is_none());

    // null: nothing.
    fake.respond("textDocument/formatting", Value::Null);
    format_document(&mut app);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await);
    idle(&mut app).await;
    assert_eq!(text(&app), UNFORMATTED);
    assert_eq!(message(&app), None);

    // The whole document replaced (UTF-16: the 😀 is two units).
    let whole = json!([{
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 4, "character": 0 } },
        "newText": FORMATTED,
    }]);
    fake.respond("textDocument/formatting", whole.clone());
    format_document(&mut app);
    assert!(app.settle(|a| text(a) == FORMATTED).await);
    assert_eq!(cursor_offsets(&app), [5, 13, end_line + 3 + 4]);
    in_sync(&mut app, &fake, &a, "formatted").await;
    assert_eq!(full_text_changes(&fake), 0, "never the full text");
    let last = did_changes(&fake).pop().unwrap();
    assert_eq!(
        last["params"]["contentChanges"],
        json!([{
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } },
            "text": "    ",
        }]),
        "only what changed"
    );
    // One undo step back, cursors too.
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), UNFORMATTED);
    assert_eq!(cursor_offsets(&app), [5, 13, end_line + 3]);
    app.press(K::Char('z'), cmd() | M::SHIFT);
    assert_eq!(text(&app), FORMATTED);
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), UNFORMATTED);

    // Several edits: still one undo step.
    fake.respond(
        "textDocument/formatting",
        json!([
            {
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 2, "character": 0 } },
                "newText": "    return 1;\n",
            },
            {
                "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 2 } },
                "newText": "///",
            },
        ]),
    );
    format_document(&mut app);
    let two = FORMATTED.replace("// ", "/// ");
    assert!(app.settle(|a| text(a) == two).await, "{:?}", text(&app));
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), UNFORMATTED, "one undo step");

    // Stale: typing before the answer cancels it; the late answer changes nothing.
    fake.hold("textDocument/formatting", true);
    put_cursor(&mut app, 0);
    format_document(&mut app);
    assert!(
        app.settle(|_| fake.requests("textDocument/formatting").len() == 5)
            .await
    );
    app.type_text("x");
    assert!(app.pending_requests().is_empty(), "the edit cancelled it");
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert_eq!(text(&app), format!("x{UNFORMATTED}"));
    // Moving the cursor cancels too.
    format_document(&mut app);
    app.press(K::Right, M::NONE);
    assert!(app.pending_requests().is_empty(), "moving cancelled it");
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    idle(&mut app).await;
    assert_eq!(text(&app), format!("x{UNFORMATTED}"));
    // Read-only by the time the answer comes: refused.
    let sent = fake.requests("textDocument/formatting").len();
    format_document(&mut app);
    assert!(
        app.settle(|_| fake.requests("textDocument/formatting").len() == sent + 1)
            .await
    );
    app.active_mut().unwrap().0.read_only = Some("answer".into());
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/flush", Value::Null);
    assert!(app.settle(|a| a.pending_requests().is_empty()).await);
    assert_eq!(message(&app).as_deref(), Some("Read-only: answer"));
    assert_eq!(text(&app), format!("x{UNFORMATTED}"));
    app.active_mut().unwrap().0.read_only = None;
    fake.hold("textDocument/formatting", false);

    // Read-only: nothing sent.
    let sent = fake.requests("textDocument/formatting").len();
    app.active_mut().unwrap().0.read_only = Some("test".into());
    format_document(&mut app);
    assert_eq!(message(&app).as_deref(), Some("Read-only: test"));
    idle(&mut app).await;
    assert_eq!(fake.requests("textDocument/formatting").len(), sent);
    app.active_mut().unwrap().0.read_only = None;

    // No server: said at once.
    app.lsp
        .server_for(Language::Cpp)
        .unwrap()
        .notify("test/crash", Value::Null);
    assert!(
        app.settle(|a| matches!(a.lsp.server_for(Language::Cpp).unwrap().state, State::Failed))
            .await
    );
    format_document(&mut app);
    assert_eq!(
        message(&app).as_deref(),
        Some("There is no formatter for 'cpp' files installed.")
    );
    assert!(fake.errors().is_empty());
}

/// A server without `documentFormattingProvider` gets no request; the palette lists the command.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_format_document_without_provider() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(dir.path(), &[("a.cpp", UNFORMATTED)]);
    let fake = FakeLauncher::new(FakeOptions {
        no_formatting: true,
        ..Default::default()
    });
    let mut app = open_app(&root, fake.clone());
    open_synced(&mut app, &fake, &root.join("a.cpp")).await;
    format_document(&mut app);
    assert_eq!(
        message(&app).as_deref(),
        Some("There is no formatter for 'cpp' files installed.")
    );
    idle(&mut app).await;
    assert!(fake.requests("textDocument/formatting").is_empty());
    assert_eq!(text(&app), UNFORMATTED);
    assert!(crate::command::Command::all().contains(&crate::command::Command::FormatDocument));
    assert_eq!(
        crate::command::Command::from_name("editor.formatDocument"),
        Some(crate::command::Command::FormatDocument)
    );
}

/// `ProcessLauncher` that also keeps every byte jed writes to the server (to check what went
/// out: `didChange`s with ranges only).
struct Tee {
    sent: Arc<std::sync::Mutex<Vec<u8>>>,
}

struct TeeWriter {
    inner: std::pin::Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
    sent: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl tokio::io::AsyncWrite for TeeWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let r = self.inner.as_mut().poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &r {
            self.sent.lock().unwrap().extend_from_slice(&buf[..*n]);
        }
        r
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner.as_mut().poll_shutdown(cx)
    }
}

impl Launcher for Tee {
    fn launch(&self, config: &ServerConfig, root: &Path) -> std::io::Result<crate::lsp::Transport> {
        let mut t = ProcessLauncher.launch(config, root)?;
        t.writer = Box::pin(TeeWriter {
            inner: t.writer,
            sent: self.sent.clone(),
        });
        Ok(t)
    }
}

impl Tee {
    /// The messages jed sent so far.
    fn messages(&self) -> Vec<Value> {
        let mut framer = crate::lsp::rpc::Framer::default();
        framer.push(&self.sent.lock().unwrap());
        let mut out = Vec::new();
        while let Some(Ok(body)) = framer.next() {
            out.push(serde_json::from_slice(&body).unwrap());
        }
        out
    }

    fn method(&self, method: &str) -> Vec<Value> {
        self.messages().into_iter().filter(|m| m["method"] == method).collect()
    }
}

/// Every `didChange` sent carried ranges (never the full text); returns how many were sent.
fn incremental_only(tee: &Tee, process: &str) -> usize {
    let changes = tee.method("textDocument/didChange");
    for m in &changes {
        for c in m["params"]["contentChanges"].as_array().unwrap() {
            assert!(c.get("range").is_some(), "{process}: a full-text change: {m}");
        }
    }
    changes.len()
}

/// Cmd+Z until the active buffer is `text` again (at most `steps` times).
fn undo_to(app: &mut App, text_: &str, steps: usize) {
    for _ in 0..steps {
        if text(app) == text_ {
            return;
        }
        app.press(K::Char('z'), cmd());
    }
    assert_eq!(text(app), text_, "undone");
}

fn errors_in(app: &App, path: &Path) -> Vec<String> {
    marks(app, path)
        .into_iter()
        .filter(|m| m.1 == Severity::Error)
        .map(|m| m.0)
        .collect()
}

/// What the milestone's "done when" runs on one project with a real server.
struct DoneWhen<'a> {
    process: &'a str,
    files: &'a [(&'a str, &'a str)],
    /// Typed in front of `error_before` makes an error on `error_word`.
    error_before: &'a str,
    error_text: &'a str,
    error_word: &'a str,
    /// A `.` typed at the end of the (last) `complete_after` lists `member`.
    complete_after: &'a str,
    member: &'a str,
    /// The last `symbol` in the first file: its hover shows `hover`, its definition is in one of
    /// `def_files`, its references span `ref_files` files; renamed to `new_name`.
    symbol: &'a str,
    hover: &'a str,
    def_files: &'a [&'a str],
    ref_files: usize,
    new_name: &'a str,
    /// The first file after format document (once renamed); `None`: the server can't format.
    formatted: Option<&'a str>,
}

/// The milestone's "done when" on one project: diagnostics follow edits (marks mapped at once,
/// then re-published; cleared by undo), completion, hover, go to definition, find references,
/// rename across files and format document all work, and no `didChange` ever carried the full
/// text.
async fn done_when(app: &mut App, root: &Path, tee: &Tee, d: &DoneWhen<'_>) {
    let process = d.process;
    let (first, original) = d.files[0];
    let path = root.join(first);
    open(app, &path);
    assert!(
        settle_for(app, 30, |a| a
            .lsp
            .server_for(a.active_buffer().unwrap().syntax.language())
            .is_some_and(|s| s.encoding().is_some()))
        .await,
        "{process} initialized"
    );

    // Diagnostics: an error typed in, followed through an edit before it, gone after undo.
    put_cursor(app, original.find(d.error_before).unwrap());
    app.type_text(d.error_text);
    assert!(
        settle_for(app, 60, |a| errors_in(a, &path).iter().any(|w| w == d.error_word)).await,
        "{process} reports the error: {:?}",
        marks(app, &path)
    );
    assert_eq!(errors_in(app, &path), [d.error_word], "{process}: only the typed error");
    put_cursor(app, 0);
    app.press(K::Enter, M::NONE);
    assert_eq!(
        errors_in(app, &path),
        [d.error_word],
        "{process}: mapped through the edit"
    );
    assert!(
        settle_for(app, 60, |a| {
            let errors = errors_in(a, &path);
            let doc = a.lsp.version(buffer_id(a, &path));
            errors == [d.error_word] && doc.is_some_and(|v| v >= 3)
        })
        .await
    );
    undo_to(app, original, 4);
    assert!(
        settle_for(app, 60, |a| errors_in(a, &path).is_empty()).await,
        "{process} clears it after undo: {:?}",
        marks(app, &path)
    );

    // Completion: `.` after the object lists the member; Enter inserts it.
    let at = original.rfind(d.complete_after).unwrap() + d.complete_after.len();
    let listed = |a: &App| shown(a).iter().any(|l| l.trim() == d.member);
    let t = Instant::now();
    while !listed(app) && t.elapsed() < Duration::from_secs(60) {
        app.press(K::Esc, M::NONE);
        undo_to(app, original, 4);
        put_cursor(app, at);
        app.type_text(".");
        settle_for(app, 5, listed).await;
    }
    assert!(listed(app), "{process} completion: {:?}", shown(app));
    let p = app.completion.popup.as_mut().unwrap();
    p.selected = p
        .shown
        .iter()
        .position(|&i| p.items[i].label.trim() == d.member)
        .unwrap();
    app.press(K::Enter, M::NONE);
    let want = format!("{}.{}", d.complete_after.trim(), d.member);
    assert!(text(app).contains(&want), "{process}: {}", text(app));
    undo_to(app, original, 4);

    // Hover.
    let at = original.rfind(d.symbol).unwrap() + 1;
    put_cursor(app, at);
    let mut term = terminal();
    draw(&mut term, app);
    let t = Instant::now();
    while app.hover.popup.is_none() && t.elapsed() < Duration::from_secs(60) {
        show_hover(app);
        settle_for(app, 3, |a| a.hover.popup.is_some()).await;
    }
    draw(&mut term, app);
    let rows = popup_rows(&term, app).join("\n");
    assert!(rows.contains(d.hover), "{process} hover: {rows}");

    // Go to definition, then back.
    app.press(K::F(12), M::NONE);
    let defs: Vec<PathBuf> = d.def_files.iter().map(|f| root.join(f)).collect();
    assert!(
        settle_for(app, 60, |a| active_path(a).is_some_and(|p| defs.contains(&p))).await,
        "{process} went to the definition; message {:?}",
        message(app)
    );
    while active_path(app).as_deref() != Some(path.as_path()) {
        app.press(K::PageUp, M::CONTROL);
    }

    // Find references (the server may need to index the other files first).
    put_cursor(app, at);
    let all = |a: &App| a.locations.as_ref().is_some_and(|l| l.files.len() == d.ref_files);
    let t = Instant::now();
    while !all(app) && t.elapsed() < Duration::from_secs(60) {
        app.press(K::F(12), M::SHIFT);
        settle_for(app, 3, all).await;
        app.press(K::Char('1'), cmd());
        put_cursor(app, at);
    }
    assert!(
        all(app),
        "{process} references: {:?}; message {:?}",
        app.locations.as_ref().map(|l| &l.files),
        message(app)
    );

    // Rename across the files.
    app.press(K::F(2), M::NONE);
    assert!(
        settle_for(app, 30, |a| a.prompt.is_some()).await,
        "{process} rename prompt; message {:?}",
        message(app)
    );
    app.type_text(d.new_name);
    app.press(K::Enter, M::NONE);
    let renamed = |a: &App| {
        d.files.iter().filter(|(_, t)| t.contains(d.symbol)).all(|(name, _)| {
            a.buffers.find_path(&root.join(name)).is_some_and(|id| {
                let t = a.buffers.get(id).unwrap().rope.to_string();
                t.contains(d.new_name) && !t.contains(d.symbol)
            })
        })
    };
    assert!(
        settle_for(app, 30, renamed).await,
        "{process} renamed; message {:?}; texts {:?}",
        message(app),
        app.buffers.iter().map(|(_, b)| b.rope.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(active_path(app).as_deref(), Some(path.as_path()));
    let renamed_text = text(app);

    // Format document: one undo step.
    format_document(app);
    match d.formatted {
        Some(formatted) => {
            let want = formatted.replace(d.symbol, d.new_name);
            assert!(
                settle_for(app, 30, |a| text(a) == want).await,
                "{process} formatted: {:?}; message {:?}",
                text(app),
                message(app)
            );
            app.press(K::Char('z'), cmd());
            assert_eq!(text(app), renamed_text, "{process}: one undo step");
        }
        None => {
            let language = app.active_buffer().unwrap().syntax.language().id();
            let no = format!("There is no formatter for '{language}' files installed.");
            assert_eq!(message(app), Some(no));
        }
    }

    let n = incremental_only(tee, process);
    assert!(n >= 5, "{process}: {n} didChanges");
    app.press(K::Char('q'), cmd());
    app.shutdown_servers().await;
    if let Some(n) = children_named(process) {
        assert_eq!(n, 0, "no {process} left");
    }
}

const CPP_MAIN: &str = "#include \"util.h\"\nint   unused_fmt( ){return 0;}\nint main() {\n  Point pt{1, 2};\n  pt;\n  int t = total(pt);\n  return t;\n}\n";
const CPP_MAIN_FORMATTED: &str = "#include \"util.h\"\nint unused_fmt() { return 0; }\nint main() {\n  Point pt{1, 2};\n  pt;\n  int t = total(pt);\n  return t;\n}\n";
const CPP_UTIL_H: &str =
    "#pragma once\nstruct Point {\n  int xcoord;\n  int ycoord;\n};\n// Adds the coordinates.\nint total(Point p);\n";
const CPP_UTIL_CPP: &str = "#include \"util.h\"\nint total(Point p) { return p.xcoord + p.ycoord; }\n";

/// M9 done when, C++: a project with `compile_commands.json` and real clangd.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_done_when_cpp_project() {
    if which("clangd").is_none() {
        eprintln!("milestone_9_done_when_cpp_project: skipped, clangd isn't on PATH");
        return;
    }
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let files = [
        ("main.cpp", CPP_MAIN),
        ("util.h", CPP_UTIL_H),
        ("util.cpp", CPP_UTIL_CPP),
    ];
    let root = project(dir.path(), &files);
    let commands: Vec<Value> = ["main.cpp", "util.cpp"]
        .iter()
        .map(|f| json!({ "directory": root, "file": f, "arguments": ["clang++", "-std=c++17", "-c", f] }))
        .collect();
    fs::write(root.join("compile_commands.json"), Value::Array(commands).to_string()).unwrap();
    let tee = Arc::new(Tee {
        sent: Default::default(),
    });
    let mut app = open_app(&root, tee.clone());
    let d = DoneWhen {
        process: "clangd",
        files: &files,
        error_before: "t;\n}",
        error_text: "zz + ",
        error_word: "zz",
        complete_after: "  pt",
        member: "ycoord",
        symbol: "total",
        hover: "function total",
        def_files: &["util.h", "util.cpp"],
        ref_files: 3,
        new_name: "sum_of",
        formatted: Some(CPP_MAIN_FORMATTED),
    };
    done_when(&mut app, &root, &tee, &d).await;
}

const SCONSTRUCT: &str = "import helpers

env = Environment(CXXFLAGS=[\"-O2\"])
env.Append(CPPPATH=[\"include\"])
sources = helpers.cpp_files(Glob(\"src/*.cpp\"))
program = env.Program(\"app\", sources)
Default(program)
Export(\"env\")
SConscript(\"lib/SConscript\")
if \"fast\" in COMMAND_LINE_TARGETS or ARGUMENTS.get(\"mode\") == \"fast\":
    env.Append(CXXFLAGS=[\"-O3\"])
env
";
const SCONS_HELPERS: &str = "def cpp_files(nodes: list) -> list:
    \"\"\"The C++ sources among `nodes`.\"\"\"
    return [n for n in nodes if str(n).endswith(\".cpp\")]
";

/// M9 done when, SCons: an extensionless `SConstruct` (Python) with real ty, SCons's globals
/// known through jed's `__builtins__.pyi` (written to the support dir, not the workspace, and put
/// on ty's search path). ty has no formatter: format document says so.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_9_done_when_scons_project() {
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join(".venv/bin/ty");
    let Some(ty) = which("ty").or_else(|| venv.is_file().then_some(venv)) else {
        eprintln!("milestone_9_done_when_scons_project: skipped, ty isn't on PATH or in .venv/bin");
        return;
    };
    let _one_at_a_time = REAL_SERVERS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let files = [("SConstruct", SCONSTRUCT), ("helpers.py", SCONS_HELPERS)];
    let root = project(dir.path(), &files);
    let support = dir.path().join("support");
    let tee = Arc::new(Tee {
        sent: Default::default(),
    });
    let mut app = open_app(&root, tee.clone());
    app.lsp.builtins.ty = ServerConfig::new(ty.to_str().unwrap(), &["server"]);
    app.lsp.support_dir = Some(support.clone());
    let d = DoneWhen {
        process: "ty",
        files: &files,
        error_before: "Default(program)",
        error_text: "zz + ",
        error_word: "zz",
        complete_after: "env",
        member: "Program",
        symbol: "cpp_files",
        hover: "def cpp_files",
        def_files: &["helpers.py"],
        ref_files: 2,
        new_name: "sources_of",
        formatted: None,
    };
    done_when(&mut app, &root, &tee, &d).await;
    // The stub: in the support dir, on ty's search path; nothing written in the workspace.
    let stub = support.join("scons/__builtins__.pyi");
    assert_eq!(fs::read_to_string(&stub).unwrap(), crate::lsp::scons::BUILTINS);
    let init = tee.method("initialize").pop().unwrap();
    assert_eq!(
        init["params"]["initializationOptions"]["configuration"]["environment"]["extra-paths"],
        json!([support.join("scons")])
    );
    let mut names: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["SConstruct", "helpers.py"]);
    let open = tee.method("textDocument/didOpen");
    assert!(open.iter().any(|m| {
        m["params"]["textDocument"]["languageId"] == "python"
            && m["params"]["textDocument"]["uri"]
                .as_str()
                .unwrap()
                .ends_with("/SConstruct")
    }));
}
