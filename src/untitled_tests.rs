//! The Untitled files and Save As task (`untitled`), subtask untitled-a, end to end: Cmd+N opens
//! `Untitled-N` (the lowest free number) as an empty, writable Plain Text tab; its name shows in
//! the tab bar and the status bar; typing works; "Change Language Mode" picks a language that
//! survives re-detection (and overrides a file's); closing an emptied untitled buffer doesn't
//! ask, a closing or quitting one with text does; path-keyed features (language servers)
//! skip it.
//!
//! Subtask untitled-c: Save on an untitled buffer and Save As ask for a path (errors stay in the
//! prompt, an existing file asks to overwrite, Esc cancels a close / quit / Save All too); the
//! tab becomes the file (path, language unless picked, titles in every split, disk stamp, clean)
//! and a Save As leaves the old file alone; Save All saves files and prompts per untitled buffer;
//! a close's or quit's Save goes through the prompt and then closes / quits.
//!
//! Subtask untitled-d: a buffer that changes path follows it: the language server gets
//! didClose (old) / didOpen (new) before any didSave, an ex-untitled buffer just didOpen; gutter
//! markers, blame, Source Control, file history and the Explorer follow the new path; the old
//! file and its history stay.
//!
//! Subtask untitled-e: crash backups cover untitled buffers (`untitled-<id>.bak`, written by the
//! timer and the panic hook, removed when emptied, saved or closed); a killed jed's next start
//! offers the text back and Restore opens an untitled buffer; the terminal going away keeps
//! non-empty untitled tabs in the session, restored from their backup with no prompt; a clean
//! quit leaves neither. Together with the other subtasks that is the task's done-when: Cmd+N,
//! typing and Cmd+S write the file and the tab becomes it (language, server, titles in every
//! split); Save As leaves the old file; Save All; quitting asks first; kill and restart.
//!
//! Subtask untitled-b: every path-keyed feature skips an untitled buffer: disk checks and
//! file system events, file history, git (quick diff, stage, compare) and blame.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, InitialFile, InputKind, Prompt};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::syntax::Language;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// An app with `a.c` open in `<dir>/ws`.
fn open_app(
    dir: &tempfile::TempDir,
    launcher: Option<std::sync::Arc<crate::lsp::fake::FakeLauncher>>,
) -> (App, PathBuf) {
    let base = workspace::normalize(dir.path());
    let root = base.join("ws");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.c"), "int x;\n").unwrap();
    let ws = Workspace::load(root.clone());
    let buffer = Buffer::open_with(&root.join("a.c"), ws.settings().indentation()).unwrap();
    let trust = TrustStore::load(base.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    if let Some(launcher) = launcher {
        app.lsp.set_launcher(launcher);
    }
    app.test_start();
    app.test_plain_whitespace();
    (app, root)
}

fn new_untitled(app: &mut App) {
    app.press(KeyCode::Char('n'), cmd());
}

fn draw(app: &mut App) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

fn run_command(app: &mut App, title: &str) {
    app.press(KeyCode::Char('p'), cmd() | M::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(KeyCode::Enter, M::NONE);
}

fn pick_language(app: &mut App, query: &str) {
    run_command(app, "Change Language Mode");
    app.type_text(query);
    app.press(KeyCode::Enter, M::NONE);
}

fn names(app: &App) -> Vec<String> {
    app.group()
        .tabs
        .iter()
        .filter_map(|t| t.buffer())
        .filter_map(|id| app.buffers.get(id))
        .map(|b| b.name())
        .collect()
}

fn active_language(app: &App) -> Language {
    app.active_buffer().unwrap().syntax.language()
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_cmd_n_opens_numbered_plain_text_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, None);
    new_untitled(&mut app);
    new_untitled(&mut app);
    assert_eq!(names(&app), ["a.c", "Untitled-1", "Untitled-2"]);
    let buf = app.active_buffer().unwrap();
    assert!(buf.is_untitled() && !buf.has_file() && buf.read_only.is_none() && !buf.is_dirty());
    assert_eq!(active_language(&app), Language::PlainText);
    let rows = draw(&mut app);
    assert!(
        rows[0].contains("Untitled-1") && rows[0].contains("Untitled-2"),
        "{:?}",
        rows[0]
    );
    assert!(rows.last().unwrap().contains("Plain Text"), "{:?}", rows.last());

    // Close Untitled-1 (empty: no prompt); the next one takes its number back.
    app.test_activate_tab(1);
    app.test_execute(Command::CloseTab);
    assert!(app.prompt.is_none());
    assert_eq!(names(&app), ["a.c", "Untitled-2"]);
    new_untitled(&mut app);
    assert_eq!(names(&app), ["a.c", "Untitled-2", "Untitled-1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_typing_makes_it_dirty_and_closing_asks_unless_emptied() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("hello");
    assert!(app.active_buffer().unwrap().is_dirty());
    app.test_execute(Command::CloseTab);
    assert!(matches!(app.prompt, Some(Prompt::SaveBeforeClose { .. })));
    assert!(draw(&mut app).iter().any(|r| r.contains("Save changes to Untitled-1?")));
    app.press(KeyCode::Char('c'), M::NONE);
    assert!(app.prompt.is_none());
    assert_eq!(names(&app), ["a.c", "Untitled-1"]);

    // Typed then deleted again: nothing to lose.
    for _ in 0..5 {
        app.press(KeyCode::Backspace, M::NONE);
    }
    assert_eq!(app.active_buffer().unwrap().rope.len(), 0);
    app.test_execute(Command::CloseTab);
    assert!(app.prompt.is_none());
    assert_eq!(names(&app), ["a.c"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_quit_asks_only_for_a_buffer_with_text() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, None);
    new_untitled(&mut app);
    app.test_execute(Command::Quit);
    assert!(app.quitting(), "an empty untitled buffer doesn't hold up quitting");

    let (mut app, _) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("x");
    app.test_execute(Command::Quit);
    assert!(!app.quitting());
    assert!(matches!(&app.prompt, Some(Prompt::SaveAllBeforeQuit { buffers }) if buffers.len() == 1));
    assert!(
        draw(&mut app)
            .iter()
            .any(|r| r.contains("Save changes to Untitled-1 before quitting?"))
    );
    app.press(KeyCode::Char('c'), M::NONE);
    assert!(!app.quitting() && app.prompt.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_change_language_mode_survives_redetection_and_overrides_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    fs::write(root.join("b.c"), "int b;\n").unwrap();
    new_untitled(&mut app);
    run_command(&mut app, "Change Language Mode");
    {
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows.len(), Language::all().len());
        assert!(
            p.rows[p.selected].detail.ends_with("current"),
            "starts on the current mode"
        );
        assert_eq!(p.rows[p.selected].label, "Plain Text");
    }
    app.type_text("rust");
    assert_eq!(app.palette.as_ref().unwrap().rows[0].label, "Rust");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(active_language(&app), Language::Rust);
    assert!(draw(&mut app).last().unwrap().contains("Rust"));
    // Opened again, it starts on the current language (not the first row): Enter keeps it.
    run_command(&mut app, "Change Language Mode");
    {
        let p = app.palette.as_ref().unwrap();
        assert_eq!(p.rows[p.selected].label, "Rust", "starts on the current language");
        assert_ne!(p.selected, 0);
    }
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(active_language(&app), Language::Rust);

    // On a file it overrides detection for that buffer only.
    app.test_activate_tab(0);
    assert_eq!(active_language(&app), Language::C);
    pick_language(&mut app, "python");
    assert_eq!(active_language(&app), Language::Python);
    app.test_open(Buffer::open(&root.join("b.c")).unwrap());
    let b = app.buffers.find_path(&root.join("b.c")).unwrap();
    assert_eq!(app.buffers.get(b).unwrap().syntax.language(), Language::C);

    // A live `.workspace` association re-detects every buffer: b.c follows it, the picked
    // languages stay.
    fs::write(
        root.join(".workspace"),
        "[files]\nassociations = { \"*.c\" = \"cpp\" }\n",
    )
    .unwrap();
    let redetected = app
        .settle(|a| a.buffers.get(b).unwrap().syntax.language() == Language::Cpp)
        .await;
    assert!(redetected, "b.c re-detected from the association");
    assert_eq!(names(&app), ["a.c", "b.c", "Untitled-1"]);
    let lang_of = |app: &App, i: usize| {
        let id = app.group().tabs[i].buffer().unwrap();
        app.buffers.get(id).unwrap().syntax.language()
    };
    assert_eq!(lang_of(&app, 0), Language::Python, "a.c keeps its pick");
    assert_eq!(lang_of(&app, 2), Language::Rust, "the untitled buffer keeps its pick");

    // Esc picks nothing.
    app.test_activate_tab(2);
    run_command(&mut app, "Change Language Mode");
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(active_language(&app), Language::Rust);

    // On a diff tab (no buffer of its own) it does nothing.
    app.test_activate_tab(0);
    app.type_text("x");
    app.test_execute(Command::CompareWithSaved);
    assert!(app.active_buffer().is_none(), "a diff tab is active");
    app.test_execute(Command::ChangeLanguageMode);
    assert!(app.palette.is_none());
}

/// The Save As prompt's text, if that prompt is open.
fn save_as_text(app: &App) -> Option<String> {
    match &app.prompt {
        Some(Prompt::Input(i)) if matches!(i.kind, InputKind::SaveAs { .. }) => Some(i.field.text.clone()),
        _ => None,
    }
}

fn save_as_error(app: &App) -> Option<&'static str> {
    match &app.prompt {
        Some(Prompt::Input(i)) => i.error,
        _ => None,
    }
}

async fn wait_saved(app: &mut App) {
    assert!(app.settle(|a| !a.buffers.any_saving()).await, "the save ends");
    app.pump();
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_asks_for_a_path_and_the_tab_becomes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    fs::create_dir(root.join("sub")).unwrap();
    new_untitled(&mut app);
    app.test_execute(Command::SplitRight);
    app.type_text("fn main() {}\n");
    assert!(!keyword_drawn(&mut app), "plain text while untitled");
    app.press(KeyCode::Char('s'), cmd());
    assert_eq!(save_as_text(&app).as_deref(), Some("Untitled-1"));
    assert!(draw(&mut app).iter().any(|r| r.contains("Save Untitled-1 as")));
    // Errors stay in the prompt.
    app.type_text("   ");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(save_as_error(&app), Some("Enter a file name"));
    app.type_text("d/");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(save_as_error(&app), Some("That is a folder: add a file name"));
    app.press(KeyCode::Char('a'), cmd());
    app.type_text("a.c");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(save_as_error(&app), Some("That file is open in another tab"));
    app.press(KeyCode::Char('a'), cmd());
    app.type_text("sub/.././a.c");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(
        save_as_error(&app),
        Some("That file is open in another tab"),
        "`.` and `..` are resolved"
    );
    app.press(KeyCode::Char('a'), cmd());
    app.type_text("sub/x.rs");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;

    assert!(app.prompt.is_none());
    assert_eq!(fs::read_to_string(root.join("sub/x.rs")).unwrap(), "fn main() {}\n");
    let buf = app.active_buffer().unwrap();
    assert!(buf.has_file() && !buf.is_untitled() && !buf.is_dirty());
    assert_eq!(buf.path, root.join("sub/x.rs"));
    assert!(buf.disk().is_some(), "the disk stamp is set");
    assert_eq!(active_language(&app), Language::Rust);
    let rows = draw(&mut app);
    assert!(!rows[0].contains("Untitled-1"), "{:?}", rows[0]);
    assert_eq!(
        rows[0].matches("x.rs").count(),
        2,
        "both splits show the new title: {:?}",
        rows[0]
    );
    assert!(rows.last().unwrap().contains("Rust"), "{:?}", rows.last());
    // Its highlighting: once parsed, `fn` is drawn in the keyword colour.
    assert!(
        app.settle(|a| a.active_buffer().unwrap().syntax.tree().is_some()).await,
        "the new file is parsed"
    );
    assert!(keyword_drawn(&mut app), "`fn` drawn as a keyword");
}

/// Whether the drawn screen shows `fn` in the theme's keyword colour.
fn keyword_drawn(app: &mut App) -> bool {
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let keyword = app.theme.get("keyword").fg.expect("a keyword colour");
    let buf = term.backend().buffer();
    (0..buf.area.height).any(|y| {
        (0..buf.area.width - 1).any(|x| {
            let (f, n) = (&buf[(x, y)], &buf[(x + 1, y)]);
            f.symbol() == "f" && n.symbol() == "n" && f.fg == keyword && n.fg == keyword
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_errors_keep_the_prompt_and_esc_cancels() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    fs::create_dir(root.join("sub")).unwrap();
    new_untitled(&mut app);
    app.type_text("keep me");
    app.press(KeyCode::Char('s'), cmd());
    for (bad, why) in [("sub", "a folder"), ("nope/y.txt", "a missing folder")] {
        app.type_text(bad);
        app.press(KeyCode::Enter, M::NONE);
        assert!(app.settle(|a| !a.buffers.any_saving()).await, "{why}");
        app.pump();
        assert_eq!(
            save_as_error(&app),
            Some("Can't save there (see the message below)"),
            "{why}"
        );
        assert_eq!(save_as_text(&app).as_deref(), Some(bad), "{why}");
        assert!(app.active_buffer().unwrap().is_untitled(), "{why}");
    }
    assert!(!root.join("nope").exists());
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.prompt.is_none());
    let buf = app.active_buffer().unwrap();
    assert!(buf.is_untitled() && buf.is_dirty() && buf.rope == "keep me");
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_leaves_the_old_file_and_moves_the_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    app.type_text("z");
    app.press(KeyCode::Char('s'), cmd() | M::SHIFT);
    assert_eq!(save_as_text(&app).as_deref(), Some("a.c"));
    app.type_text("b.c");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    assert_eq!(
        fs::read_to_string(root.join("a.c")).unwrap(),
        "int x;\n",
        "a.c is untouched"
    );
    assert_eq!(fs::read_to_string(root.join("b.c")).unwrap(), "zint x;\n");
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.path, root.join("b.c"));
    assert!(!buf.is_dirty());
    assert_eq!(names(&app), ["b.c"]);

    // The old file is no longer the buffer's: changing it leaves the buffer alone.
    app.fs_flushed.clear();
    fs::write(root.join("a.c"), "changed\n").unwrap();
    assert!(
        app.settle(|a| a.fs_flushed.iter().any(|p| p.ends_with("a.c"))).await,
        "the watcher saw a.c change"
    );
    app.pump();
    let buf = app.active_buffer().unwrap();
    assert!(buf.rope == "zint x;\n" && !buf.is_dirty() && buf.disk_conflict.is_none());
    // The new one is: a change to it reloads the clean buffer.
    fs::write(root.join("b.c"), "theirs\n").unwrap();
    assert!(app.settle(|a| a.active_buffer().unwrap().rope == "theirs\n").await);
    assert!(!app.active_buffer().unwrap().is_dirty());
}

/// An edit typed while the Save As write runs isn't in the file and leaves the buffer dirty;
/// the disk stamp is the written file's, so the next Save goes through without a conflict.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_keeps_edits_made_during_the_write_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("one");
    app.press(KeyCode::Char('s'), cmd());
    app.type_text("x.txt");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.buffers.any_saving(), "the write runs off the UI thread");
    // `press` doesn't take app events: the SaveDone can't be handled before this edit.
    app.type_text("!");
    wait_saved(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("x.txt")).unwrap(), "one");
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.path, root.join("x.txt"));
    assert!(buf.rope == "one!" && buf.is_dirty() && !buf.is_untitled());

    app.press(KeyCode::Char('s'), cmd());
    assert!(save_as_text(&app).is_none(), "a plain save now");
    wait_saved(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("x.txt")).unwrap(), "one!");
    let buf = app.active_buffer().unwrap();
    assert!(!buf.is_dirty() && buf.disk_conflict.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_onto_an_existing_file_asks_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    fs::write(root.join("b.c"), "other\n").unwrap();
    new_untitled(&mut app);
    app.type_text("mine");
    app.press(KeyCode::Char('s'), cmd());
    app.type_text("b.c");
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| matches!(a.prompt, Some(Prompt::ConfirmOverwrite { .. })))
            .await
    );
    assert!(draw(&mut app).iter().any(|r| r.contains("already exists")));
    assert_eq!(fs::read_to_string(root.join("b.c")).unwrap(), "other\n");
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.prompt.is_none());
    assert_eq!(fs::read_to_string(root.join("b.c")).unwrap(), "other\n");
    assert!(app.active_buffer().unwrap().is_untitled());

    app.press(KeyCode::Char('s'), cmd());
    app.type_text("b.c");
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| matches!(a.prompt, Some(Prompt::ConfirmOverwrite { .. })))
            .await
    );
    app.press(KeyCode::Char('o'), M::NONE);
    wait_saved(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("b.c")).unwrap(), "mine");
    assert_eq!(app.active_buffer().unwrap().path, root.join("b.c"));
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_redetects_the_language_unless_one_was_picked() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("x");
    app.press(KeyCode::Char('s'), cmd());
    app.type_text("one.rs");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    assert_eq!(active_language(&app), Language::Rust);

    new_untitled(&mut app);
    app.type_text("x");
    pick_language(&mut app, "Python");
    let picked = active_language(&app);
    assert_ne!(picked, Language::PlainText);
    app.press(KeyCode::Char('s'), cmd());
    app.type_text("two.rs");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    assert!(root.join("two.rs").is_file());
    assert_eq!(active_language(&app), picked, "the picked language wins");
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_all_saves_files_and_prompts_for_each_untitled_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    app.type_text("y");
    new_untitled(&mut app);
    app.type_text("one");
    new_untitled(&mut app);
    app.type_text("two");
    app.press(KeyCode::Char('s'), cmd() | M::ALT);
    assert!(app.settle(|a| !a.buffers.any_saving()).await);
    assert_eq!(fs::read_to_string(root.join("a.c")).unwrap(), "yint x;\n");
    assert_eq!(save_as_text(&app).as_deref(), Some("Untitled-1"));
    app.type_text("1.txt");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| save_as_text(a).as_deref() == Some("Untitled-2")).await);
    app.press(KeyCode::Esc, M::NONE);
    wait_saved(&mut app).await;
    assert!(app.prompt.is_none());
    assert_eq!(fs::read_to_string(root.join("1.txt")).unwrap(), "one");
    assert_eq!(names(&app), ["a.c", "1.txt", "Untitled-2"]);
    assert!(
        app.active_buffer().unwrap().is_untitled(),
        "the cancelled one is still untitled"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_quit_save_asks_for_a_path_then_quits() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("bye");
    app.test_activate_tab(0);
    app.type_text("y");
    app.test_execute(Command::Quit);
    assert!(matches!(&app.prompt, Some(Prompt::SaveAllBeforeQuit { buffers }) if buffers.len() == 2));
    app.press(KeyCode::Char('s'), M::NONE);
    assert!(
        app.settle(|a| save_as_text(a).is_some() && !a.buffers.any_saving())
            .await
    );
    app.pump();
    assert!(!app.quitting(), "waits for the path");
    assert_eq!(fs::read_to_string(root.join("a.c")).unwrap(), "yint x;\n");
    app.type_text("bye.txt");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.quitting()).await);
    assert_eq!(fs::read_to_string(root.join("bye.txt")).unwrap(), "bye");
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_quit_save_stops_when_the_path_prompt_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("bye");
    new_untitled(&mut app);
    app.type_text("bye too");
    app.test_execute(Command::Quit);
    app.press(KeyCode::Char('s'), M::NONE);
    assert_eq!(save_as_text(&app).as_deref(), Some("Untitled-1"));
    app.press(KeyCode::Esc, M::NONE);
    app.pump();
    assert!(!app.quitting());
    assert!(app.prompt.is_none());
    // The quit and the rest of its queue are forgotten: a later Save As neither quits nor asks
    // for Untitled-2.
    app.test_activate_tab(1);
    app.press(KeyCode::Char('s'), cmd());
    assert_eq!(save_as_text(&app).as_deref(), Some("Untitled-1"));
    app.type_text("one.txt");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("one.txt")).unwrap(), "bye");
    assert!(!app.quitting(), "the quit was dropped");
    assert!(app.prompt.is_none(), "Untitled-2 left the queue");
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_close_save_asks_for_a_path_then_closes_the_tab() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("closing");
    app.test_execute(Command::CloseTab);
    assert!(matches!(app.prompt, Some(Prompt::SaveBeforeClose { .. })));
    app.press(KeyCode::Char('s'), M::NONE);
    app.type_text("c.txt");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| names(a) == ["a.c"]).await);
    assert_eq!(fs::read_to_string(root.join("c.txt")).unwrap(), "closing");
}

