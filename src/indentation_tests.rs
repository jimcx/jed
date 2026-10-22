//! The Indentation milestone (`indentation`), end to end. indentation-a: a file's indentation is
//! guessed where it is read (the first file as `main` opens it, quick open, a disk reload) and
//! followed by Tab, Enter, Cmd+] / Cmd+[, the status bar, Format Document's request, and in
//! every split of the buffer a tab's drawn width and the column maths (status bar column, Down,
//! a click); a file with no indented lines, a Makefile and `detect_indentation = false` keep the
//! settings; and the 10 MB detection budget
//! (`milestone_indentation_budget_*`, release).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ropey::Rope;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::command::Command;
use crate::config::settings;
use crate::indentation::{self, Guess, Indentation};
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
