//! Milestone 5 end to end: command palette, quick open, go to line, `keybindings.toml`.
//! Done when: every command can be run from the palette; a binding in `keybindings.toml`
//! replaces the default and shows in the palette; Esc closes the palette and does nothing else.
//! Drives the real `App` (and `ui::render` for the drawn palette and the mouse).

use std::fs;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, reload};

use crate::app::{App, Focus, InitialFile, Prompt};
use crate::buffer::Buffer;
use crate::buffer::pos::{self, ByteIdx};
use crate::command::Command;
use crate::config::keymap::{self, Keymap, Platform};
use crate::logging;
use crate::palette::{Action, Mode};
use crate::selection::{Selection, Selections};
use crate::workspace::trust::{Trust, TrustStore};
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn open_app(root: &Path, file: Option<&str>) -> App {
    let initial = file.map(|f| InitialFile {
        buffer: Buffer::open(&root.join(f)).unwrap(),
        goto: None,
    });
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, initial, true);
    app.log_dir = root.join("logs");
    app.test_start();
    app
}

fn workspace_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    (dir, root)
}

fn command_palette(app: &mut App) {
    app.press(K::Char('p'), cmd() | M::SHIFT);
}

fn mode(app: &App) -> Option<Mode> {
    app.palette.as_ref().map(|p| p.mode())
}

fn labels(app: &App) -> Vec<String> {
    app.palette
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.label.clone())
        .collect()
}

/// What Esc in the palette must leave alone.
fn snapshot(app: &App) -> String {
    let tab = app.group().active_tab();
    format!(
        "text={:?} sel={:?} focus={:?} side={} groups={} active={:?} tabs={} tab={:?} selected={:?} rows={} prompt={} msg={:?} quit={}",
        app.active_buffer().map(|b| b.rope.to_string()),
        tab.map(|t| t.view.selections.clone()),
        app.ui.focus,
        app.ui.side_visible,
        app.groups.len(),
        app.groups.active,
        app.group().tabs.len(),
        app.group().active,
        app.explorer.selected,
        app.explorer.rows().len(),
        app.prompt.is_some(),
        app.message.as_ref().map(|m| m.text.clone()),
        app.quitting(),
    )
}

/// Line and column (0-based) of the primary cursor.
fn cursor(app: &App) -> (usize, usize) {
    let buf = app.active_buffer().unwrap();
    let head = app.group().active_tab().unwrap().view.selections.primary().head;
    (pos::line_of(&buf.rope, head), pos::display_col(&buf.rope, head, 4).0)
}

fn draw(app: &mut App, term: &mut Terminal<TestBackend>) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

