//! The Indentation milestone (`indentation`), end to end. indentation-a: a file's indentation is
//! guessed where it is read (the first file as `main` opens it, quick open, a disk reload) and
//! followed by Tab, Enter, Cmd+] / Cmd+[, the status bar, Format Document's request, and in
//! every split of the buffer a tab's drawn width and the column maths (status bar column, Down,
//! a click); a file with no indented lines, a Makefile and `detect_indentation = false` keep the
//! settings; and the 10 MB detection budget
//! (`milestone_indentation_budget_*`, release). indentation-b: the status bar item (a click
//! opens the actions), "Indent Using Spaces / Tabs" and "Change Tab Display Size" through the
//! palette's sizes picker (current one marked, Esc changes nothing, only the active buffer, every
//! split of it), "Detect Indentation from Content", a reload dropping what was chosen, and
//! nothing on a hex tab.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ropey::Rope;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::Command;
use crate::config::settings;
use crate::indentation::{self, Guess, Indentation};
use crate::palette::Mode;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    fs::write(&path, text).unwrap();
    path
}

/// An app on a workspace holding `files`, the first open as `main` opens it (guessed with the
/// layered settings), with `user` as the user's `settings.toml`.
fn open_app(dir: &tempfile::TempDir, user: &str, files: &[(&str, &str)]) -> (App, PathBuf) {
    let base = workspace::normalize(dir.path());
    let config_dir = base.join("config");
    let root = base.join("ws");
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(&root).unwrap();
    write(&config_dir, settings::FILE_NAME, user);
    for (rel, text) in files {
        write(&root, rel, text);
    }
    let loaded = settings::load(&config_dir.join(settings::FILE_NAME));
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    let ws = Workspace::load(root.clone()).with_user_settings(loaded.overrides);
    let buffer = Buffer::open_with(&root.join(files[0].0), ws.settings().indentation()).unwrap();
    let trust = TrustStore::load(base.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.config_dir = config_dir;
    app.test_start();
    app.test_plain_whitespace();
    (app, root)
}

/// Opens `name` through quick open (the load runs off-thread, as any open's).
async fn quick_open(app: &mut App, name: &str) {
    app.press(KeyCode::Char('p'), cmd());
    app.type_text(name);
    let found = app
        .settle(|a| {
            a.palette
                .as_ref()
                .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == name))
        })
        .await;
    assert!(found, "quick open lists {name}");
    app.press(KeyCode::Enter, M::NONE);
    let opened = app
        .settle(|a| a.active_buffer().is_some_and(|b| b.name() == name))
        .await;
    assert!(opened, "{name} opened");
}