/// The backup files of `root`'s workspace, sorted.
fn backup_files(state: &std::path::Path, root: &std::path::Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(crate::backup::dir(state, root))
        .map(|d| d.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

fn untitled_file(state: &std::path::Path, root: &std::path::Path, app: &App) -> PathBuf {
    let id = app.active_buffer().unwrap().untitled_id().unwrap().to_string();
    crate::backup::key_file(state, root, &crate::backup::Key::Untitled(id))
}

/// A workspace in `<dir>/proj` with `a.txt`, and a state dir.
fn dirs(dir: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let base = workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(root.join("a.txt"), "a\n").unwrap();
    (root, state)
}

/// The app on `root` with sessions and backups in `state`, the session read.
async fn reopen(root: &std::path::Path, state: &std::path::Path) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.state_dir = Some(state.to_path_buf());
    app.test_start();
    app.test_plain_whitespace();
    assert!(app.settle(|a| !a.session.loading()).await, "the session is read");
    app
}

async fn back_up(app: &mut App) {
    app.test_backup_tick(crate::backup::INTERVAL);
    assert!(app.settle(|a| !a.backups.busy()).await, "the backup job ends");
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_gets_a_crash_backup_until_empty_saved_or_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = reopen(&root, &state).await;
    new_untitled(&mut app);
    // Empty: nothing to back up.
    back_up(&mut app).await;
    assert!(backup_files(&state, &root).is_empty());
    app.type_text("unsaved");
    let file = untitled_file(&state, &root, &app);
    back_up(&mut app).await;
    assert_eq!(backup_files(&state, &root), std::slice::from_ref(&file));
    let b = crate::backup::read(&state, &root, &file).unwrap();
    assert_eq!(b.text.to_string(), "unsaved");
    assert_eq!(b.untitled.as_deref(), app.active_buffer().unwrap().untitled_id());

    // Emptied again: its backup goes.
    app.press(KeyCode::Char('a'), cmd());
    app.press(KeyCode::Backspace, M::NONE);
    assert!(app.settle(|_| !file.exists()).await, "an emptied buffer needs none");

    // Save As: the backup (keyed by the id) goes, the file has the text.
    app.type_text("saved text");
    back_up(&mut app).await;
    assert!(file.exists());
    app.press(KeyCode::Char('s'), cmd());
    app.type_text("s.txt");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    assert!(
        app.settle(|_| !file.exists()).await,
        "saved as a file: no untitled backup"
    );
    assert_eq!(fs::read_to_string(root.join("s.txt")).unwrap(), "saved text");
    assert!(app.settle(|a| !a.backups.busy()).await);
    assert!(backup_files(&state, &root).is_empty());

    // Closing a dirty untitled buffer (Don't Save) removes its backup too.
    new_untitled(&mut app);
    app.type_text("temp");
    let file = untitled_file(&state, &root, &app);
    back_up(&mut app).await;
    assert!(file.exists());
    app.test_execute(Command::CloseTab);
    app.press(KeyCode::Char('d'), M::NONE);
    assert!(app.settle(|_| !file.exists()).await, "closed: backup removed");
}

/// Killing jed (no shutdown) after the timer's backup: the next start offers the text back and
/// Restore opens it as an untitled buffer, dirty, numbered afresh; Discard removes it.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_killed_jed_offers_its_text_back_as_an_untitled_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = reopen(&root, &state).await;
    new_untitled(&mut app);
    new_untitled(&mut app);
    app.type_text("second\nline");
    let (id, file) = (
        app.active_buffer().unwrap().untitled_id().unwrap().to_string(),
        untitled_file(&state, &root, &app),
    );
    back_up(&mut app).await;
    app.type_text(" lost");
    drop(app);

    let mut app = reopen(&root, &state).await;
    assert!(app.settle(|a| matches!(a.prompt, Some(Prompt::RestoreBackups))).await);
    assert_eq!(app.backups.offered.len(), 1);
    let rows = draw(&mut app);
    assert!(
        rows.iter().any(|r| r.contains("untitled text \"second\"")),
        "named in the prompt: {rows:?}"
    );
    assert!(names(&app).is_empty(), "no tab before restoring");
    app.press(KeyCode::Char('r'), M::NONE);
    assert!(
        app.settle(|a| a.backups.restoring() == 0 && !a.group().tabs.is_empty())
            .await
    );
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.rope.to_string(), "second\nline");
    assert!(buf.is_untitled() && buf.is_dirty() && !buf.has_file());
    assert_eq!(buf.name(), "Untitled-1");
    assert_eq!(buf.untitled_id(), Some(id.as_str()));
    assert!(file.exists(), "its backup stays until it is saved or closed");
    back_up(&mut app).await;
    assert_eq!(
        backup_files(&state, &root),
        std::slice::from_ref(&file),
        "not duplicated"
    );

    // Discard instead: removed, nothing opens.
    drop(app);
    let mut app = reopen(&root, &state).await;
    assert!(app.settle(|a| matches!(a.prompt, Some(Prompt::RestoreBackups))).await);
    app.press(KeyCode::Char('d'), M::NONE);
    assert!(app.settle(|_| !file.exists()).await);
    assert!(names(&app).is_empty());
}