fn screen_text(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer();
    let mut s = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.push('\n');
    }
    s
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_every_command_runs_from_the_palette() {
    let (_dir, root) = workspace_dir();
    fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let mut app = open_app(&root, Some("a.txt"));

    // The palette lists every command but its own keys, each with its current binding.
    command_palette(&mut app);
    assert_eq!(mode(&app), Some(Mode::Commands));
    let listed: Vec<(Command, Option<String>)> = app
        .palette
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| match r.action {
            Action::Run(c) => (c, r.keys.clone()),
            ref other => panic!("not a command row: {other:?}"),
        })
        .collect();
    // Everything except the palette's own keys (Esc, Enter, arrows inside it).
    let expected: Vec<Command> = Command::all()
        .into_iter()
        .filter(|c| !matches!(c, Command::Palette(_)))
        .collect();
    assert_eq!(listed.len(), expected.len());
    for c in &expected {
        let keys = listed.iter().find(|(l, _)| l == c).map(|(_, k)| k.clone());
        assert_eq!(keys, Some(app.keymap.binding_for(*c).map(|k| k.label())), "{c:?}");
    }
    // The first default key is the one shown (Cmd+Shift+P, not F1; Cmd+Q, not Ctrl+Q).
    let keys_of = |cmd: Command| listed.iter().find(|(c, _)| *c == cmd).unwrap().1.clone();
    let mac = cfg!(target_os = "macos");
    assert_eq!(
        keys_of(Command::Save).as_deref(),
        Some(if mac { "Cmd+S" } else { "Ctrl+S" })
    );
    assert_eq!(
        keys_of(Command::ShowCommands).as_deref(),
        Some(if mac { "Cmd+Shift+P" } else { "Ctrl+Shift+P" })
    );
    assert_eq!(
        keys_of(Command::Quit).as_deref(),
        Some(if mac { "Cmd+Q" } else { "Ctrl+Q" })
    );
    assert_eq!(keys_of(Command::GoToLine).as_deref(), Some("Ctrl+G"));
    assert_eq!(keys_of(Command::ManageWorkspaceTrust), None);
    app.press(K::Esc, M::NONE);

    // Run each one: open the palette, type its title, pick its row, Enter.
    for c in expected {
        command_palette(&mut app);
        app.type_text(&c.title());
        let rows = &app.palette.as_ref().unwrap().rows;
        let index = rows
            .iter()
            .position(|r| r.action == Action::Run(c))
            .unwrap_or_else(|| panic!("typing its title doesn't find {c:?}"));
        for _ in 0..index {
            app.press(K::Down, M::NONE);
        }
        app.last_command = None;
        app.press(K::Enter, M::NONE);
        assert_eq!(app.last_command, Some(c), "Enter ran the selected command");
        let reopens = matches!(
            c,
            Command::ShowCommands
                | Command::QuickOpen
                | Command::GoToLine
                | Command::SetLogLevel
                | Command::OpenToolLog
                | Command::IndentUsingSpaces
                | Command::IndentUsingTabs
                | Command::ChangeTabDisplaySize
                | Command::ChangeLanguageMode
        );
        assert_eq!(app.palette.is_some(), reopens, "{c:?}");
        // Dismiss whatever it opened (prompts, the palette again, the cheat sheet).
        while app.prompt.is_some() || app.palette.is_some() || app.cheat_sheet.is_some() {
            app.press(K::Esc, M::NONE);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_palette_commands_take_effect() {
    let (_dir, root) = workspace_dir();
    fs::write(root.join("a.txt"), "one\n").unwrap();
    fs::write(root.join("b.txt"), "").unwrap();
    let mut app = open_app(&root, Some("a.txt"));

    // Explorer commands act on the explorer when it has focus.
    assert!(app.settle(|a| a.explorer.rows().len() == 2).await);
    app.press(K::Char('e'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, Focus::SidePanel);
    command_palette(&mut app);
    app.type_text("explorer select last");
    app.press(K::Enter, M::NONE);
    assert_eq!(app.explorer.selected, Some(root.join("b.txt")));
    command_palette(&mut app);
    app.type_text("explorer select first");
    app.press(K::Enter, M::NONE);
    assert_eq!(app.explorer.selected, Some(root.join("a.txt")));
    assert_eq!(app.ui.focus, Focus::SidePanel, "the palette gives focus back");
    app.press(K::Char('1'), cmd());

    app.type_text("X");
    command_palette(&mut app);
    app.type_text("file save");
    assert_eq!(labels(&app)[0], "File: Save");
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "Xone\n");

    // Editing commands act on the active editor.
    command_palette(&mut app);
    app.type_text("select all");
    assert_eq!(labels(&app)[0], "Selection: Select All");
    app.press(K::Enter, M::NONE);
    let sel = app.group().active_tab().unwrap().view.selections.primary();
    assert_eq!((sel.anchor, sel.head), (ByteIdx(0), ByteIdx(5)));
    command_palette(&mut app);
    app.type_text("edit: delete left");
    app.press(K::Enter, M::NONE);
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "");
    command_palette(&mut app);
    app.type_text("edit: undo");
    app.press(K::Enter, M::NONE);
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "Xone\n");

    command_palette(&mut app);
    app.type_text("split right");
    app.press(K::Enter, M::NONE);
    assert_eq!(app.groups.len(), 2);

    // Also matched by stable name.
    command_palette(&mut app);
    app.type_text("view.toggleSidebar");
    assert_eq!(
        app.palette.as_ref().unwrap().rows[0].action,
        Action::Run(Command::ToggleSidebar)
    );
    app.press(K::Enter, M::NONE);
    assert!(!app.ui.side_visible);

    // Backspacing the `>` turns the palette into quick open, as VS Code.
    command_palette(&mut app);
    app.press(K::Backspace, M::NONE);
    assert_eq!(mode(&app), Some(Mode::Files));
    app.type_text(">");
    assert_eq!(mode(&app), Some(Mode::Commands));
    // A non-palette key closes the palette first, then acts.
    app.press(K::Char('w'), cmd());
    assert!(app.palette.is_none());
    assert_eq!(app.groups.len(), 1, "Cmd+W closed the second group's tab");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_esc_closes_the_palette_and_nothing_else() {
    let (_dir, root) = workspace_dir();
    fs::write(root.join("a.txt"), "hello world\nline two\n").unwrap();
    let mut app = open_app(&root, Some("a.txt"));
    let two = Selections::new(
        vec![
            Selection::new(ByteIdx(0), ByteIdx(5)),
            Selection::new(ByteIdx(12), ByteIdx(16)),
        ],
        1,
    );
    app.active_mut().unwrap().1.selections = two.clone();

    // Every way in; Esc in the editor alone would drop the extra cursor.
    let openers = [
        (K::Char('p'), cmd() | M::SHIFT),
        (K::Char('p'), cmd()),
        (K::Char('g'), M::CONTROL),
        (K::F(1), M::NONE),
    ];
    assert!(app.settle(|a| a.explorer.rows().len() == 1).await);
    let before = snapshot(&app);
    for (code, mods) in openers {
        app.press(code, mods);
        assert!(app.palette.is_some());
        // With a row selected that Enter would run or open.
        app.type_text("a");
        app.press(K::Esc, M::NONE);
        assert!(app.palette.is_none());
        let view = &app.group().active_tab().unwrap().view;
        assert_eq!(view.selections, two, "selections untouched");
        assert_eq!(app.ui.focus, Focus::Editor);
        assert_eq!(snapshot(&app), before);
    }
    // From the explorer too: focus and the explorer's selection stay.
    app.press(K::Char('e'), cmd() | M::SHIFT);
    app.press(K::Down, M::NONE);
    assert!(app.explorer.selected.is_some());
    let before = snapshot(&app);
    command_palette(&mut app);
    app.press(K::Esc, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(app.ui.focus, Focus::SidePanel);
    assert_eq!(snapshot(&app), before);
    // Nothing arrives later either (e.g. a file opened by a walk).
    let changed = tokio::time::timeout(Duration::from_millis(300), app.settle(|a| snapshot(a) != before)).await;
    assert!(changed.is_err(), "{}", snapshot(&app));
    // With the palette closed, Esc is the editor's again.
    app.press(K::Char('1'), cmd());
    app.press(K::Esc, M::NONE);
    assert_eq!(app.group().active_tab().unwrap().view.selections.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_keybindings_toml_replaces_default_and_shows_in_palette() {
    let (_dir, root) = workspace_dir();
    fs::write(root.join("a.txt"), "one\n").unwrap();
    let config = root.join(keymap::FILE_NAME);
    let cmd_b = if cfg!(target_os = "macos") { "cmd+b" } else { "ctrl+b" };
    fs::write(
        &config,
        format!(
            r#"
            [[binding]]
            key = "ctrl+alt+s"
            command = "file.save"

            # Palette-only: Cmd+B moves down the list there (it toggles the side bar elsewhere).
            [[binding]]
            key = "{cmd_b}"
            command = "palette.next"
            context = "palette"

            [[binding]]
            key = "ctrl+alt+x"
            command = "no.such.command"
            "#
        ),
    )
    .unwrap();
    let mut app = open_app(&root, Some("a.txt"));
    let (km, errors) = Keymap::load(&config, Platform::current());
    app.set_keymap(km, errors);
    let msg = app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert!(msg.contains("no.such.command"), "bad entries are reported: {msg}");

    // The default no longer saves; the new key does.
    app.type_text("X");
    app.press(K::Char('s'), cmd());
    assert!(app.active_buffer().unwrap().is_dirty());
    app.press(K::Char('s'), M::CONTROL | M::ALT);
    assert!(app.settle(|a| !a.active_buffer().unwrap().is_dirty()).await);

    // The palette shows the user's key.
    command_palette(&mut app);
    app.type_text("file save");
    let row = &app.palette.as_ref().unwrap().rows[0];
    assert_eq!(
        (row.label.as_str(), row.keys.as_deref()),
        ("File: Save", Some("Ctrl+Alt+S"))
    );

    // The palette-context binding wins in the palette; elsewhere the key keeps its default.
    app.press(K::Esc, M::NONE);
    command_palette(&mut app);
    app.press(K::Char('b'), cmd());
    assert!(
        app.palette.is_some(),
        "didn't toggle the side bar (which closes the palette)"
    );
    assert_eq!(app.palette.as_ref().unwrap().selected, 1);
    assert!(app.ui.side_visible);
    app.press(K::Down, M::NONE);
    assert_eq!(
        app.palette.as_ref().unwrap().selected,
        1,
        "palette.next lost its default Down"
    );
    app.press(K::Up, M::NONE);
    assert_eq!(app.palette.as_ref().unwrap().selected, 0, "other commands keep theirs");
    app.press(K::Esc, M::NONE);
    app.press(K::Char('b'), cmd());
    assert!(!app.ui.side_visible, "outside the palette Cmd+B toggles the side bar");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_quick_open() {
    let (_dir, root) = workspace_dir();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::create_dir_all(root.join("gen")).unwrap();
    fs::create_dir_all(root.join(".jed/history")).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    fs::write(root.join(workspace::FILE_NAME), "[files]\nexclude = [\"gen/\"]\n").unwrap();
    fs::write(root.join("src/deep/needle.rs"), "a\nbb\n\tccc\n").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("target/needle.rs"), "").unwrap();
    fs::write(root.join("gen/needle.rs"), "").unwrap();
    fs::write(root.join(".jed/history/needle.rs"), "").unwrap();
    let mut app = open_app(&root, None);

    app.press(K::Char('p'), cmd());
    assert_eq!(mode(&app), Some(Mode::Files));
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.rows.len() == 4))
            .await
    );
    let mut all = labels(&app);
    all.sort();
    assert_eq!(all, [".gitignore", ".workspace", "main.rs", "needle.rs"]);

    // Fuzzy; skips .gitignore'd, excluded and .jed/ files.
    app.type_text("ndl");
    // Wait for matching on the whole query (each row comes with its own hits).
    assert!(
        app.settle(|a| a
            .palette
            .as_ref()
            .is_some_and(|p| p.rows.len() == 1 && p.rows[0].label_hits.len() == 3))
            .await
    );
    let row = &app.palette.as_ref().unwrap().rows[0];
    assert_eq!((row.label.as_str(), row.detail.as_str()), ("needle.rs", "src/deep"));
    assert_eq!(row.label_hits, vec![0, 3, 4]);

    // `file:line:col` opens there.
    app.type_text(":2:2");
    assert_eq!(
        app.palette.as_ref().unwrap().rows.len(),
        1,
        "the position isn't part of the match"
    );
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert!(app.settle(|a| a.active_buffer().is_some()).await);
    assert_eq!(app.active_buffer().unwrap().path, root.join("src/deep/needle.rs"));
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(cursor(&app), (1, 1), "line 2, column 2");

    // Again on the already open file: moves the cursor.
    app.press(K::Char('p'), cmd());
    app.type_text("needle:3");
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.rows.len() == 1))
            .await
    );
    app.press(K::Enter, M::NONE);
    assert_eq!(app.group().tabs.len(), 1);
    assert_eq!(cursor(&app), (2, 0));

    // A pasted `file:line` goes into the input, not the editor behind it.
    app.press(K::Char('p'), cmd());
    app.handle_test_event(crossterm::event::Event::Paste("needle.rs:2\n".into()));
    assert_eq!(app.palette.as_ref().unwrap().field.text, "needle.rs:2");
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| p.rows.len() == 1))
            .await
    );
    app.press(K::Enter, M::NONE);
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "a\nbb\n\tccc\n");
    assert_eq!(cursor(&app), (1, 0));

    // No match: Enter does nothing and the palette stays.
    app.press(K::Char('p'), cmd());
    app.type_text("zzzz");
    assert!(
        app.settle(|a| a
            .palette
            .as_ref()
            .is_some_and(|p| p.hint.as_deref() == Some("No matching files")))
            .await
    );
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_go_to_line() {
    let (_dir, root) = workspace_dir();
    fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    let mut app = open_app(&root, Some("a.txt"));

    app.press(K::Char('g'), M::CONTROL);
    assert_eq!(mode(&app), Some(Mode::GoToLine));
    let hint = app.palette.as_ref().unwrap().hint.clone().unwrap();
    assert!(
        hint.contains("Current line: 1") && hint.contains("between 1 and 5"),
        "{hint}"
    );
    app.type_text("3");
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(cursor(&app), (2, 0));

    app.press(K::Char('g'), M::CONTROL);
    app.type_text("2,3");
    app.press(K::Enter, M::NONE);
    assert_eq!(cursor(&app), (1, 2));

    // A column past the end of the line clamps to it; from the explorer, focus goes to the editor.
    app.press(K::Char('e'), cmd() | M::SHIFT);
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("3:99");
    app.press(K::Enter, M::NONE);
    assert_eq!(cursor(&app), (2, 5));
    assert_eq!(app.ui.focus, Focus::Editor);

    // Past the end clamps; not a number: Enter does nothing.
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("99");
    app.press(K::Enter, M::NONE);
    assert_eq!(cursor(&app).0, 4);
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("x");
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_some());
    app.press(K::Esc, M::NONE);

    // Same widget: `:` typed in quick open is go to line.
    app.press(K::Char('p'), cmd());
    app.type_text(":1");
    app.press(K::Enter, M::NONE);
    assert_eq!(cursor(&app), (0, 0));

    // No file open: a hint, and Enter does nothing.
    app.press(K::Char('w'), cmd());
    assert!(app.active_buffer().is_none());
    app.press(K::Char('g'), M::CONTROL);
    app.type_text("2");
    assert_eq!(
        app.palette.as_ref().unwrap().hint.as_deref(),
        Some("Open a file first to go to a line")
    );
    app.press(K::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert!(app.active_buffer().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_trust_and_log_commands() {
    let (_dir, root) = workspace_dir();
    fs::write(root.join(workspace::FILE_NAME), "[tasks.build]\ncommand = \"make\"\n").unwrap();
    fs::write(root.join("a.txt"), "x\n").unwrap();
    let mut app = open_app(&root, Some("a.txt"));
    assert!(matches!(app.prompt, Some(Prompt::Trust { .. })));
    app.press(K::Char('r'), M::NONE);
    assert_eq!(app.trust, Trust::Restricted);

    // Manage Workspace Trust asks again.
    command_palette(&mut app);
    app.type_text("manage workspace trust");
    app.press(K::Enter, M::NONE);
    assert!(matches!(app.prompt, Some(Prompt::Trust { .. })));
    app.press(K::Char('t'), M::NONE);
    assert_eq!(app.trust, Trust::Trusted);

    // Open Log File: the newest log.
    fs::create_dir_all(&app.log_dir).unwrap();
    fs::write(app.log_dir.join("jed.2026-10-13.log"), "old\n").unwrap();
    fs::write(app.log_dir.join("jed.2026-10-14.log"), "INFO starting\n").unwrap();
    command_palette(&mut app);
    app.type_text("open log file");
    app.press(K::Enter, M::NONE);
    let newest = app.log_dir.join("jed.2026-10-14.log");
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.path == newest))
            .await
    );

    // Set Log Level: logging off says so.
    command_palette(&mut app);
    app.type_text("set log level");
    app.press(K::Enter, M::NONE);
    assert_eq!(mode(&app), Some(Mode::LogLevel));
    app.type_text("debug");
    app.press(K::Enter, M::NONE);
    assert_eq!(
        app.message.as_ref().unwrap().text,
        "Logging is off: see the start-up error"
    );

    // With logging on, a picked level or a typed filter replaces the filter.
    let (layer, handle) = reload::Layer::new(EnvFilter::new("info"));
    let _subscriber = tracing_subscriber::registry().with(layer);
    app.log = Some(handle.clone());
    app.press(K::F(1), M::NONE);
    app.type_text("set log level");
    app.press(K::Enter, M::NONE);
    assert!(
        app.palette
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .any(|r| r.detail.ends_with("(current)"))
    );
    app.type_text("deb");
    app.press(K::Enter, M::NONE);
    assert_eq!(logging::current_filter(&handle).as_deref(), Some("debug"));
    app.press(K::F(1), M::NONE);
    app.type_text("set log level");
    app.press(K::Enter, M::NONE);
    app.type_text("jed::app=trace,info");
    assert!(app.palette.as_ref().unwrap().rows.is_empty());
    app.press(K::Enter, M::NONE);
    assert_eq!(logging::current_filter(&handle).as_deref(), Some("jed::app=trace,info"));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_5_palette_drawn_and_clickable() {
    let (_dir, root) = workspace_dir();
    fs::write(root.join("a.txt"), "one\n").unwrap();
    let mut app = open_app(&root, Some("a.txt"));
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();

    command_palette(&mut app);
    app.type_text("split editor");
    draw(&mut app, &mut term);
    let screen = screen_text(&term);
    assert!(screen.contains(">split editor"), "{screen}");
    assert!(screen.contains("View: Split Editor Right"));
    let keys = if cfg!(target_os = "macos") { "Cmd+\\" } else { "Ctrl+\\" };
    assert!(screen.contains(keys), "binding shown: {screen}");

    // Click the "Split Editor Down" row.
    let hit = app.hits.palette.unwrap();
    let index = app
        .palette
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .position(|r| r.action == Action::Run(Command::SplitDown))
        .unwrap();
    app.mouse(
        MouseEventKind::Down(MouseButton::Left),
        hit.rows.x + 2,
        hit.rows.y + index as u16,
    );
    assert!(app.palette.is_none());
    assert_eq!(app.groups.len(), 2);

    // After scrolling, a click picks the row under the mouse.
    command_palette(&mut app);
    draw(&mut app, &mut term);
    let hit = app.hits.palette.unwrap();
    app.mouse(MouseEventKind::ScrollDown, hit.rows.x + 2, hit.rows.y);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.scroll, 3);
    let third = p.rows[p.scroll + 2].action.clone();
    let Action::Run(third) = third else { unreachable!() };
    draw(&mut app, &mut term);
    app.last_command = None;
    app.mouse(MouseEventKind::Down(MouseButton::Left), hit.rows.x + 2, hit.rows.y + 2);
    assert_eq!(app.last_command, Some(third));
    while app.prompt.is_some() || app.palette.is_some() {
        app.press(K::Esc, M::NONE);
    }

    // A click outside closes it and does nothing else.
    command_palette(&mut app);
    draw(&mut app, &mut term);
    let active = app.groups.active;
    app.mouse(MouseEventKind::Down(MouseButton::Left), 1, 28);
    assert!(app.palette.is_none());
    assert_eq!(app.groups.active, active);
}