fn head(app: &App) -> ByteIdx {
    app.group().active_tab().unwrap().view.selections.primary().head
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn status_line(app: &mut App) -> String {
    let mut term = Terminal::new(TestBackend::new(120, 10)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..120).map(|x| buf[(x, 9)].symbol()).collect()
}

/// The active buffer's effective `(insert_spaces, tab_width)`.
fn effective(app: &App) -> (bool, usize) {
    let s = app.settings.for_buffer(app.active_buffer().unwrap());
    (s.insert_spaces, s.tab_width)
}

fn format_options(app: &App) -> serde_json::Value {
    crate::format::options(&app.settings.for_buffer(app.active_buffer().unwrap()))
}

const USER: &str = "[settings]\ninsert_spaces = true\ntab_width = 4\n";
const C_TABS: &str = "int main() {\n\tif (x) {\n\t\ty();\n\t}\n}\n";
const PY_2: &str = "def f():\n  if x:\n    y()\n  return 1\n";

#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_tab_indented_c_file_gets_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("a.c", C_TABS)]);
    assert_eq!(effective(&app), (false, 4), "tabs, drawn at the settings' width");
    assert!(status_line(&mut app).contains("Tab Size: 4"));
    assert_eq!(
        format_options(&app),
        serde_json::json!({ "tabSize": 4, "insertSpaces": false })
    );

    // Tab at the end of the first line.
    app.press(KeyCode::End, M::NONE);
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("int main() {\t\n"), "{:?}", text(&app));
    app.press(KeyCode::Backspace, M::NONE);
    // Enter after `{` indents with a tab.
    app.press(KeyCode::Enter, M::NONE);
    assert!(text(&app).starts_with("int main() {\n\t\n\tif"), "{:?}", text(&app));
    app.type_text("z();");
    // Cmd+] on that line adds a tab.
    app.press(KeyCode::Char(']'), cmd());
    assert!(text(&app).starts_with("int main() {\n\t\tz();\n"), "{:?}", text(&app));
    app.press(KeyCode::Char('['), cmd());
    assert!(text(&app).starts_with("int main() {\n\tz();\n"), "{:?}", text(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_two_space_python_file_gets_two_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("a.c", "x\n"), ("b.py", PY_2)]);
    quick_open(&mut app, "b.py").await;
    assert_eq!(effective(&app), (true, 2));
    assert!(status_line(&mut app).contains("Spaces: 2"));
    assert_eq!(
        format_options(&app),
        serde_json::json!({ "tabSize": 2, "insertSpaces": true })
    );

    // Tab at the start of the file inserts two spaces; Cmd+[ removes them.
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("  def f():\n"), "{:?}", text(&app));
    app.press(KeyCode::Char('['), cmd());
    assert!(text(&app).starts_with("def f():\n"), "{:?}", text(&app));
    // Enter after `def f():` indents one level of two.
    app.press(KeyCode::End, M::NONE);
    app.press(KeyCode::Enter, M::NONE);
    assert!(text(&app).starts_with("def f():\n  \n  if x:"), "{:?}", text(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_files_without_a_guess_follow_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    let user = "[settings]\ninsert_spaces = false\ntab_width = 3\n";
    let (mut app, _) = open_app(&dir, user, &[("flat.txt", "a\nb\nc\n"), ("b.py", PY_2)]);
    assert_eq!(app.active_buffer().unwrap().guess(), Guess::default());
    assert_eq!(effective(&app), (false, 3), "no indented lines: the settings");
    app.press(KeyCode::Tab, M::NONE);
    assert_eq!(text(&app), "\ta\nb\nc\n");
    // The Python file is guessed: spaces, 2.
    quick_open(&mut app, "b.py").await;
    assert_eq!(effective(&app), (true, 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_detect_indentation_off_keeps_the_settings() {
    let dir = tempfile::tempdir().unwrap();
    let user = "[settings]\ninsert_spaces = true\ntab_width = 4\ndetect_indentation = false\n";
    let (mut app, _) = open_app(&dir, user, &[("a.c", C_TABS), ("b.py", PY_2)]);
    assert!(!app.settings.detect_indentation);
    assert_eq!(effective(&app), (true, 4));
    assert!(status_line(&mut app).contains("Spaces: 4"));
    app.press(KeyCode::End, M::NONE);
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("int main() {    \n"), "{:?}", text(&app));
    quick_open(&mut app, "b.py").await;
    assert_eq!(effective(&app), (true, 4));
    assert_eq!(
        format_options(&app),
        serde_json::json!({ "tabSize": 4, "insertSpaces": true })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_makefile_indented_with_spaces_still_gets_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let make = "all:\n    cc -o a a.c\n    ./a\n";
    let (mut app, _) = open_app(&dir, USER, &[("Makefile", make)]);
    assert_eq!(
        app.active_buffer().unwrap().guess().spaces,
        Some(true),
        "the text says spaces"
    );
    assert_eq!(effective(&app), (false, 4));
    app.press(KeyCode::End, M::NONE);
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("all:\n\t\n"), "{:?}", text(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_a_clean_buffer_is_guessed_again_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, USER, &[("b.py", PY_2)]);
    assert_eq!(effective(&app), (true, 2));
    // A chosen indentation is dropped by the reload too.
    app.active_mut().unwrap().0.indentation = Some(Indentation { spaces: true, size: 8 });
    assert_eq!(effective(&app), (true, 8));
    let tabs = "def f():\n\tif x:\n\t\ty()\n\treturn 1\n";
    fs::write(root.join("b.py"), tabs).unwrap();
    assert!(app.settle(|a| text(a) == tabs).await, "reloaded");
    assert!(!app.active_buffer().unwrap().is_dirty());
    assert_eq!(app.active_buffer().unwrap().indentation, None);
    assert_eq!(effective(&app), (false, 4));
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("\tdef f():"), "{:?}", text(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_every_split_follows_the_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("b.py", PY_2)]);
    app.test_execute(Command::SplitRight);
    let ids: Vec<_> = app.groups.iter_mut().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), 2);
    // What "Indent Using Tabs" → 8 will set (indentation-b).
    app.active_mut().unwrap().0.indentation = Some(Indentation { spaces: false, size: 8 });
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("\tdef f():"), "{:?}", text(&app));

    let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let cells = term.backend().buffer();
    for &id in &ids {
        let t = app.hits.group(id).unwrap().text;
        assert_eq!(
            cells[(t.x + 8, t.y)].symbol(),
            "d",
            "the tab is drawn 8 wide in group {id:?}"
        );
        assert_eq!(cells[(t.x + 7, t.y)].symbol(), " ");
    }
    // Column maths at the buffer's width: the status bar's column, Down's goal column and a click.
    assert!(
        status_line(&mut app).contains("Ln 1, Col 9"),
        "{}",
        status_line(&mut app)
    );
    app.press(KeyCode::Down, M::NONE);
    // "  if x:" is 7 columns: column 8 is past its end (at width 4 it would be column 4).
    assert_eq!(head(&app), ByteIdx(10 + 7));
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let t = app.hits.group(app.groups.active).unwrap().text;
    app.mouse(MouseEventKind::Down(MouseButton::Left), t.x + 8, t.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), t.x + 8, t.y);
    assert_eq!(head(&app), ByteIdx(1), "the cell after the 8-wide tab is the `d`");
    // The other split types a tab too.
    app.test_execute(Command::FocusGroup(1));
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("\t\tdef f():"), "{:?}", text(&app));
}

