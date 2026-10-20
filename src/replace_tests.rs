//! Search and replace (`replace`) end to end.
//!
//! replace-a: the Search view's whole-word toggle (Cmd+Alt+W / Alt+W and the `ab` button, its
//! own option, apart from the find widget's) narrows the real search, on disk and in an unsaved
//! buffer alike (`foo` skips `foobar`); file rows collapse / expand by their chevron, Left /
//! Right in the list and the palette's Collapse All / Expand All (drawn `▸` / `▾`); F4 steps
//! into a collapsed file, expanding it; Delete in the list and the hovered row's `×` dismiss a
//! line or a file, the counts (summary and file row) following, the selection moving to the row
//! in its place, F4 skipping what's gone; a re-search brings everything back.
//!
//! replace-b: the replace input (the `▸` toggle left of the query, Cmd+Shift+H from anywhere,
//! between the query and the list in the Tab order; editing it re-searches nothing); a regex
//! search's result lines drawn with each match struck through and followed by its own
//! replacement (`foo(\d)` → `bar$1`), back to plain highlights once the input is empty or hidden;
//! a click (or Enter) on a result while it has text opens the file's replace preview diff tab
//! (the open, unsaved buffer with every match not dismissed replaced), reused for the file's
//! other results and following edits of the replace text, never kept in the session; the wheel
//! moves the hovered row's `×` with the list.

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

/// A workspace for replacing: `a.txt` open (three lines with `foo` + digit), two more files.
fn replace_app(root: &Path) -> App {
    fs::create_dir_all(root.join(".git")).unwrap();
    write(root, "a.txt", "foo1 one\nkeep\nfoo2 foo3\nfoo4\n");
    write(root, "src/b.rs", "let x = foo5;\n");
    write(root, "d.txt", "nothing\n");
    test_app(root, Buffer::open(&root.join("a.txt")).unwrap())
}

/// The current search's generation (a new search, a new one).
fn generation_of(app: &App) -> u64 {
    app.search.as_ref().unwrap().generation
}

fn started(mut app: App) -> App {
    app.test_start();
    app.test_plain_whitespace();
    app
}

/// A regex search for `foo(\d)`, run to the end (the toggle pressed in the query).
async fn regex_search(app: &mut App) {
    app.press(K::Char('f'), cmd() | M::SHIFT);
    if !app.search_view.options.regex {
        app.press(K::Char('r'), toggle());
    }
    app.type_text(r"foo(\d)");
    app.press(K::Enter, M::NONE);
    settled(app).await;
}

/// The drawn text of the results row `key` (trimmed).
fn row_text(term: &Terminal<TestBackend>, app: &App, key: &RowKey) -> String {
    let y = row_y(app, key);
    let l = app.hits.search.as_ref().unwrap().list;
    text(term, y, l.x, l.x + l.width).trim().to_string()
}