/// The terminal going away (shutdown without Quit): the session keeps the non-empty untitled tab
/// and its backup is written; the next start restores it from the backup, no prompt, with its
/// cursor; an empty untitled tab is not kept.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_tabs_with_text_come_back_with_the_session_without_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = reopen(&root, &state).await;
    new_untitled(&mut app);
    app.type_text("notes\nmore");
    app.test_execute(Command::SplitRight);
    new_untitled(&mut app);
    app.shutdown().await;
    let session = fs::read_to_string(crate::session::file(&state, &root)).unwrap();
    // The split shows the one buffer in two groups; the empty Untitled-2 is left out.
    assert_eq!(session.matches("\"untitled\"").count(), 2, "{session}");
    assert_eq!(backup_files(&state, &root).len(), 1);

    let mut app = reopen(&root, &state).await;
    assert!(app.settle(|a| !a.backups.busy()).await);
    assert!(app.prompt.is_none(), "restored by the session: not offered");
    assert!(app.backups.offered.is_empty());
    let texts: Vec<String> = app.buffers.iter().map(|(_, b)| b.rope.to_string()).collect();
    assert_eq!(texts, ["notes\nmore"], "one untitled buffer, nothing else");
    assert_eq!(
        app.groups.ids().len(),
        2,
        "the split is back, both tabs on the one buffer"
    );
    let buf = app.active_buffer().unwrap();
    assert!(buf.is_untitled() && buf.is_dirty());
    assert_eq!(buf.name(), "Untitled-1");
    assert_eq!(
        app.active_mut().unwrap().1.selections.primary().head.0,
        "notes\nmore".len(),
        "the cursor is where it was"
    );
    // The restored buffer is backed up by the file already there: still one, and a Save As
    // removes it.
    back_up(&mut app).await;
    let file = untitled_file(&state, &root, &app);
    assert_eq!(backup_files(&state, &root), std::slice::from_ref(&file));
    app.press(KeyCode::Char('s'), cmd());
    app.type_text("n.txt");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    assert!(app.settle(|_| !file.exists()).await);
}