/// Detection on a 10 MB file (release): the guess reads only its first `MAX_LINES` lines.
#[test]
fn milestone_indentation_budget_detect_10mb() {
    let mut text = String::with_capacity(10_100_000);
    let mut i = 0;
    while text.len() < 10_000_000 {
        let depth = [0, 1, 2, 1][i % 4];
        text.push_str(&"    ".repeat(depth));
        text.push_str("let value = compute(alpha, beta, gamma); // a line of code\n");
        i += 1;
    }
    let rope = Rope::from_str(&text);
    let defaults = Indentation { spaces: false, size: 8 };
    assert_eq!(
        indentation::guess(&rope, defaults),
        Guess {
            spaces: Some(true),
            size: Some(4)
        }
    );
    let runs = 20;
    let start = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(indentation::guess(&rope, defaults));
    }
    let each = start.elapsed() / runs;
    println!("indentation guess on 10 MB: {each:?}");
    let budget = if cfg!(debug_assertions) {
        Duration::from_millis(100)
    } else {
        Duration::from_millis(5)
    };
    assert!(each < budget, "{each:?}");
}

/// Format Document sends the buffer's guessed options, not the settings' (through a language
/// server, the file opened as any open is).
#[tokio::test(flavor = "multi_thread")]
async fn indentation_a_format_document_sends_the_guess() {
    let dir = tempfile::tempdir().unwrap();
    let files = [("a.cpp", C_TABS), (".workspace", USER)];
    let (mut app, fake, _) = crate::milestone9_tests::lsp_project(dir.path(), &files, true).await;
    if cfg!(target_os = "macos") {
        app.press(KeyCode::Char('f'), M::SHIFT | M::ALT);
    } else {
        app.press(KeyCode::Char('i'), M::CONTROL | M::SHIFT);
    }
    assert!(
        app.settle(|_| !fake.requests("textDocument/formatting").is_empty())
            .await,
        "sent"
    );
    let params = fake.requests("textDocument/formatting").pop().unwrap();
    assert_eq!(
        params["options"],
        serde_json::json!({ "tabSize": 4, "insertSpaces": false })
    );
}

/// Runs `title` from the command palette (typed, then Enter).
fn run_from_palette(app: &mut App, title: &str) {
    app.press(KeyCode::Char('p'), cmd() | M::SHIFT);
    app.type_text(title);
    let first = app.palette.as_ref().unwrap().rows.first().map(|r| r.label.clone());
    assert_eq!(first.as_deref(), Some(title));
    app.press(KeyCode::Enter, M::NONE);
}