/// The cell of row `key` where `needle` starts in its drawn text.
fn cell_of(term: &Terminal<TestBackend>, app: &App, key: &RowKey, needle: &str) -> ratatui::buffer::Cell {
    let y = row_y(app, key);
    let l = app.hits.search.as_ref().unwrap().list;
    let row: Vec<String> = (l.x..l.x + l.width)
        .map(|x| term.backend().buffer()[(x, y)].symbol().to_string())
        .collect();
    let at = (0..row.len())
        .find(|&i| row[i..].concat().starts_with(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in {:?}", row.concat()));
    term.backend().buffer()[(l.x + at as u16, y)].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn replace_input_toggle_keys_and_tab_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::workspace::normalize(dir.path());
    let mut app = started(replace_app(&root));
    let mut term = term();
    search_for(&mut app, "foo").await;
    draw(&mut term, &mut app);
    let hit = app.hits.search.clone().unwrap();
    let toggle_at = hit.replace_toggle;
    assert_eq!(toggle_at.y, hit.inputs[0].y, "beside the query");
    assert_eq!(text(&term, toggle_at.y, toggle_at.x, toggle_at.x + 1), "▸");
    assert_eq!(hit.replace, ratatui::layout::Rect::default(), "hidden");

    // The toggle shows the input under the query, with the keys.
    app.mouse(MouseEventKind::Down(MouseButton::Left), toggle_at.x, toggle_at.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), toggle_at.x, toggle_at.y);
    assert!(app.search_view.replacing);
    assert_eq!(app.search_view.input, crate::search_view::Input::Replace);
    draw(&mut term, &mut app);
    let hit = app.hits.search.clone().unwrap();
    assert_eq!(text(&term, toggle_at.y, toggle_at.x, toggle_at.x + 1), "▾");
    assert_eq!(hit.replace.y, hit.inputs[0].y + 1);
    let r = hit.replace;
    assert!(
        text(&term, r.y, r.x, r.x + r.width).contains("Replace"),
        "its placeholder"
    );

    // Typing edits it and searches nothing.
    let generation = generation_of(&app);
    app.type_text("bar");
    assert_eq!(app.search_view.replace.text, "bar");
    assert_eq!(app.search_view.query.text, "foo");
    assert_eq!(app.search_view.deadline(), None, "no search pending");
    assert_eq!(generation_of(&app), generation);
    draw(&mut term, &mut app);
    assert!(text(&term, r.y, r.x, r.x + r.width).starts_with("bar"));

    // Tab: replace → the list → query → replace; Shift+Tab back to the query.
    use crate::search_view::Input;
    let at = |app: &App| (app.search_view.list_focused, app.search_view.input);
    app.press(K::Tab, M::NONE);
    assert!(app.search_view.list_focused);
    app.press(K::Tab, M::NONE);
    assert_eq!(at(&app), (false, Input::Query));
    app.press(K::Tab, M::NONE);
    assert_eq!(at(&app), (false, Input::Replace));
    app.press(K::Tab, M::SHIFT);
    assert_eq!(at(&app), (false, Input::Query));
    // A click on the replace input gives it the keys.
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 1, r.y);
    assert_eq!(at(&app), (false, Input::Replace));

    // The palette's Toggle Replace hides it: the query has the keys, the text stays.
    run_command(&mut app, "Search: Toggle Replace");
    assert!(!app.search_view.replacing);
    assert_eq!(at(&app), (false, Input::Query));
    assert_eq!(app.search_view.replace.text, "bar");
    app.press(K::Tab, M::NONE);
    assert!(app.search_view.list_focused, "skipped while hidden");

    // Cmd+Shift+H from the editor: shown again, its text selected with the keys (the query has
    // text), and the search runs now.
    app.press(K::Esc, M::NONE);
    assert_eq!(app.ui.focus, Focus::Editor);
    app.press(K::Char('h'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, Focus::SidePanel);
    assert!(app.search_view.replacing && app.search_view.replace.all_selected);
    assert_eq!(at(&app), (false, Input::Replace));
    assert!(generation_of(&app) > generation);
    // With an empty query the query gets the keys.
    app.press(K::Tab, M::SHIFT);
    app.press(K::End, M::NONE);
    for _ in 0..3 {
        app.press(K::Backspace, M::NONE);
    }
    assert_eq!(app.search_view.query.text, "");
    app.press(K::Esc, M::NONE);
    app.press(K::Char('h'), cmd() | M::SHIFT);
    assert_eq!(at(&app), (false, Input::Query));
}

#[tokio::test(flavor = "multi_thread")]
async fn replace_previews_strike_through_each_match() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::workspace::normalize(dir.path());
    let mut app = started(replace_app(&root));
    let mut term = term();
    regex_search(&mut app).await;
    assert_eq!(result_lines(&app), ["a.txt:1", "a.txt:3", "a.txt:4", "src/b.rs:1"]);
    let generation = generation_of(&app);
    app.press(K::Char('h'), cmd() | M::SHIFT);
    settled(&mut app).await;
    let generation = generation_of(&app).max(generation);
    app.type_text("bar$1");
    assert_eq!(generation_of(&app), generation, "the replace text re-searches nothing");
    draw(&mut term, &mut app);

    // Each match struck through, then its own replacement ($1 from that match).
    let line3 = key("a.txt", 2);
    assert_eq!(row_text(&term, &app, &line3), "foo2bar2 foo3bar3");
    assert_eq!(row_text(&term, &app, &key("a.txt", 1)), "foo1bar1 one");
    assert_eq!(row_text(&term, &app, &key("src/b.rs", 1)), "let x = foo5bar5;");
    let removed = app.theme.get("search.replace.removed");
    let inserted = app.theme.get("search.replace.inserted");
    for needle in ["foo2", "oo2", "foo3"] {
        let c = cell_of(&term, &app, &line3, needle);
        assert!(c.modifier.contains(ratatui::style::Modifier::CROSSED_OUT), "{needle}");
        assert_eq!(c.bg, removed.bg.unwrap(), "{needle}");
    }
    for needle in ["bar2", "2 foo3", "bar3"] {
        let c = cell_of(&term, &app, &line3, needle);
        assert!(!c.modifier.contains(ratatui::style::Modifier::CROSSED_OUT), "{needle}");
        assert_eq!(c.bg, inserted.bg.unwrap(), "{needle}");
    }
    let plain = cell_of(&term, &app, &key("a.txt", 1), " one");
    assert!(plain.bg != removed.bg.unwrap() && plain.bg != inserted.bg.unwrap());

    // Emptied: the plain highlights again.
    for _ in 0..5 {
        app.press(K::Backspace, M::NONE);
    }
    draw(&mut term, &mut app);
    assert_eq!(row_text(&term, &app, &line3), "foo2 foo3");
    let matched = app.theme.get("search.match").bg.unwrap();
    let c = cell_of(&term, &app, &line3, "foo2");
    assert_eq!(c.bg, matched);
    assert!(!c.modifier.contains(ratatui::style::Modifier::CROSSED_OUT));

    // Hidden with text in it: no previews either.
    app.type_text("X");
    draw(&mut term, &mut app);
    assert_eq!(row_text(&term, &app, &line3), "foo2X foo3X");
    run_command(&mut app, "Search: Toggle Replace");
    draw(&mut term, &mut app);
    assert_eq!(row_text(&term, &app, &line3), "foo2 foo3");
    assert_eq!(generation_of(&app), generation);
}

