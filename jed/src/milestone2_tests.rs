//! Milestone 2 end to end: workspace, trust, explorer, tabs, file operations, live reload.
//! Drives the real `App` through key events; only the terminal is left out.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus, Prompt};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::workspace::trust::{Trust, TrustStore};
use crate::workspace::{self, Workspace};

/// Cmd on macOS, Ctrl elsewhere — whatever the default keymap uses.
fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn open_app(root: &Path, state: &Path) -> App {
    open_app_inner(root, state, false)
}

/// `open_app`, with the file watcher on (for tests that change files on disk after start).
fn open_app_watched(root: &Path, state: &Path) -> App {
    open_app_inner(root, state, true)
}

fn open_app_inner(root: &Path, state: &Path, watch: bool) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    if watch {
        app.enable_file_watching();
    }
    app.test_start();
    app
}

fn active_name(app: &App) -> Option<String> {
    app.active_buffer().map(|b| b.name())
}

fn rows(app: &App) -> Vec<String> {
    app.explorer
        .rows()
        .iter()
        .map(|r| format!("{}{}", "  ".repeat(r.depth), r.entry.name))
        .collect()
}

fn draw(app: &mut App, term: &mut Terminal<TestBackend>) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

/// Tab labels wider than a narrow bar scroll it: the active tab is revealed, the wheel and the
/// `‹` / `›` arrows move it a tab at a time (the wheel without focusing the group), and a new
/// active tab is revealed again.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_2_tab_bar_scrolls() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path()).join("proj");
    let state = workspace::normalize(dir.path()).join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    let mut app = open_app(&root, &state);

    // Eight tabs, each 20 columns wide; the side panel leaves the bar about 39.
    for i in 0..8 {
        let mut buffer = Buffer::from_str("one\ntwo\n");
        buffer.path = PathBuf::from(format!("/tab-number-{i}.txt"));
        app.test_open(buffer);
    }
    assert_eq!(app.group().active, 7, "the last opened tab is active");
    let mut term = Terminal::new(TestBackend::new(70, 20)).unwrap();
    draw(&mut app, &mut term);

    // The active tab is revealed: the bar scrolled towards the end.
    let first = app.group().scroll;
    assert!(first > 0, "the active tab is revealed");
    assert_eq!(app.hits.groups[0].tabs.first().unwrap().index, first);
    assert!(
        app.hits.groups[0].tabs.iter().any(|t| t.index == 7),
        "the active tab is drawn"
    );
    let bars = app.hits.groups[0].tab_arrows.expect("tabs wider than the bar: arrows");
    let y = app.hits.groups[0].tab_bar.y;
    let screen = term.backend().buffer();
    assert_eq!(screen[(bars.back, y)].symbol(), "‹");
    assert_eq!(screen[(bars.forward, y)].symbol(), "›");
    let arrow = app.theme.get("tab.scrollArrow").fg.unwrap();
    let disabled = app.theme.get("tab.scrollArrow.disabled").fg.unwrap();
    assert_eq!(screen[(bars.back, y)].fg, arrow, "tabs to the left");
    assert_eq!(screen[(bars.forward, y)].fg, disabled, "the last tab is shown");

    // The wheel over the bar moves it a tab at a time, without focusing the group.
    let focus = app.ui.focus;
    let x = app.hits.groups[0].tab_bar.x + 3;
    app.mouse(MouseEventKind::ScrollUp, x, y);
    assert_eq!(app.group().scroll, first - 1);
    assert_eq!(app.ui.focus, focus);
    draw(&mut app, &mut term);
    assert_eq!(app.group().scroll, first - 1);
    app.mouse(MouseEventKind::ScrollDown, x, y);

    // The arrows move it too; the end one stops at the furthest the tabs fit.
    app.mouse(MouseEventKind::Down(MouseButton::Left), bars.back, y);
    assert_eq!(app.group().scroll, first - 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), bars.forward, y);
    draw(&mut app, &mut term);
    assert_eq!(app.group().scroll, first);
    for _ in 0..3 {
        app.mouse(MouseEventKind::Down(MouseButton::Left), bars.forward, y);
        draw(&mut app, &mut term);
    }
    assert_eq!(app.group().scroll, first, "no further than the last tabs fit");

    // A new active tab is revealed: the first.
    app.test_execute(Command::NextTab);
    draw(&mut app, &mut term);
    assert_eq!(app.group().scroll, 0);
    assert_eq!(app.hits.groups[0].tabs.first().unwrap().index, 0);

    // Wide enough for every tab: no arrows, and the bar starts at the first.
    let mut term = Terminal::new(TestBackend::new(220, 20)).unwrap();
    draw(&mut app, &mut term);
    assert!(app.hits.groups[0].tab_arrows.is_none(), "all tabs fit");
    assert_eq!(app.hits.groups[0].tabs.len(), 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_2_workspace_explorer_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path()).join("proj");
    let state = workspace::normalize(dir.path()).join("state");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(root.join("src/main.c"), "int main;\n").unwrap();
    fs::write(root.join("notes.txt"), "hi\n").unwrap();
    fs::write(root.join(".workspace"), "[tasks.build]\ncommand = \"make\"\n").unwrap();

    // Trust: commands present and undecided → asked; the answer is stored.
    let mut app = open_app_watched(&root, &state);
    assert!(matches!(app.prompt, Some(Prompt::Trust { .. })));
    app.press(K::Char('t'), M::NONE);
    assert_eq!(app.trust, Trust::Trusted);
    assert!(
        app.settle(|_| state.join("trust.toml").exists()).await,
        "trust decision written"
    );

    // Explorer lists the root (no tabs yet, so it has focus).
    assert!(app.settle(|a| a.explorer.rows().len() == 3).await, "{:?}", rows(&app));
    assert_eq!(rows(&app), vec!["src", ".workspace", "notes.txt"]);
    assert_eq!(app.ui.focus, Focus::SidePanel);

    // Keyboard: expand src, open main.c.
    app.press(K::Home, M::NONE);
    app.press(K::Right, M::NONE);
    assert!(app.settle(|a| a.explorer.rows().len() == 4).await);
    app.press(K::Down, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| active_name(a).as_deref() == Some("main.c")).await);
    assert_eq!(app.ui.focus, Focus::Editor);

    // Edit, then close the tab: asked to save; saving closes it.
    app.type_text("//");
    app.press(K::Char('w'), cmd());
    assert!(matches!(app.prompt, Some(Prompt::SaveBeforeClose { .. })));
    app.press(K::Char('s'), M::NONE);
    assert!(app.settle(|a| a.group().tabs.is_empty()).await);
    assert_eq!(fs::read_to_string(root.join("src/main.c")).unwrap(), "//int main;\n");

    // New file from the explorer: created, listed, opened.
    app.press(K::Char('e'), cmd() | M::SHIFT);
    app.press(K::Char('n'), cmd() | M::ALT);
    assert!(matches!(app.prompt, Some(Prompt::Input(_))));
    app.type_text("lib/util.c");
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| active_name(a).as_deref() == Some("util.c")).await);
    assert!(
        root.join("src/lib/util.c").is_file(),
        "created next to the selected file"
    );

    // Rename the open file: the tab follows.
    app.press(K::Char('e'), cmd() | M::SHIFT);
    assert!(
        app.settle(|a| a.explorer.selected.as_deref() == Some(&root.join("src/lib/util.c")))
            .await
    );
    app.press(K::F(2), M::NONE);
    for _ in 0.."util.c".len() {
        app.press(K::Backspace, M::NONE);
    }
    app.type_text("helpers.c");
    app.press(K::Enter, M::NONE);
    assert!(app.settle(|a| active_name(a).as_deref() == Some("helpers.c")).await);
    assert!(root.join("src/lib/helpers.c").is_file());
    assert!(
        app.settle(|a| a.explorer.find(&root.join("src/lib/helpers.c")).is_some())
            .await
    );

    // Live .workspace reload: settings apply; unchanged commands don't re-ask.
    fs::write(
        root.join(".workspace"),
        "[settings]\ntab_width = 2\n[tasks.build]\ncommand = \"make\"\n",
    )
    .unwrap();
    assert!(
        app.settle(|a| a.settings.tab_width == 2).await,
        "watcher picked up .workspace"
    );
    assert!(app.prompt.is_none());
    assert_eq!(app.trust, Trust::Trusted);

    // Changed commands → asked again.
    fs::write(root.join(".workspace"), "[tasks.build]\ncommand = \"make all\"\n").unwrap();
    assert!(app.settle(|a| matches!(a.prompt, Some(Prompt::Trust { .. }))).await);
    app.press(K::Esc, M::NONE);
    assert_eq!(app.trust, Trust::Restricted);

    // A file created outside the editor shows up.
    fs::write(root.join("outside.txt"), "").unwrap();
    assert!(
        app.settle(|a| a.explorer.find(&a.workspace.root.join("outside.txt")).is_some())
            .await
    );

    // Quit with unsaved changes: asked; Cancel keeps running.
    app.press(K::Char('1'), cmd());
    app.type_text("x");
    app.press(K::Char('q'), M::CONTROL);
    assert!(matches!(&app.prompt, Some(Prompt::SaveAllBeforeQuit { buffers }) if buffers.len() == 1));
    app.press(K::Char('c'), M::NONE);
    assert!(!app.quitting());
    app.press(K::Char('q'), M::CONTROL);
    app.press(K::Char('d'), M::NONE);
    assert!(app.quitting());
}