/// The start's session read and crash backup scan find the same untitled backup: whichever is
/// handled first, the session restores the one untitled buffer from it (text and cursor), and the
/// backup offer is dropped, not prompted.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_session_and_its_backup_restore_in_either_start_order() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut setup = reopen(&root, &state).await;
    new_untitled(&mut setup);
    setup.type_text("notes\nmore");
    let id = setup.active_buffer().unwrap().untitled_id().unwrap().to_string();
    let file = untitled_file(&state, &root, &setup);
    setup.shutdown().await;
    drop(setup);
    assert!(file.exists(), "the terminal going away writes its backup");
    assert!(
        fs::read_to_string(crate::session::file(&state, &root))
            .unwrap()
            .contains("untitled"),
        "the session keeps the tab"
    );

    for session_first in [true, false] {
        let order = if session_first { "session first" } else { "scan first" };
        let ws = Workspace::load(root.clone());
        let trust = TrustStore::load(state.join("trust.toml"));
        let mut app = App::new(ws, trust, None, true);
        app.state_dir = Some(state.clone());
        app.test_start();
        assert!(
            app.test_start_reads_in_order(session_first).await,
            "{order}: the session read and backup scan are handled"
        );
        assert!(!app.session.loading(), "{order}: the session is read");
        assert!(app.prompt.is_none(), "{order}: restored by the session, not offered");
        assert!(app.backups.offered.is_empty(), "{order}: the offer is dropped");
        let buffers: Vec<(String, Option<String>)> = app
            .buffers
            .iter()
            .map(|(_, b)| (b.rope.to_string(), b.untitled_id().map(str::to_string)))
            .collect();
        assert_eq!(buffers, [("notes\nmore".to_string(), Some(id.clone()))], "{order}");
        assert_eq!(names(&app), ["Untitled-1"], "{order}");
        assert_eq!(
            app.active_mut().unwrap().1.selections.primary().head.0,
            "notes\nmore".len(),
            "{order}: the cursor is where it was"
        );
        assert!(file.exists(), "{order}: its backup stays until it is saved or closed");
        // A kill: the state stays as it was for the other order.
        drop(app);
    }
}