/// The open picker's rows as (label, detail), and the selected row's label.
fn picker(app: &App) -> (Vec<(String, String)>, String) {
    let p = app.palette.as_ref().expect("a picker is open");
    let rows = p.rows.iter().map(|r| (r.label.clone(), r.detail.clone())).collect();
    (rows, p.selected_row().unwrap().label.clone())
}

/// The active buffer's `(insert_spaces, indent size, tab width)`.
fn resolved(app: &App) -> (bool, usize, usize) {
    let s = app.settings.for_buffer(app.active_buffer().unwrap());
    (s.insert_spaces, s.indent_size(), s.tab_width)
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_indent_using_tabs_from_the_palette_in_both_splits() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("b.py", PY_2)]);
    app.test_open(Buffer::from_str("x\n"));
    let other = app.group().active_tab().unwrap().buffer;
    app.test_execute(Command::PrevTab);
    app.test_execute(Command::SplitRight);
    let ids: Vec<_> = app.groups.iter_mut().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), 2);

    run_from_palette(&mut app, "Indent Using Tabs");
    assert_eq!(app.palette.as_ref().unwrap().mode(), Mode::IndentSize);
    let (rows, selected) = picker(&app);
    let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["1", "2", "3", "4", "5", "6", "7", "8"]);
    assert_eq!(rows[1].1, "current", "the guessed size");
    assert_eq!(rows[3].1, "configured", "the settings' tab_width");
    assert_eq!(selected, "2", "it starts on the current size");
    app.type_text("8");
    assert_eq!(picker(&app).1, "8");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(resolved(&app), (false, 8, 8));
    assert_eq!(
        app.active_buffer().unwrap().indentation,
        Some(Indentation { spaces: false, size: 8 })
    );
    assert!(
        status_line(&mut app).contains("Tab Size: 8"),
        "{}",
        status_line(&mut app)
    );
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("\tdef f():"), "{:?}", text(&app));

    // Drawn 8 wide in both splits.
    let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let cells = term.backend().buffer();
    for &id in &ids {
        let t = app.hits.group(id).unwrap().text;
        assert_eq!(cells[(t.x + 8, t.y)].symbol(), "d", "group {id:?}");
    }
    // The other split types a tab too; the other buffer keeps the settings.
    app.test_execute(Command::FocusGroup(1));
    assert_eq!(resolved(&app), (false, 8, 8));
    let o = app.buffers.get(other).unwrap();
    let s = app.settings.for_buffer(o);
    assert_eq!((s.insert_spaces, s.tab_width, o.indentation), (true, 4, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_status_bar_click_opens_the_actions() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("a.c", C_TABS)]);
    let line = status_line(&mut app);
    let x = line.find("Tab Size: 4").expect("drawn") as u16;
    let at = app.hits.status_indentation.expect("a hit");
    assert_eq!((at.x, at.width), (x, "Tab Size: 4".len() as u16));
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.x + 2, at.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), at.x + 2, at.y);
    assert_eq!(app.palette.as_ref().map(|p| p.mode()), Some(Mode::Indentation));
    let titles: Vec<String> = picker(&app).0.into_iter().map(|(l, _)| l).collect();
    assert_eq!(
        titles,
        [
            "Indent Using Spaces",
            "Indent Using Tabs",
            "Change Tab Display Size",
            "Detect Indentation from Content"
        ]
    );
    // Enter on the first: the spaces picker, the tab file's size current.
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(picker(&app).1, "4");
    app.press(KeyCode::Up, M::NONE);
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(resolved(&app), (true, 3, 3));
    assert!(status_line(&mut app).contains("Spaces: 3"));
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("   int main"), "{:?}", text(&app));
    // Cmd+] adds three more.
    app.press(KeyCode::Char(']'), cmd());
    assert!(text(&app).starts_with("      int main"), "{:?}", text(&app));

    // A click on the rest of the status bar opens nothing.
    status_line(&mut app);
    let at = app.hits.status_indentation.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.x + at.width + 1, at.y);
    assert!(app.palette.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_esc_leaves_it_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("b.py", PY_2)]);
    for title in ["Indent Using Spaces", "Indent Using Tabs", "Change Tab Display Size"] {
        run_from_palette(&mut app, title);
        app.press(KeyCode::Down, M::NONE);
        app.press(KeyCode::Down, M::NONE);
        app.press(KeyCode::Esc, M::NONE);
        assert!(app.palette.is_none(), "{title}");
        assert_eq!(resolved(&app), (true, 2, 2), "{title}");
        let b = app.active_buffer().unwrap();
        assert_eq!((b.indentation, b.tab_display), (None, None), "{title}");
    }
    // A size that isn't offered: no rows, Enter does nothing.
    run_from_palette(&mut app, "Indent Using Spaces");
    app.type_text("9");
    assert!(picker_rows_empty(&app));
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_some());
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(resolved(&app), (true, 2, 2));
}

