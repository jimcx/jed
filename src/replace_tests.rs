//! Search and replace (`replace`) end to end.
//!
//! replace-a: the Search view's whole-word toggle (Cmd+Alt+W / Alt+W and the `ab` button, its
//! own option, apart from the find widget's) narrows the real search, on disk and in an unsaved
//! buffer alike (`foo` skips `foobar`); file rows collapse / expand by their chevron, Left /
//! Right in the list and the palette's Collapse All / Expand All (drawn `▸` / `▾`); F4 steps
//! into a collapsed file, expanding it; Delete in the list and the hovered row's `×` dismiss a
//! line or a file, the counts (summary and file row) following, the selection moving to the row
//! in its place, F4 skipping what's gone; a re-search brings everything back.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus};
use crate::buffer::Buffer;
use crate::milestone_tests::test_app;
use crate::search_view::RowKey;

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// The find / search toggles: Cmd+Alt on macOS, Alt on Linux.
fn toggle() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER | M::ALT
    } else {
        M::ALT
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A small workspace with `a.txt` open.
fn open_app(dir: &tempfile::TempDir) -> (App, PathBuf) {
    let root = crate::workspace::normalize(dir.path());
    fs::create_dir_all(root.join(".git")).unwrap();
    write(&root, "a.txt", "foo one\nfoobar two\nthe foo three\n");
    write(&root, "src/b.rs", "fn foo() {}\nlet foo_bar = 1;\nfoo(foo);\n");
    write(&root, "src/c.rs", "nothing\n");
    write(&root, "d.txt", "foo\n");
    let mut app = test_app(&root, Buffer::open(&root.join("a.txt")).unwrap());
    app.test_start();
    app.test_plain_whitespace();
    (app, root)
}

fn term() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(100, 30)).unwrap()
}

fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

/// Cmd+Shift+F, the query typed, and the search run to the end.
async fn search_for(app: &mut App, text: &str) {
    app.press(K::Char('f'), cmd() | M::SHIFT);
    app.type_text(text);
    app.press(K::Enter, M::NONE);
    settled(app).await;
}

async fn settled(app: &mut App) {
    assert!(
        app.settle(|a| a.search.as_ref().is_some_and(|r| r.done) && a.search_view.deadline().is_none())
            .await
    );
}

fn total(app: &App) -> usize {
    app.search.as_ref().unwrap().total
}

/// `path:line` (1-based) per result line, in list order.
fn result_lines(app: &App) -> Vec<String> {
    let r = app.search.as_ref().unwrap();
    r.files
        .iter()
        .flat_map(|f| {
            f.lines
                .iter()
                .map(move |l| format!("{}:{}", f.path.display(), l.line + 1))
        })
        .collect()
}

/// The text of screen row `y` between `x0` and `x1`.
fn text(term: &Terminal<TestBackend>, y: u16, x0: u16, x1: u16) -> String {
    let buf = term.backend().buffer();
    (x0..x1).map(|x| buf[(x, y)].symbol()).collect()
}

/// The Search view's results as drawn, one string per row (trimmed).
fn drawn_rows(term: &Terminal<TestBackend>, app: &App) -> Vec<String> {
    let hit = app.hits.search.as_ref().unwrap();
    (0..hit.rows.len() as u16)
        .map(|i| {
            let l = hit.list;
            text(term, l.y + i, l.x, l.x + l.width).trim_end().to_string()
        })
        .collect()
}

/// The screen row the key's results row was drawn on.
fn row_y(app: &App, key: &RowKey) -> u16 {
    let hit = app.hits.search.as_ref().unwrap();
    let i = hit.rows.iter().position(|k| k == key).expect("drawn");
    hit.list.y + i as u16
}

fn key(path: &str, k: usize) -> RowKey {
    (PathBuf::from(path), k)
}