/// A clean quit asks about an untitled buffer first, so what it leaves is neither a session tab
/// nor a backup.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_discarded_at_a_clean_quit_leaves_no_session_tab_or_backup() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = reopen(&root, &state).await;
    new_untitled(&mut app);
    app.type_text("gone");
    back_up(&mut app).await;
    assert_eq!(backup_files(&state, &root).len(), 1);
    app.test_execute(Command::Quit);
    assert!(app.prompt.is_some(), "asks first");
    app.press(KeyCode::Char('d'), M::NONE);
    assert!(app.quitting());
    app.shutdown().await;
    let session = fs::read_to_string(crate::session::file(&state, &root)).unwrap_or_default();
    assert!(!session.contains("untitled"), "{session}");
    assert!(backup_files(&state, &root).is_empty());
    let mut app = reopen(&root, &state).await;
    assert!(app.settle(|a| !a.backups.busy()).await);
    assert!(app.prompt.is_none() && app.buffers.iter().count() == 0);
}

/// A click on the status bar's language opens Change Language Mode; Cmd+S while the Save As is
/// writing doesn't open a second prompt.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_status_bar_language_click_and_save_while_writing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("x");
    draw(&mut app);
    let r = app.hits.status_language.expect("the language is clickable");
    app.mouse(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        r.x,
        r.y,
    );
    let p = app.palette.as_ref().expect("the language picker opens");
    assert!(p.rows.iter().any(|row| row.label == "Rust"), "languages listed");
    app.press(KeyCode::Esc, M::NONE);

    app.press(KeyCode::Char('s'), cmd());
    app.type_text("w.txt");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.active_buffer().unwrap().save_in_flight(), "writing");
    app.press(KeyCode::Char('s'), cmd());
    assert!(app.prompt.is_none(), "no second prompt while it writes");
    wait_saved(&mut app).await;
    assert_eq!(fs::read_to_string(root.join("w.txt")).unwrap(), "x");
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_has_no_language_server_document() {
    let dir = tempfile::tempdir().unwrap();
    let fake = crate::lsp::fake::FakeLauncher::new(crate::lsp::fake::FakeOptions::default());
    let (mut app, _) = open_app(&dir, Some(fake.clone()));
    let state = workspace::normalize(dir.path()).join("state");
    fs::create_dir_all(&state).unwrap();
    app.state_dir = Some(state.clone());
    let opened = app
        .settle(|_| fake.methods().iter().any(|m| m == "textDocument/didOpen"))
        .await;
    assert!(opened, "the C file is opened on its server");
    let opens =
        |f: &crate::lsp::fake::FakeLauncher| f.methods().iter().filter(|m| *m == "textDocument/didOpen").count();
    assert_eq!(opens(&fake), 1);

    assert_eq!(fake.launches().len(), 1, "clangd for a.c: {:?}", fake.launches());

    // A language whose server isn't running yet (ty for Python): an untitled buffer starts
    // none, and gets no document on one that runs (clangd for C++).
    new_untitled(&mut app);
    app.type_text("x = 1");
    pick_language(&mut app, "python");
    assert_eq!(active_language(&app), Language::Python);
    new_untitled(&mut app);
    app.type_text("int y;");
    pick_language(&mut app, "c++");
    assert_eq!(active_language(&app), Language::Cpp);
    app.pump();
    app.settle(|_| false).await;
    assert_eq!(
        fake.launches().len(),
        1,
        "no server for an untitled buffer: {:?}",
        fake.launches()
    );
    assert_eq!(opens(&fake), 1, "an untitled buffer is never opened on a server");
    assert!(!fake.methods().iter().any(|m| m == "textDocument/didChange"));
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_commands_on_one_leave_files_alone() {
    // Every command on an untitled buffer neither panics nor creates a file.
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("fn main() {}\n");
    let before: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    for c in Command::all() {
        if matches!(
            c,
            Command::Quit | Command::CloseTab | Command::Save | Command::NewUntitledFile
        ) {
            continue;
        }
        app.test_execute(c);
        app.prompt = None;
        app.palette = None;
        if app.active_buffer().is_none_or(|b| !b.is_untitled()) {
            let untitled = app.group().tabs.iter().position(|t| {
                t.buffer()
                    .and_then(|id| app.buffers.get(id))
                    .is_some_and(|b| b.is_untitled())
            });
            match untitled {
                Some(i) => app.test_activate_tab(i),
                None => app.test_execute(Command::NewUntitledFile),
            }
            assert!(app.active_buffer().is_some_and(|b| b.is_untitled()), "after {c:?}");
        }
    }
    let after: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(before.len(), after.len());
}

/// A git repo with a modified `f.txt` open, plus an untitled tab holding text.
async fn git_app() -> (tempfile::TempDir, App, PathBuf, PathBuf) {
    use crate::milestone10_tests::{dirs, open_file_app, settle_git};
    let (d, root, state) = dirs();
    crate::git::test_repo::init(&root);
    fs::write(root.join("f.txt"), "one\n").unwrap();
    crate::git::test_repo::commit_all(&root, "one");
    fs::write(root.join("f.txt"), "two\n").unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    app.state_dir = Some(state.clone());
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    new_untitled(&mut app);
    app.type_text("scratch\n");
    (d, app, root, state)
}

fn assert_untitled_intact(app: &App, what: &str) {
    let b = app.active_buffer().unwrap();
    assert!(b.is_untitled(), "{what}");
    assert_eq!(b.rope.to_string(), "scratch\n", "{what}");
    assert!(!b.deleted, "{what}: not marked deleted");
}

/// Disk checks (Reload, Keep Mine) and file system events (the open file deleted, renamed) leave
/// an untitled buffer untitled, not deleted, with its text and no banner.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_disk_checks_and_file_events_leave_it_alone() {
    let (_d, mut app, root, _state) = git_app().await;
    let (u, checked) = (app.group().active_tab().unwrap().buffer, app.disk.checked);
    app.test_execute(Command::ReloadFromDisk);
    app.test_execute(Command::KeepMine);
    assert!(
        !app.disk.busy(u) && app.disk.checked == checked,
        "no disk check of an untitled buffer"
    );
    assert_untitled_intact(&app, "reload / keep mine");
    fs::rename(root.join("f.txt"), root.join("g.txt")).unwrap();
    let f = app
        .buffers
        .iter()
        .find(|(_, b)| b.has_file())
        .map(|(id, _)| id)
        .unwrap();
    assert!(
        app.settle(|a| a.buffers.get(f).is_some_and(|b| b.deleted)).await,
        "f.txt seen gone"
    );
    fs::remove_dir_all(&root).unwrap();
    app.test_execute(Command::ReloadFromDisk);
    assert!(!app.disk.busy(u));
    assert_untitled_intact(&app, "after the folder is gone");
    assert!(
        app.message.as_ref().is_none_or(|m| !m.text.contains("Untitled")),
        "no message about the untitled buffer: {:?}",
        app.message.as_ref().map(|m| &m.text)
    );
}