#[tokio::test(flavor = "multi_thread")]
async fn reopening_trusted_workspace_does_not_ask() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path()).join("p");
    let state = workspace::normalize(dir.path()).join("s");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(root.join(".workspace"), "[lsp.c]\ncommand = \"clangd\"\n").unwrap();

    let mut app = open_app(&root, &state);
    app.press(K::Char('r'), M::NONE);
    assert!(app.settle(|_| state.join("trust.toml").exists()).await);
    drop(app);

    let app = open_app(&root, &state);
    assert!(app.prompt.is_none());
    assert_eq!(app.trust, Trust::Restricted);
}

/// Regression: `jed src/main.rs` panicked because `App::new` started a listing outside the runtime.
#[test]
fn new_with_file_inside_workspace_needs_no_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let buffer = crate::buffer::Buffer::open(&root.join("src/main.rs")).unwrap();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(
        ws,
        trust,
        Some(crate::app::InitialFile {
            buffer,
            goto: Some((1, 4)),
        }),
        true,
    );
    assert_eq!(active_name(&app).as_deref(), Some("main.rs"));
    // `main.rs:1:4`: 1-based line and column.
    let s = *app.active_mut().unwrap().1.selections.primary();
    assert_eq!((s.anchor.0, s.head.0), (3, 3));
}

#[tokio::test(flavor = "multi_thread")]
async fn initial_file_is_revealed_after_start() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "").unwrap();
    let buffer = crate::buffer::Buffer::open(&root.join("src/main.rs")).unwrap();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(crate::app::InitialFile { buffer, goto: None }), true);
    app.test_start();
    let target = root.join("src/main.rs");
    assert!(
        app.settle(|a| a.explorer.selected.as_deref() == Some(target.as_path()))
            .await
    );
}