fn picker_rows_empty(app: &App) -> bool {
    app.palette.as_ref().unwrap().rows.is_empty()
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_change_tab_display_size_keeps_the_indent_size() {
    let dir = tempfile::tempdir().unwrap();
    let py = "def f():\n  if x:\n    y()\n  return 1\n\tz\n";
    let (mut app, _) = open_app(&dir, USER, &[("b.py", py), ("a.c", C_TABS)]);
    assert_eq!(resolved(&app), (true, 2, 2));
    run_from_palette(&mut app, "Change Tab Display Size");
    assert_eq!(picker(&app).1, "2");
    app.type_text("8");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(
        resolved(&app),
        (true, 2, 8),
        "spaces: the indent stays, tabs drawn 8 wide"
    );
    assert_eq!(app.active_buffer().unwrap().tab_display, Some(8));
    assert!(status_line(&mut app).contains("Spaces: 2"));
    let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let t = app.hits.group(app.groups.active).unwrap().text;
    assert_eq!(
        term.backend().buffer()[(t.x + 8, t.y + 4)].symbol(),
        "z",
        "the tab drawn 8 wide"
    );
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("  def"), "{:?}", text(&app));
    // Enter after the `:` adds one level of two, Cmd+[ takes two off, Cmd+] puts them back.
    app.press(KeyCode::End, M::NONE);
    app.press(KeyCode::Enter, M::NONE);
    assert!(text(&app).starts_with("  def f():\n    \n"), "{:?}", text(&app));
    app.type_text("w");
    app.press(KeyCode::Char('['), cmd());
    assert!(text(&app).starts_with("  def f():\n  w\n"), "{:?}", text(&app));
    app.press(KeyCode::Char(']'), cmd());
    assert!(text(&app).starts_with("  def f():\n    w\n"), "{:?}", text(&app));
    let line = status_line(&mut app);
    assert!(line.contains("Ln 2, Col 6"), "the column counts spaces: {line}");
    assert_eq!(
        format_options(&app),
        serde_json::json!({ "tabSize": 2, "insertSpaces": true }),
        "servers get the indent size"
    );
    // Choosing an indentation drops the display size.
    run_from_palette(&mut app, "Indent Using Spaces");
    assert_eq!(picker(&app).1, "2");
    app.type_text("4");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(resolved(&app), (true, 4, 4));
    assert_eq!(app.active_buffer().unwrap().tab_display, None);

    // With tabs it is the tab size.
    quick_open(&mut app, "a.c").await;
    run_from_palette(&mut app, "Change Tab Display Size");
    assert_eq!(picker(&app).1, "4");
    app.type_text("2");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(resolved(&app), (false, 2, 2));
    assert!(status_line(&mut app).contains("Tab Size: 2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_detect_indentation_from_content() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("b.py", "x = 1\n")]);
    assert_eq!(resolved(&app), (true, 4, 4), "nothing to guess from: the settings");
    app.handle_test_event(Event::Paste("def f():\n\tif x:\n\t\ty()\n\treturn 1\n".into()));
    assert_eq!(resolved(&app), (true, 4, 4), "not guessed again by itself");
    // Something chosen before is dropped too.
    run_from_palette(&mut app, "Change Tab Display Size");
    app.type_text("6");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(resolved(&app), (true, 4, 6));
    run_from_palette(&mut app, "Detect Indentation from Content");
    assert!(app.palette.is_none());
    assert_eq!(resolved(&app), (false, 4, 4));
    let b = app.active_buffer().unwrap();
    assert_eq!((b.indentation, b.tab_display), (None, None));
    assert!(status_line(&mut app).contains("Tab Size: 4"));
    // Through the status bar's picker too, after choosing spaces.
    app.active_mut()
        .unwrap()
        .0
        .choose_indentation(Indentation { spaces: true, size: 2 });
    status_line(&mut app);
    let at = app.hits.status_indentation.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.x, at.y);
    app.type_text("detect");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(resolved(&app), (false, 4, 4));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_a_reload_drops_what_was_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, USER, &[("b.py", PY_2)]);
    run_from_palette(&mut app, "Change Tab Display Size");
    app.type_text("8");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(resolved(&app), (true, 2, 8));
    let tabs = "def f():\n\tif x:\n\t\ty()\n\treturn 1\n";
    fs::write(root.join("b.py"), tabs).unwrap();
    assert!(app.settle(|a| text(a) == tabs).await, "reloaded");
    assert_eq!(app.active_buffer().unwrap().tab_display, None);
    assert_eq!(resolved(&app), (false, 4, 4), "guessed again");
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_nothing_on_a_hex_tab() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, USER, &[("b.py", PY_2)]);
    fs::write(root.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
    app.press(KeyCode::Char('p'), cmd());
    app.type_text("bin.dat");
    assert!(
        app.settle(|a| a.palette.as_ref().is_some_and(|p| !p.rows.is_empty()))
            .await,
        "listed"
    );
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| a.active_hex().is_some()).await, "a hex tab");
    for c in [
        Command::IndentUsingSpaces,
        Command::IndentUsingTabs,
        Command::ChangeTabDisplaySize,
        Command::DetectIndentation,
    ] {
        app.test_execute(c);
        assert!(app.palette.is_none(), "{c:?}");
    }
    status_line(&mut app);
    assert_eq!(app.hits.status_indentation, None, "no indentation item");
    let py = app.buffers.iter().find(|(_, b)| b.name() == "b.py").unwrap().1;
    assert_eq!((py.indentation, py.tab_display), (None, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_detect_applies_even_with_detection_off() {
    let dir = tempfile::tempdir().unwrap();
    let user = "[settings]\ninsert_spaces = true\ntab_width = 4\ndetect_indentation = false\n";
    let (mut app, _) = open_app(&dir, user, &[("a.c", C_TABS)]);
    assert_eq!(resolved(&app), (true, 4, 4), "not guessed on load");
    run_from_palette(&mut app, "Detect Indentation from Content");
    assert_eq!(resolved(&app), (false, 4, 4), "asked for: the tabs are found");
    assert!(status_line(&mut app).contains("Tab Size: 4"));
    app.press(KeyCode::Tab, M::NONE);
    assert!(text(&app).starts_with("\tint main"), "{:?}", text(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_the_sizes_picker_sets_the_buffer_it_was_opened_for() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("b.py", PY_2)]);
    let py = app.group().active_tab().unwrap().buffer;
    app.test_open(Buffer::from_str("x\n"));
    let other = app.group().active_tab().unwrap().buffer;
    app.test_execute(Command::PrevTab);
    run_from_palette(&mut app, "Indent Using Tabs");
    // The active tab changes under the open picker (nothing in the UI does this today).
    app.group_mut().active = app.group().tabs.iter().position(|t| t.buffer() == Some(other)).unwrap();
    app.type_text("3");
    app.press(KeyCode::Enter, M::NONE);
    assert_eq!(
        app.buffers.get(py).unwrap().indentation,
        Some(Indentation { spaces: false, size: 3 })
    );
    assert_eq!(app.buffers.get(other).unwrap().indentation, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn indentation_b_the_status_item_is_clipped_on_a_narrow_bar() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, USER, &[("a.c", C_TABS)]);
    for width in [120u16, 60, 40, 34, 30] {
        let mut term = Terminal::new(TestBackend::new(width, 10)).unwrap();
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        let cells = term.backend().buffer();
        let line: String = (0..width).map(|x| cells[(x, 9)].symbol()).collect();
        match app.hits.status_indentation {
            Some(at) => {
                assert!(at.x + at.width <= width, "{width}: {at:?}");
                let drawn: String = (at.x..at.x + at.width).map(|x| cells[(x, 9)].symbol()).collect();
                assert!(
                    "Tab Size: 4".starts_with(&drawn) && !drawn.is_empty(),
                    "{width}: {drawn:?} in {line:?}"
                );
            }
            None => assert!(!line.contains("Tab"), "{width}: drawn but no hit: {line:?}"),
        }
    }
}