/// File history, git and compare commands on an untitled tab: no picker, no diff tab, nothing
/// staged, no history on disk, no message about a nameless file.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_file_history_and_git_commands_do_nothing() {
    use crate::milestone10_tests::settle_git;
    let (_d, mut app, root, _state) = git_app().await;
    for c in [
        Command::ShowFileHistory,
        Command::RestoreFileHistory,
        Command::StageHunk,
        Command::UnstageHunk,
        Command::RevertHunk,
        Command::CompareWithSaved,
        Command::StageFile,
    ] {
        app.message = None;
        app.test_execute(c);
        assert!(app.palette.is_none(), "{c:?}: no picker");
        assert!(
            app.group().tabs.iter().all(|t| t.diff().is_none()),
            "{c:?}: no diff tab"
        );
        assert_untitled_intact(&app, &format!("{c:?}"));
        // Only a command's own hint (Revert Hunk's "from its diff"), nothing about this buffer.
        if let Some(m) = &app.message {
            let t = &m.text;
            assert!(!t.contains("Untitled") && !t.contains("git repository"), "{c:?}: {t}");
        }
    }
    assert!(!root.join(".jed").exists(), "no file history");
    assert!(
        settle_git(&mut app, |s| s
            .entry(std::path::Path::new("f.txt"))
            .is_some_and(|e| e.staged.is_none()))
        .await,
        "nothing staged"
    );
    let b = app.active_buffer().unwrap();
    assert!(b.quick_diff.marks().is_empty(), "no gutter markers");
    // Control: on f.txt's tab the same setup does mark and stage.
    app.test_execute(Command::PrevTab);
    assert!(app.active_buffer().unwrap().has_file());
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().quick_diff.marks().is_empty())
            .await,
        "f.txt has markers"
    );
    app.test_execute(Command::StageFile);
    assert!(
        settle_git(&mut app, |s| s
            .entry(std::path::Path::new("f.txt"))
            .is_some_and(|e| e.staged.is_some()))
        .await,
        "f.txt staged"
    );
}

/// Inline blame on, an untitled tab shown: nothing is blamed or started.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_buffer_is_not_blamed() {
    let (_d, mut app, _root, _state) = git_app().await;
    app.test_execute(Command::ToggleBlame);
    assert!(app.blame.on);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    // `sync_blame` (run by every pump) would start a job at once for a blamable buffer.
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.settle(|a| !a.git.running()).await);
    app.pump();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.pump();
    let b = app.active_buffer().unwrap();
    assert!(!b.blame.running() && b.blame.hunks().is_empty());
    assert_untitled_intact(&app, "blame");
    // Control: f.txt's tab shown is blamed.
    app.test_execute(Command::PrevTab);
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().blame.hunks().is_empty())
            .await,
        "f.txt blamed"
    );
}

