//! The Untitled files and Save As task (`untitled`), subtask untitled-a, end to end: Cmd+N opens
//! `Untitled-N` (the lowest free number) as an empty, writable Plain Text tab; its name shows in
//! the tab bar and the status bar; typing works; "Change Language Mode" picks a language that
//! survives re-detection (and overrides a file's); closing an emptied untitled buffer doesn't
//! ask, a closing or quitting one with text does; Save on one writes nothing yet (Save As is
//! untitled-c, and a quit's Save stops there); path-keyed features (language servers, crash
//! backups) skip it.
//!
//! Subtask untitled-b: every path-keyed feature skips an untitled buffer: disk checks and
//! file system events, file history, git (quick diff, stage, compare), blame and sessions.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, InitialFile, Prompt};
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

#[tokio::test(flavor = "multi_thread")]
async fn untitled_save_writes_nothing_and_keeps_the_text() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    new_untitled(&mut app);
    app.type_text("keep me");
    let before: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    app.press(KeyCode::Char('s'), cmd());
    app.pump();
    let after: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(before, after, "no file, and nothing written for the empty path");
    assert!(!root.join("").is_file());
    let buf = app.active_buffer().unwrap();
    assert!(buf.is_dirty() && buf.rope == "keep me");
    assert!(!app.buffers.any_saving());

    // Quit's "Save" saves the file, stops at the untitled buffer and doesn't go on to quit
    // (or ask again) once the file's save is done.
    app.test_activate_tab(0);
    app.type_text("y");
    app.test_execute(Command::Quit);
    assert!(matches!(&app.prompt, Some(Prompt::SaveAllBeforeQuit { buffers }) if buffers.len() == 2));
    app.press(KeyCode::Char('s'), M::NONE);
    assert!(app.settle(|a| !a.buffers.any_saving()).await, "a.c's save ends");
    assert_eq!(fs::read_to_string(root.join("a.c")).unwrap(), "yint x;\n");
    app.pump();
    assert!(!app.quitting(), "the untitled buffer still holds text");
    assert!(app.prompt.is_none(), "the quit stopped: not asked again");
}

#[tokio::test(flavor = "multi_thread")]
async fn untitled_gets_no_crash_backup() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, None);
    let state = workspace::normalize(dir.path()).join("state");
    fs::create_dir_all(&state).unwrap();
    app.state_dir = Some(state.clone());
    app.type_text("// a\n");
    new_untitled(&mut app);
    app.type_text("unsaved");
    let backups = |state: &std::path::Path| -> Vec<PathBuf> {
        let folder = crate::backup::dir(state, &root);
        let mut found: Vec<PathBuf> = fs::read_dir(folder)
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default();
        found.sort();
        found
    };
    let a_c = crate::backup::file(&state, &root, &root.join("a.c"));
    // The timer's backup.
    app.test_backup_tick(crate::backup::INTERVAL);
    assert!(app.settle(|a| !a.backups.busy()).await, "the backup job ends");
    assert_eq!(
        backups(&state),
        std::slice::from_ref(&a_c),
        "only the file is backed up"
    );
    // The terminal going away: backups brought up to date.
    app.shutdown().await;
    assert_eq!(backups(&state), [a_c]);
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

/// An untitled tab is not saved in the session (until untitled-e): the session restores with
/// the file's tab only, no tab with an empty path.
#[tokio::test(flavor = "multi_thread")]
async fn untitled_tabs_are_left_out_of_the_session() {
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