#[tokio::test(flavor = "multi_thread")]
async fn replace_preview_diff_tab() {
    let dir = tempfile::tempdir().unwrap();
    let root = crate::workspace::normalize(dir.path());
    let mut app = started(replace_app(&root));
    let mut term = term();
    // An unsaved edit: the preview shows the buffer, not the disk.
    app.type_text("foo9 ");
    regex_search(&mut app).await;
    app.press(K::Char('h'), cmd() | M::SHIFT);
    settled(&mut app).await;
    app.type_text("bar$1");

    // a.txt's first line dismissed (Delete in the list).
    app.press(K::Tab, M::NONE);
    app.press(K::Down, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 1)));
    app.press(K::Delete, M::NONE);
    assert_eq!(result_lines(&app), ["a.txt:3", "a.txt:4", "src/b.rs:1"]);

    // A click on a.txt:3 opens the preview in the active group; the list keeps the keys.
    draw(&mut term, &mut app);
    let tabs = app.group().tabs.len();
    let y = row_y(&app, &key("a.txt", 1));
    let x = app.hits.search.as_ref().unwrap().list.x + 6;
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(app.group().tabs.len(), tabs + 1);
    assert_eq!((app.ui.focus, app.search_view.list_focused), (Focus::SidePanel, true));
    assert!(app.settle(|a| a.diffs_settled()).await);
    let lines = |l: &crate::diff_view::Lines| (0..l.len()).map(|i| l.line(i).to_string()).collect::<Vec<_>>();
    {
        let d = app.group().active_diff().expect("a diff tab");
        assert_eq!(d.pair, crate::diff_view::Pair::REPLACE);
        assert_eq!(d.title(), "a.txt ↔ a.txt (Replace Preview)");
        let doc = d.doc.as_ref().unwrap();
        assert_eq!(lines(&doc.left), ["foo9 foo1 one", "keep", "foo2 foo3", "foo4"]);
        assert_eq!(
            lines(&doc.right),
            ["foo9 foo1 one", "keep", "bar2 bar3", "bar4"],
            "the dismissed line left alone"
        );
        assert_eq!(d.cursor_line(), 2, "on the clicked line");
    }
    draw(&mut term, &mut app);
    let tab_bar = (0..100)
        .map(|x| term.backend().buffer()[(x, 0)].symbol().to_string())
        .collect::<String>();
    assert!(tab_bar.contains("a.txt ↔ a.txt (Replace Preview)"), "{tab_bar}");

    // Another result of the file: the same tab, on its line.
    let y = row_y(&app, &key("a.txt", 2));
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(app.group().tabs.len(), tabs + 1);
    assert!(app.settle(|a| a.diffs_settled()).await);
    assert_eq!(app.group().active_diff().unwrap().cursor_line(), 3);

    // Editing the replace text brings the open preview up to date.
    let r = app.hits.search.as_ref().unwrap().replace;
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 1, r.y);
    app.press(K::End, M::NONE);
    app.type_text("x");
    assert!(
        app.settle(|a| a.diffs_settled()
            && a.group()
                .active_diff()
                .unwrap()
                .doc
                .as_ref()
                .is_some_and(|d| d.right.line(3) == "bar4x"))
            .await
    );

    // Dismissing a line (the clicked a.txt:4) takes its replacement out of the open preview.
    app.press(K::Tab, M::NONE);
    assert!(app.search_view.list_focused);
    assert_eq!(app.search_view.list.selected(), Some(&key("a.txt", 2)));
    app.press(K::Delete, M::NONE);
    assert_eq!(result_lines(&app), ["a.txt:3", "src/b.rs:1"]);
    assert!(
        app.settle(|a| a.diffs_settled()
            && a.group()
                .active_diff()
                .unwrap()
                .doc
                .as_ref()
                .is_some_and(|d| { d.right.line(3) == "foo4" && d.right.line(2) == "bar2x bar3x" }))
            .await,
        "a.txt:4 dismissed: left alone in the preview"
    );

    // Enter on another file's result: its own preview, focused.
    app.press(K::End, M::NONE);
    assert_eq!(app.search_view.list.selected(), Some(&key("src/b.rs", 1)));
    app.press(K::Enter, M::NONE);
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(app.group().tabs.len(), tabs + 2);
    assert!(app.settle(|a| a.diffs_settled()).await);
    let d = app.group().active_diff().unwrap();
    assert_eq!(d.title(), "b.rs ↔ b.rs (Replace Preview)");
    assert_eq!(lines(&d.doc.as_ref().unwrap().right), ["let x = bar5x;"]);
    assert_eq!(
        fs::read_to_string(root.join("src/b.rs")).unwrap(),
        "let x = foo5;\n",
        "nothing written"
    );

    // With the replace input empty, a click opens the file at the match as before.
    draw(&mut term, &mut app);
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 1, r.y);
    app.press(K::End, M::NONE);
    for _ in 0..6 {
        app.press(K::Backspace, M::NONE);
    }
    assert_eq!(app.search_view.replace.text, "");
    let y = row_y(&app, &key("a.txt", 1));
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert!(app.group().active_diff().is_none());
    let buf = app.active_buffer().unwrap();
    assert_eq!(buf.path, root.join("a.txt"));
    let head = app.group().active_tab().unwrap().view.selections.primary().head;
    assert_eq!(crate::buffer::pos::line_of(&buf.rope, head), 2, "at a.txt:3's match");
    assert_eq!(app.group().tabs.len(), tabs + 2);
}

