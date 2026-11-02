//! File History section (history_view-a, rows in -b): the Explorer body holds the tree, then "FILE HISTORY",
//! then "GIT HISTORY", each with a header (a click collapses it) and a draggable divider, the
//! tree keeping its minimum height; the new section is empty ("No history"). Git History's own
//! behaviour stays in `git_history_tests`.

use std::fs;

use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::App;
use crate::milestone10_tests::{dirs, open_file_app};

fn app() -> (tempfile::TempDir, App) {
    let (d, root, state) = dirs();
    fs::write(root.join("a.txt"), "one\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    app.ui.side_width = 44;
    (d, app)
}

fn draw(app: &mut App) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

fn row(term: &Terminal<TestBackend>, r: Rect) -> String {
    let buf = term.backend().buffer();
    (r.x..r.x + r.width).map(|x| buf[(x, r.y)].symbol()).collect()
}

fn click(app: &mut App, r: Rect) {
    app.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 3, r.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 3, r.y);
}

fn drag(app: &mut App, from: Rect, to_row: u16) {
    let x = from.x + 2;
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, from.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, to_row);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, to_row);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_draws_both_headers_in_order() {
    let (_d, mut app) = app();
    idle(&mut app).await;
    let term = draw(&mut app);
    let file = app.hits.file_history.unwrap();
    let git = app.hits.history.unwrap();
    assert!(
        row(&term, file.header).contains("▾ FILE HISTORY"),
        "{}",
        row(&term, file.header)
    );
    assert!(row(&term, git.header).contains("▾ GIT HISTORY"));
    assert!(
        app.hits.explorer_rows.y + app.hits.explorer_rows.height <= file.divider.y,
        "the tree is above"
    );
    assert_eq!(
        file.divider.y + 6,
        git.divider.y,
        "the default 6 rows, then Git History"
    );
    let body = Rect {
        y: file.header.y + 1,
        ..file.header
    };
    assert!(row(&term, body).contains("No history"));
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_headers_collapse_their_sections() {
    let (_d, mut app) = app();
    draw(&mut app);
    let tree = app.hits.explorer_rows.height;
    let header = app.hits.file_history.unwrap().header;
    click(&mut app, header);
    assert!(app.file_history_view.collapsed());
    assert!(!app.git_history.collapsed());
    let term = draw(&mut app);
    let (file, git) = (app.hits.file_history.unwrap(), app.hits.history.unwrap());
    assert!(row(&term, file.header).contains("▸ FILE HISTORY"));
    assert_eq!(git.divider.y, file.divider.y + 2, "header only");
    assert_eq!(app.hits.explorer_rows.height, tree + 4, "the tree took the rows");

    click(&mut app, file.header);
    assert!(!app.file_history_view.collapsed());
    let header = app.hits.history.unwrap().header;
    click(&mut app, header);
    assert!(app.git_history.collapsed() && !app.file_history_view.collapsed());
    draw(&mut app);
    let (file, git) = (app.hits.file_history.unwrap(), app.hits.history.unwrap());
    assert_eq!(git.divider.y, file.divider.y + 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_dividers_drag_each_section() {
    let (_d, mut app) = app();
    draw(&mut app);
    let (tree, file, git) = (
        app.hits.explorer_rows.height,
        app.hits.file_history.unwrap(),
        app.hits.history.unwrap(),
    );
    // File History's divider up three rows: it grows, Git History keeps its size.
    drag(&mut app, file.divider, file.divider.y - 3);
    assert_eq!(app.file_history_view.section_height, 9);
    assert_eq!(app.git_history.section_height, 10);
    draw(&mut app);
    assert_eq!(app.hits.explorer_rows.height, tree - 3);
    assert_eq!(
        app.hits.history.unwrap().divider.y,
        git.divider.y,
        "Git History stays put"
    );

    // Git History's divider up two rows: it grows, File History keeps its size.
    let git = app.hits.history.unwrap();
    drag(&mut app, git.divider, git.divider.y - 2);
    assert_eq!(app.git_history.section_height, 12);
    assert_eq!(app.file_history_view.section_height, 9);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_the_tree_keeps_its_minimum() {
    let (_d, mut app) = app();
    draw(&mut app);
    let side = app.hits.regions.side.unwrap();
    // Dragged to the top: each section is clamped so the tree keeps three rows.
    let git = app.hits.history.unwrap();
    drag(&mut app, git.divider, side.y);
    draw(&mut app);
    assert_eq!(app.hits.explorer_rows.height, 3);
    assert_eq!(app.file_history_view.section_height, 6);
    let file = app.hits.file_history.unwrap();
    drag(&mut app, file.divider, side.y);
    draw(&mut app);
    assert_eq!(app.hits.explorer_rows.height, 3, "still three rows after both drags");
    let (file, git) = (app.hits.file_history.unwrap(), app.hits.history.unwrap());
    assert_eq!(git.divider.y - file.divider.y, 2, "File History shrank to its header");
    assert!(app.hits.file_history.is_some() && app.hits.history.is_some());

    // A short window hides File History first, then both.
    let mut term = Terminal::new(TestBackend::new(120, 9)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.hits.file_history.is_none() && app.hits.history.is_some());
    let mut term = Terminal::new(TestBackend::new(120, 6)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(app.hits.file_history.is_none() && app.hits.history.is_none());
}

// ---- history_view-b: the rows ----

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::buffer::Buffer;
use crate::history;

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

/// Saves the active buffer and waits for the save and its history task.
async fn save(app: &mut App) {
    app.press(K::Char('s'), cmd());
    let id = app.group().active_tab().unwrap().buffer;
    assert!(
        app.settle(|a| !a.buffers.get(id).unwrap().save_in_flight() && a.file_history.running() == 0)
            .await,
        "save done"
    );
}

/// Waits for the section's read of the active file to end.
async fn idle(app: &mut App) {
    assert!(
        app.settle(|a| !a.file_history_view.loading && !a.file_history_view.stale)
            .await
    );
}

/// The section's list rows as drawn (blank rows included).
fn list_rows(app: &mut App) -> Vec<String> {
    let term = draw(app);
    let list = app.hits.file_history.unwrap().list;
    (0..list.height)
        .map(|i| {
            row(
                &term,
                Rect {
                    y: list.y + i,
                    height: 1,
                    ..list
                },
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_rows_list_the_entries_newest_first() {
    let (_d, mut app) = app();
    idle(&mut app).await;
    assert!(list_rows(&mut app)[0].contains("No history"));
    app.type_text("x");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    let rows = list_rows(&mut app);
    assert!(
        rows[0].contains("just now") && rows[0].contains("5 B") && rows[0].contains("UTC"),
        "{rows:?}"
    );
    assert!(!rows[0].contains("No history"));

    std::thread::sleep(std::time::Duration::from_millis(2));
    app.type_text("yy");
    save(&mut app).await;
    assert!(
        app.settle(|a| a.file_history_view.entries.len() == 2).await,
        "a new row after one more save"
    );
    let rows = list_rows(&mut app);
    assert!(
        rows[0].contains("7 B") && rows[1].contains("5 B"),
        "newest first: {rows:?}"
    );
    let on_disk = history::entries(&app.workspace.root, std::path::Path::new("a.txt")).unwrap();
    assert_eq!(on_disk[0].id, app.file_history_view.entries[0].id);
    // The date uses its own theme key.
    let term = draw(&mut app);
    let list = app.hits.file_history.unwrap().list;
    let line: String = row(&term, Rect { height: 1, ..list });
    let col = line.find("UTC").unwrap() - 5;
    let cell = &term.backend().buffer()[(list.x + col as u16, list.y)];
    assert_eq!(Some(cell.fg), app.theme.get("fileHistory.date").fg);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_follows_the_active_file_and_says_no_history_elsewhere() {
    let (_d, mut app) = app();
    app.type_text("x");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);

    // A file with no history, then one outside the workspace.
    let root = app.workspace.root.clone();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    app.test_open(Buffer::open(&root.join("b.txt")).unwrap());
    let b = root.join("b.txt");
    assert!(
        app.settle(|a| a.file_history_view.path.as_deref() == Some(b.as_path()))
            .await
    );
    assert!(app.file_history_view.entries.is_empty());
    idle(&mut app).await;
    assert!(list_rows(&mut app)[0].contains("No history"));

    let other = tempfile::tempdir().unwrap();
    let outside = crate::workspace::normalize(other.path()).join("o.txt");
    fs::write(&outside, "o\n").unwrap();
    app.test_open(Buffer::open(&outside).unwrap());
    assert!(
        app.settle(|a| a.file_history_view.path.as_deref() == Some(outside.as_path()))
            .await
    );
    assert!(!app.file_history_view.loading && app.file_history_view.entries.is_empty());
    idle(&mut app).await;
    assert!(list_rows(&mut app)[0].contains("No history"));

    // Back to a.txt: its entry is listed again.
    app.test_activate_tab(0);
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    assert!(list_rows(&mut app)[0].contains("5 B"));
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_reads_nothing_while_collapsed() {
    let (_d, mut app) = app();
    draw(&mut app);
    let header = app.hits.file_history.unwrap().header;
    click(&mut app, header);
    assert!(app.file_history_view.collapsed());
    app.type_text("x");
    save(&mut app).await;
    for _ in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        app.pump();
    }
    assert!(
        app.file_history_view.entries.is_empty() && !app.file_history_view.loading,
        "not read while collapsed"
    );
    // Expanding reads it.
    draw(&mut app);
    let header = app.hits.file_history.unwrap().header;
    click(&mut app, header);
    assert!(!app.file_history_view.collapsed());
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_scrolls_with_the_wheel_the_bar_and_selects_on_click() {
    let (_d, mut app) = app();
    let root = app.workspace.root.clone();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    for i in 0..9 {
        let bytes = format!("v{i}\n");
        history::record(
            &root,
            std::path::Path::new("b.txt"),
            bytes.as_bytes(),
            &Default::default(),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.test_open(Buffer::open(&root.join("b.txt")).unwrap());
    assert!(app.settle(|a| a.file_history_view.entries.len() == 9).await);
    draw(&mut app);
    let list = app.hits.file_history.unwrap().list;
    let bar = app.hits.file_history_scrollbar.expect("scrollable");
    assert_eq!(app.file_history_view.height, 4);
    // Wheel over the rows.
    app.mouse(MouseEventKind::ScrollDown, list.x + 2, list.y + 1);
    assert_eq!(app.file_history_view.scroll(), 3);
    app.mouse(MouseEventKind::ScrollDown, list.x + 2, list.y + 1);
    assert_eq!(app.file_history_view.scroll(), 5, "clamped at the bottom");
    // Dragging the bar to the top scrolls back.
    draw(&mut app);
    let (x, bottom) = (bar.track.x, bar.track.y + bar.track.height - 1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, bottom);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, bar.track.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, bar.track.y);
    assert_eq!(app.file_history_view.scroll(), 0);
    // A click selects the row (by entry id) and gives the section the focus.
    draw(&mut app);
    app.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 3, list.y + 2);
    app.mouse(MouseEventKind::Up(MouseButton::Left), list.x + 3, list.y + 2);
    assert_eq!(app.file_history_view.selected_row(), Some(2));
    assert!(app.file_history_view.focused());
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_a_diff_tab_counts_as_its_file() {
    let (_d, mut app) = app();
    app.type_text("x");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    let root = app.workspace.root.clone();
    fs::write(root.join("b.txt"), "b\n").unwrap();
    app.test_open(Buffer::open(&root.join("b.txt")).unwrap());
    idle(&mut app).await;
    assert!(app.file_history_view.entries.is_empty());
    // A diff tab of a.txt shows a.txt's entries.
    app.open_diff(root.join("a.txt"), crate::diff_view::Pair::UNSTAGED);
    assert!(app.group().active_diff().is_some());
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    assert_eq!(
        app.file_history_view.path.as_deref(),
        Some(root.join("a.txt").as_path())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_drops_reads_of_another_file_or_an_older_generation() {
    let (_d, mut app) = app();
    app.type_text("x");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    idle(&mut app).await;
    let path = app.file_history_view.path.clone().unwrap();
    let gen_now = app.file_history_view.generation;
    // Another file's result and an older generation's are ignored.
    app.test_file_history_entries(path.with_file_name("zzz.txt"), gen_now, Ok(vec![]));
    assert_eq!(app.file_history_view.entries.len(), 1);
    app.test_file_history_entries(path.clone(), gen_now - 1, Ok(vec![]));
    assert_eq!(app.file_history_view.entries.len(), 1);
    // The current one is applied: an error shows instead of "No history".
    app.test_file_history_entries(path.clone(), gen_now, Err("boom".into()));
    assert_eq!(app.file_history_view.error.as_deref(), Some("boom"));
    assert!(list_rows(&mut app)[0].contains("boom"), "{:?}", list_rows(&mut app));
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_a_save_re_reads_and_the_selection_stays_on_its_entry() {
    let (_d, mut app) = app();
    app.type_text("x");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    draw(&mut app);
    let list = app.hits.file_history.unwrap().list;
    app.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 3, list.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), list.x + 3, list.y);
    let id = app.file_history_view.entries[0].id.clone();
    assert_eq!(app.file_history_view.selected_row(), Some(0));
    app.ui.focus = crate::app::Focus::Editor;
    std::thread::sleep(std::time::Duration::from_millis(2));
    app.type_text("yy");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 2).await);
    assert_eq!(app.file_history_view.selected_row(), Some(1));
    assert_eq!(app.file_history_view.entries[1].id, id);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_view_history_done_invalidates_a_read_in_flight() {
    let (_d, mut app) = app();
    app.type_text("x");
    save(&mut app).await;
    assert!(app.settle(|a| a.file_history_view.entries.len() == 1).await);
    idle(&mut app).await;
    let path = app.file_history_view.path.clone().unwrap();
    // A read is in flight (started before the new entry was written).
    app.file_history_view.loading = true;
    let old = app.file_history_view.generation;
    app.test_history_done();
    assert!(app.file_history_view.generation > old && app.file_history_view.stale);
    assert!(!app.file_history_view.loading);
    // Its late, outdated result is dropped.
    app.test_file_history_entries(path, old, Ok(vec![]));
    assert_eq!(app.file_history_view.entries.len(), 1);
    // A fresh read runs.
    idle(&mut app).await;
    assert_eq!(app.file_history_view.entries.len(), 1);
}