/// The summary line ("6 results in 3 files"), the row under the query.
fn summary(term: &Terminal<TestBackend>, app: &App) -> String {
    let hit = app.hits.search.as_ref().unwrap();
    let l = hit.list;
    text(term, hit.inputs[0].y + 1, l.x, l.x + l.width).trim().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn replace_whole_word_toggle_narrows_the_search() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let mut term = term();
    search_for(&mut app, "foo").await;
    assert_eq!(total(&app), 8, "foobar and foo_bar too");

    // The key: whole word, the Search view's own option (not the find widget's).
    app.press(K::Char('w'), toggle());
    assert!(app.search_view.options.whole_word && !app.find_options.whole_word);
    settled(&mut app).await;
    assert_eq!(
        result_lines(&app),
        ["a.txt:1", "a.txt:3", "d.txt:1", "src/b.rs:1", "src/b.rs:3"]
    );
    assert_eq!(total(&app), 6, "foo(foo) is two");
    draw(&mut term, &mut app);
    let hit = app.hits.search.clone().unwrap();
    let ab = hit.toggles[1];
    assert_eq!(text(&term, ab.y, ab.x, ab.x + 2), "ab");
    let lit = app.theme.get("find.option.active").bg;
    assert_eq!(term.backend().buffer()[(ab.x, ab.y)].bg, lit.unwrap());
    assert_eq!(summary(&term, &app), "6 results in 3 files");

    // An unsaved edit is searched with the same rule.
    app.press(K::Esc, M::NONE);
    assert_eq!(app.ui.focus, Focus::Editor);
    app.type_text("foobar foo ");
    app.press(K::Char('f'), cmd() | M::SHIFT);
    settled(&mut app).await;
    let a = &app.search.as_ref().unwrap().files[0];
    assert_eq!(a.lines[0].text, "foobar foo foo one");
    assert_eq!(a.lines[0].ranges, [7..10, 11..14], "not foobar");
    assert_eq!(total(&app), 7);

    // The button turns it off again: every `foo` in the buffer counts.
    draw(&mut term, &mut app);
    app.mouse(MouseEventKind::Down(MouseButton::Left), ab.x, ab.y);
    assert!(!app.search_view.options.whole_word);
    settled(&mut app).await;
    assert_eq!(total(&app), 10);
    draw(&mut term, &mut app);
    assert_ne!(term.backend().buffer()[(ab.x, ab.y)].bg, lit.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn replace_file_rows_collapse_and_expand() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let mut term = term();
    search_for(&mut app, "foo").await;
    draw(&mut term, &mut app);
    let rows = drawn_rows(&term, &app);
    assert!(rows[0].starts_with(" ▾ a.txt"), "{rows:?}");
    assert_eq!(rows.len(), 3 + 7, "every line shown");

    // The chevron click collapses a.txt: one row, `▸`, the count unchanged.
    let y = row_y(&app, &key("a.txt", 0));
    let x = app.hits.search.as_ref().unwrap().list.x + 1;
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    assert!(app.search_view.list_focused);
    draw(&mut term, &mut app);
    let rows = drawn_rows(&term, &app);
    assert!(rows[0].starts_with(" ▸ a.txt") && rows[0].ends_with(" 3"), "{rows:?}");
    assert!(rows[1].starts_with(" ▾ d.txt"), "{rows:?}");
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 0)));
    // Clicked again: expanded.
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    draw(&mut term, &mut app);
    assert!(drawn_rows(&term, &app)[0].starts_with(" ▾ a.txt"));

    // Keys: Left on a line goes to its file, Left again collapses, Right expands, Right again
    // goes to the first line.
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 2)));
    app.press(K::Left, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 0)));
    app.press(K::Left, M::NONE);
    assert!(app.search.as_ref().unwrap().files[0].collapsed);
    app.press(K::Down, M::NONE);
    assert_eq!(
        app.search_view.list.selected(),
        Some(&key("d.txt", 0)),
        "past the collapsed lines"
    );
    app.press(K::Up, M::NONE);
    app.press(K::Right, M::NONE);
    assert!(!app.search.as_ref().unwrap().files[0].collapsed);
    app.press(K::Right, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 1)));
    assert_eq!(app.search_view.query.text, "foo", "the keys never reach the query");

    // Collapse All from the palette: three rows; F4 steps into the first file, expanding it.
    run_command(&mut app, "Search: Collapse All");
    draw(&mut term, &mut app);
    let rows = drawn_rows(&term, &app);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows.iter().all(|r| r.starts_with(" ▸ ")));
    assert_eq!(
        app.search_view.list.selected(),
        Some(&key("a.txt", 0)),
        "folded to its file"
    );
    app.press(K::F(4), M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 1)));
    let r = app.search.as_ref().unwrap();
    assert!(!r.files[0].collapsed && r.files[1].collapsed);
    // Shift+F4 from a.txt's first match goes back to the last file's last match.
    app.press(K::F(4), M::SHIFT);
    assert_eq!(app.search_view.list.selected(), Some(&key("src/b.rs", 3)));
    assert!(!app.search.as_ref().unwrap().files[2].collapsed);
    run_command(&mut app, "Search: Expand All");
    draw(&mut term, &mut app);
    assert_eq!(drawn_rows(&term, &app).len(), 10);

    // A new search shows everything expanded.
    run_command(&mut app, "Search: Collapse All");
    app.press(K::Char('f'), cmd() | M::SHIFT);
    settled(&mut app).await;
    assert!(app.search.as_ref().unwrap().files.iter().all(|f| !f.collapsed));
}