/// A clean quit leaves an untitled tab out of the session: the session restores with the file's
/// tab only, no tab with an empty path.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_clean_quit_leaves_it_out_of_the_session() {
    let (_d, mut app, root, state) = git_app().await;
    assert!(app.settle(|a| !a.session.loading()).await, "restore done");
    app.test_execute(Command::Quit);
    // The unsaved untitled buffer asks first; discard it.
    if app.prompt.is_some() {
        app.press(KeyCode::Char('n'), M::NONE);
        app.press(KeyCode::Char('d'), M::NONE);
    }
    app.test_execute(Command::Quit);
    app.shutdown().await;
    let file = crate::session::file(&state, &root);
    let text = fs::read_to_string(&file).unwrap_or_default();
    assert!(text.contains("f.txt"), "the file's tab saved: {text}");
    assert!(!text.contains("Untitled") && !text.contains("\"path\": \"\""), "{text}");
}

/// Messages about documents the fake server received, as (method, uri).
fn doc_messages(fake: &crate::lsp::fake::FakeLauncher) -> Vec<(String, String)> {
    fake.received()
        .iter()
        .filter_map(|m| {
            let method = m["method"].as_str()?;
            method.starts_with("textDocument/did").then_some(())?;
            let uri = m["params"]["textDocument"]["uri"].as_str()?;
            Some((method.trim_start_matches("textDocument/").to_string(), uri.to_string()))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_moves_the_language_server_document() {
    let dir = tempfile::tempdir().unwrap();
    // A server that wants didSave (with the text, which the fake checks against its copy).
    let fake = crate::lsp::fake::FakeLauncher::new(crate::lsp::fake::FakeOptions {
        sync: Some(serde_json::json!({ "openClose": true, "change": 2, "save": { "includeText": true } })),
        ..Default::default()
    });
    let (mut app, root) = open_app(&dir, Some(fake.clone()));
    assert!(
        app.settle(|_| doc_messages(&fake).iter().any(|(m, _)| m == "didOpen"))
            .await
    );
    let uri = |name: &str| crate::lsp::uri::from_path(&root.join(name)).as_str().to_string();

    // File to file: close a.c, open b.c, never a message about b.c before its open.
    app.type_text("z");
    app.press(KeyCode::Char('s'), cmd() | M::SHIFT);
    app.type_text("b.c");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    let opened_b = |a: &App| a.lsp.version(a.group().active_tab().unwrap().buffer).is_some();
    let saved_b = || doc_messages(&fake).contains(&("didSave".to_string(), uri("b.c")));
    assert!(
        app.settle(|a| opened_b(a) && saved_b()).await,
        "{:?}",
        doc_messages(&fake)
    );
    app.type_text("q");
    app.pump();
    assert!(
        app.settle(|_| doc_messages(&fake)
            .iter()
            .any(|(m, u)| m == "didChange" && *u == uri("b.c")))
            .await
    );
    let msgs = doc_messages(&fake);
    let at = |m: &str, n: &str| msgs.iter().position(|(mm, u)| mm == m && *u == uri(n));
    let (close_a, open_b) = (
        at("didClose", "a.c").expect("close a"),
        at("didOpen", "b.c").expect("open b"),
    );
    assert!(close_a < open_b, "{msgs:?}");
    // The Save As's didSave is b.c's, after its open (none for a.c).
    assert!(at("didSave", "b.c").expect("save b") > open_b, "{msgs:?}");
    assert_eq!(at("didSave", "a.c"), None, "{msgs:?}");
    assert!(
        msgs.iter().position(|(_, u)| *u == uri("b.c")) == Some(open_b),
        "b.c's first message is its didOpen: {msgs:?}"
    );
    assert!(
        msgs.iter().skip(close_a + 1).all(|(_, u)| *u != uri("a.c")),
        "nothing about a.c after its close: {msgs:?}"
    );
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());

    // Untitled to file: just a didOpen.
    new_untitled(&mut app);
    app.type_text("int y;\n");
    let before = doc_messages(&fake).len();
    app.press(KeyCode::Char('s'), cmd());
    app.type_text("x.c");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    // The fake records a message before applying it: wait for its copy of x.c and the save.
    let x_text = || fake.document(&uri("x.c")).map(|d| d.text);
    let saved_x = || doc_messages(&fake).contains(&("didSave".to_string(), uri("x.c")));
    assert!(
        app.settle(|_| x_text().as_deref() == Some("int y;\n") && saved_x())
            .await,
        "{:?} {:?}",
        x_text(),
        doc_messages(&fake)
    );
    let msgs = doc_messages(&fake);
    let x: Vec<_> = msgs[before..].iter().filter(|(_, u)| *u == uri("x.c")).collect();
    assert_eq!(x[0].0, "didOpen", "{msgs:?}");
    assert!(msgs[before..].iter().all(|(m, _)| m != "didClose"), "{msgs:?}");
    assert!(fake.errors().is_empty(), "{:?}", fake.errors());
    app.shutdown_servers().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_in_a_repo_follows_git_history_and_the_explorer() {
    use crate::milestone10_tests::settle_git;
    let (_d, mut app, root, _state) = git_app().await;
    app.test_execute(Command::PrevTab);
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().quick_diff.marks().is_empty())
            .await
    );
    app.press(KeyCode::Char('s'), cmd() | M::SHIFT);
    assert_eq!(save_as_text(&app).as_deref(), Some("f.txt"));
    app.type_text("g.txt");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;

    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        "two\n",
        "f.txt untouched"
    );
    assert_eq!(fs::read_to_string(root.join("g.txt")).unwrap(), "two\n");
    assert!(
        app.active_buffer().unwrap().quick_diff.marks().is_empty(),
        "f.txt's markers dropped at once"
    );
    // Source Control lists the new, untracked file; it has no markers or blame.
    assert!(
        settle_git(&mut app, |s| s.entry(std::path::Path::new("g.txt")).is_some()).await,
        "g.txt in Source Control"
    );
    assert!(app.settle(|a| !a.active_buffer().unwrap().quick_diff.running()).await);
    assert!(app.active_buffer().unwrap().quick_diff.marks().is_empty());
    // The Explorer shows the new file.
    assert!(
        app.settle(|a| a.explorer.rows().iter().any(|r| r.entry.path == root.join("g.txt")))
            .await,
        "g.txt listed in the explorer"
    );
    // The git history view follows the active path; the old file keeps its own.
    app.pump();
    assert_eq!(app.git_history.path, Some(root.join("g.txt")));
    // File history of the buffer is g.txt's (the Save As snapshot).
    app.test_execute(Command::ShowFileHistory);
    assert!(app.settle(|a| a.palette.is_some()).await, "history of g.txt listed");
}