/// A replace preview is never kept in the session; the file's own tab is.
#[tokio::test(flavor = "multi_thread")]
async fn replace_previews_are_not_kept_in_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let base = crate::workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&root).unwrap();
    let mut app = replace_app(&root);
    app.state_dir = Some(state.clone());
    let mut app = started(app);
    assert!(app.settle(|a| !a.session.loading()).await);
    regex_search(&mut app).await;
    app.press(K::Char('h'), cmd() | M::SHIFT);
    settled(&mut app).await;
    app.type_text("bar$1");
    app.press(K::Tab, M::NONE);
    app.press(K::Enter, M::NONE);
    assert!(app.group().active_diff().is_some());
    assert!(app.settle(|a| a.diffs_settled()).await);
    app.test_execute(crate::command::Command::Quit);
    assert!(app.quitting());
    app.shutdown().await;

    let session = crate::session::load(&state, &root).unwrap().expect("written");
    let tabs = &session.groups[0].tabs;
    assert_eq!(tabs.len(), 1, "{tabs:?}");
    assert!(matches!(&tabs[0], crate::session::TabState::Text { path, .. } if path.ends_with("a.txt")));
    assert_eq!(session.groups[0].active, 0);
}

/// The wheel over the results moves the hovered row (and its `×`) with the list.
#[tokio::test(flavor = "multi_thread")]
async fn replace_wheel_moves_the_hovered_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _root) = open_app(&dir);
    let mut term = Terminal::new(TestBackend::new(100, 10)).unwrap();
    search_for(&mut app, "foo").await;
    draw(&mut term, &mut app);
    let list = app.hits.search.as_ref().unwrap().list;
    assert!(list.height < 10, "scrolls: {list:?}");
    let x_at = list.x + list.width - 2;
    app.mouse(MouseEventKind::Moved, list.x + 5, list.y);
    assert_eq!(app.search_view.hover, Some(key("a.txt", 0)));
    app.mouse(MouseEventKind::ScrollDown, list.x + 5, list.y);
    assert_eq!(app.search_view.hover, Some(key("a.txt", 3)), "three rows on");
    draw(&mut term, &mut app);
    let hit = app.hits.search.as_ref().unwrap();
    assert_eq!(hit.rows[0], key("a.txt", 3));
    assert_eq!(
        hit.dismiss,
        vec![(
            ratatui::layout::Rect {
                x: x_at,
                y: list.y,
                width: 1,
                height: 1
            },
            key("a.txt", 3)
        )]
    );
    assert_eq!(text(&term, list.y, x_at, x_at + 1), "×");
    app.mouse(MouseEventKind::ScrollUp, list.x + 5, list.y);
    assert_eq!(app.search_view.hover, Some(key("a.txt", 0)));
}
