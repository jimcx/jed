//! File History section (history_view-a): the Explorer body holds the tree, then "FILE HISTORY",
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