#[tokio::test(flavor = "multi_thread")]
async fn replace_dismissing_matches_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let mut term = term();
    search_for(&mut app, "foo").await;
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "8 results in 3 files");

    // Delete on a selected line: it goes, the next line takes the selection.
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 1)));
    app.press(K::Delete, M::NONE);
    assert_eq!(result_lines(&app)[..2], ["a.txt:2", "a.txt:3"]);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 1)), "a.txt:2 now");
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "7 results in 3 files");
    let rows = drawn_rows(&term, &app);
    assert!(rows[0].contains("a.txt") && rows[0].ends_with(" 2"), "{rows:?}");

    // Delete on a file row: the whole file; its row's place goes to the next file.
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("d.txt", 0)));
    app.press(K::Delete, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("src/b.rs", 0)));
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "6 results in 2 files");

    // The pointer over a row draws its `×`; a click dismisses that row (the selection, on
    // another, keeps what it showed).
    let y = row_y(&app, &key("src/b.rs", 2));
    let list = app.hits.search.as_ref().unwrap().list;
    let x_at = list.x + list.width - 2;
    assert_ne!(text(&term, y, x_at, x_at + 1), "×", "not before the pointer is there");
    app.mouse(MouseEventKind::Moved, list.x + 5, y);
    draw(&mut term, &mut app);
    assert_eq!(text(&term, y, x_at, x_at + 1), "×");
    assert_eq!(
        app.hits.search.as_ref().unwrap().dismiss.len(),
        1,
        "only the hovered row's"
    );
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    app.press(K::Down, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("src/b.rs", 3)));
    app.mouse(MouseEventKind::Down(MouseButton::Left), x_at, y);
    assert_eq!(result_lines(&app), ["a.txt:2", "a.txt:3", "src/b.rs:1", "src/b.rs:3"]);
    assert_eq!(
        app.search_view.list.selected(),
        Some(&key("src/b.rs", 2)),
        "still foo(foo)"
    );
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "5 results in 2 files");

    // F4 never visits what's gone: from foo(foo)'s first match, its second, then around:
    // a.txt:2, a.txt:3, b.rs:1, b.rs:3 (twice).
    let mut seen = Vec::new();
    for _ in 0..6 {
        app.press(K::F(4), M::NONE);
        let (path, row) = app.search_view.list.selected().cloned().unwrap();
        seen.push(format!("{}:{row}", path.display()));
    }
    assert_eq!(
        seen,
        [
            "src/b.rs:2",
            "a.txt:1",
            "a.txt:2",
            "src/b.rs:1",
            "src/b.rs:2",
            "src/b.rs:2"
        ]
    );

    // The last line of a file takes the file; with nothing left the query gets the keys.
    assert!(app.search_view.list_focused);
    for _ in 0..4 {
        app.press(K::Home, M::NONE);
        app.press(K::Down, M::NONE);
        app.press(K::Delete, M::NONE);
    }
    assert!(app.search.as_ref().unwrap().files.is_empty());
    assert!(!app.search_view.list_focused);
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "No results found");

    // A re-search (Enter in the query) brings everything back.
    app.press(K::Enter, M::NONE);
    settled(&mut app).await;
    assert_eq!(total(&app), 8);
    assert!(app.search.as_ref().unwrap().dismissed.is_empty());
}