/// Save As over a tracked file (after Overwrite): the gutter markers and the blame become the
/// new path's (against its own index and HEAD text), and the old path's are gone at once.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_onto_a_tracked_file_takes_its_markers_and_blame() {
    use crate::blame::Line;
    use crate::git::HunkKind;
    use crate::git::test_repo::{commit_all, init};
    use crate::milestone10_tests::{dirs, open_file_app, settle_git};
    let (_d, root, state) = dirs();
    init(&root);
    fs::write(root.join("f.txt"), "one\n").unwrap();
    commit_all(&root, "f's commit");
    fs::write(root.join("h.txt"), "x\ntwo\n").unwrap();
    commit_all(&root, "h's commit");
    fs::write(root.join("f.txt"), "two\n").unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    app.test_execute(Command::ToggleBlame);

    let kinds = |a: &App| -> Vec<HunkKind> {
        let b = a.active_buffer().unwrap();
        b.quick_diff.marks().iter().map(|m| m.kind).collect()
    };
    // The first line's blame: a commit's summary, or "uncommitted".
    let blame = |a: &App| -> Option<String> {
        let b = a.active_buffer().unwrap();
        match b.blame.line(&b.rope, 0)? {
            Line::Committed(c) => Some(c.summary.clone()),
            Line::Uncommitted => Some("uncommitted".into()),
        }
    };
    let idle = |a: &App| {
        let b = a.active_buffer().unwrap();
        !a.git.running() && !b.quick_diff.running() && !b.blame.running()
    };
    // f.txt: "one" became "two", not committed.
    assert!(
        app.settle(|a| idle(a) && kinds(a) == [HunkKind::Modified] && blame(a).as_deref() == Some("uncommitted"))
            .await,
        "f.txt's markers and blame: {:?} {:?}",
        kinds(&app),
        blame(&app)
    );

    app.press(KeyCode::Char('s'), cmd() | M::SHIFT);
    app.press(KeyCode::Char('a'), cmd());
    app.type_text("h.txt");
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| matches!(a.prompt, Some(Prompt::ConfirmOverwrite { .. })))
            .await
    );
    app.press(KeyCode::Char('o'), M::NONE);
    wait_saved(&mut app).await;
    assert_eq!(app.active_buffer().unwrap().path, root.join("h.txt"));
    assert_eq!(
        fs::read_to_string(root.join("f.txt")).unwrap(),
        "two\n",
        "f.txt untouched"
    );
    assert_eq!(fs::read_to_string(root.join("h.txt")).unwrap(), "two\n");
    // At once, f.txt's markers and blame are gone (h.txt's may already have landed: the
    // settle in `wait_saved` starts their computations).
    assert!(
        kinds(&app) != [HunkKind::Modified] && blame(&app).as_deref() != Some("uncommitted"),
        "f.txt's markers and blame dropped at once: {:?} {:?}",
        kinds(&app),
        blame(&app)
    );
    // h.txt: its index and HEAD text is "x\ntwo\n": a line deleted above "two", which is h's.
    assert!(
        app.settle(|a| idle(a) && kinds(a) == [HunkKind::Deleted] && blame(a).as_deref() == Some("h's commit"))
            .await,
        "h.txt's markers and blame: {:?} {:?}",
        kinds(&app),
        blame(&app)
    );
}

/// The old path's diagnostics are dropped when a Save As moves the buffer, and a later publish
/// for the old uri doesn't land on it; the new uri's do.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_as_drops_the_old_paths_diagnostics() {
    use crate::milestone9_tests::{diag, publish};
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let fake = crate::lsp::fake::FakeLauncher::new(crate::lsp::fake::FakeOptions::default());
    let (mut app, root) = open_app(&dir, Some(fake.clone()));
    assert!(
        app.settle(|_| doc_messages(&fake).iter().any(|(m, _)| m == "didOpen"))
            .await
    );
    let messages = |a: &App| -> Vec<String> {
        let b = a.active_buffer().unwrap();
        b.diagnostics.all().iter().map(|m| m.diag.message.clone()).collect()
    };
    publish(
        &app,
        Language::C,
        &root.join("a.c"),
        None,
        json!([diag(0, 4, 5, 1, "on a")]),
    );
    assert!(app.settle(|a| messages(a) == ["on a"]).await, "a.c's squiggle");

    app.press(KeyCode::Char('s'), cmd() | M::SHIFT);
    app.press(KeyCode::Char('a'), cmd());
    app.type_text("b.c");
    app.press(KeyCode::Enter, M::NONE);
    wait_saved(&mut app).await;
    assert_eq!(app.active_buffer().unwrap().path, root.join("b.c"));
    assert!(
        messages(&app).is_empty(),
        "a.c's diagnostics dropped: {:?}",
        messages(&app)
    );

    // Published in order: a.c's late one first, then b.c's; only b.c's lands.
    publish(
        &app,
        Language::C,
        &root.join("a.c"),
        None,
        json!([diag(0, 0, 3, 1, "late on a")]),
    );
    publish(
        &app,
        Language::C,
        &root.join("b.c"),
        None,
        json!([diag(0, 0, 3, 2, "on b")]),
    );
    assert!(
        app.settle(|a| !messages(a).is_empty()).await,
        "b.c's diagnostics arrive"
    );
    assert_eq!(messages(&app), ["on b"]);
    app.shutdown_servers().await;
}

/// An explorer rename of a tracked file drops its gutter markers at once (they were against the
/// old path's index blob); the new, untracked path has none.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_explorer_rename_drops_the_old_paths_markers() {
    use crate::milestone10_tests::{dirs, open_file_app, settle_git};
    let (_d, root, state) = dirs();
    crate::git::test_repo::init(&root);
    fs::write(root.join("f.txt"), "one\n").unwrap();
    crate::git::test_repo::commit_all(&root, "one");
    fs::write(root.join("f.txt"), "two\n").unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(settle_git(&mut app, |s| !s.entries.is_empty()).await);
    assert!(
        app.settle(|a| !a.active_buffer().unwrap().quick_diff.marks().is_empty())
            .await,
        "f.txt's markers"
    );

    app.press(KeyCode::Char('e'), cmd() | M::SHIFT);
    let f = root.join("f.txt");
    assert!(app.settle(|a| a.explorer.find(&f).is_some()).await, "root listed");
    app.explorer.selected = Some(f.clone());
    app.press(KeyCode::F(2), M::NONE);
    for _ in 0.."f.txt".len() {
        app.press(KeyCode::Backspace, M::NONE);
    }
    app.type_text("g.txt");
    app.press(KeyCode::Enter, M::NONE);
    let g = root.join("g.txt");
    assert!(
        app.settle(|a| a.active_buffer().unwrap().path == g).await,
        "the buffer follows the rename"
    );
    assert!(
        app.active_buffer().unwrap().quick_diff.marks().is_empty(),
        "f.txt's markers dropped at once"
    );
}