/// The `×` on a hovered file row drops the whole file; Delete on a collapsed file's row drops
/// it too; what was dismissed stays recorded in `Results` (for Replace All) until a re-search,
/// which brings every file back, expanded.
#[tokio::test(flavor = "multi_thread")]
async fn replace_dismissing_files_by_button_and_collapsed_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let mut term = term();
    search_for(&mut app, "foo").await;
    draw(&mut term, &mut app);
    let list = app.hits.search.as_ref().unwrap().list;
    let x_at = list.x + list.width - 2;

    // Hover d.txt's file row: its `×`; clicked, the file goes.
    let y = row_y(&app, &key("d.txt", 0));
    app.mouse(MouseEventKind::Moved, list.x + 5, y);
    draw(&mut term, &mut app);
    assert_eq!(text(&term, y, x_at, x_at + 1), "×");
    app.mouse(MouseEventKind::Down(MouseButton::Left), x_at, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x_at, y);
    assert_eq!(
        result_lines(&app),
        [
            "a.txt:1",
            "a.txt:2",
            "a.txt:3",
            "src/b.rs:1",
            "src/b.rs:2",
            "src/b.rs:3"
        ]
    );
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "7 results in 2 files");

    // The `×` on a line row of a.txt (foobar): only that line.
    let y = row_y(&app, &key("a.txt", 2));
    app.mouse(MouseEventKind::Moved, list.x + 5, y);
    draw(&mut term, &mut app);
    assert_eq!(text(&term, y, x_at, x_at + 1), "×");
    assert_eq!(app.hits.search.as_ref().unwrap().dismiss.len(), 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x_at, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x_at, y);
    assert_eq!(result_lines(&app)[..2], ["a.txt:1", "a.txt:3"]);

    // src/b.rs collapsed by its chevron (which selects its row), then Delete: all four of its
    // matches go.
    draw(&mut term, &mut app);
    let y = row_y(&app, &key("src/b.rs", 0));
    app.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 1, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), list.x + 1, y);
    assert!(app.search.as_ref().unwrap().files[1].collapsed);
    assert_eq!(app.search_view.list.selected(), Some(&key("src/b.rs", 0)));
    app.press(K::Delete, M::NONE);
    assert_eq!(result_lines(&app), ["a.txt:1", "a.txt:3"]);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 2)), "the last row");
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "2 results in 1 file");
    assert_eq!(drawn_rows(&term, &app).len(), 3);

    // Recorded, for Replace All to leave alone.
    {
        use crate::workspace_search::Dismissed;
        let d = &app.search.as_ref().unwrap().dismissed;
        assert_eq!(d.len(), 3, "{d:?}");
        assert_eq!(d.get(Path::new("d.txt")), Some(&Dismissed::File));
        assert_eq!(d.get(Path::new("src/b.rs")), Some(&Dismissed::File));
        assert_eq!(
            d.get(Path::new("a.txt")),
            Some(&Dismissed::Lines([1].into_iter().collect()))
        );
    }

    // A re-search from the editor: everything back, expanded, nothing recorded.
    app.press(K::Esc, M::NONE);
    app.press(K::Char('f'), cmd() | M::SHIFT);
    settled(&mut app).await;
    assert_eq!(total(&app), 8);
    assert_eq!(result_lines(&app).len(), 7);
    let r = app.search.as_ref().unwrap();
    assert!(r.dismissed.is_empty() && r.files.iter().all(|f| !f.collapsed));
    draw(&mut term, &mut app);
    assert_eq!(summary(&term, &app), "8 results in 3 files");
}

/// Runs a palette command by its title, through the keys.
fn run_command(app: &mut App, title: &str) {
    app.press(K::Char('p'), cmd() | M::SHIFT);
    app.type_text(title);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, title);
    app.press(K::Enter, M::NONE);
}
