//! Horizontal scrolling (hscroll-a, the wheel): in a text tab with long lines a sideways wheel
//! event and Shift+wheel scroll the tab under the pointer sideways without focusing it or
//! moving its cursor, stopping one column past the widest line drawn so far (which only grows,
//! so scrolling down past that line keeps it); the wheel over the vertical bar does the same;
//! the next cursor reveal scrolls back. Side-by-side and inline diff tabs scroll the same way
//! and no longer scroll past their lines.

use std::fs;

use crossterm::event::{KeyCode, KeyModifiers as M, MouseEventKind as Mouse};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::command::Command;
use crate::diff_view::Pair;
use crate::git::test_repo::{commit_all, init};
use crate::layout::GroupId;
use crate::milestone10_tests::{dirs, open_file_app};

/// The wheel's sideways step (`app::mouse::SIDE_STEP`, the diff view's).
const SIDE: usize = 4;

fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// 100 short lines, line 1 (0-based) `long` columns wide.
fn long_line_text(long: usize) -> String {
    (0..100)
        .map(|i| {
            if i == 1 {
                "x".repeat(long) + "\n"
            } else {
                format!("line {i}\n")
            }
        })
        .collect()
}

/// A cell in the middle of group `g`'s text.
fn text_cell(app: &App, g: GroupId) -> (u16, u16) {
    let t = app.hits.group(g).unwrap().text;
    (t.x + t.width / 2, t.y + t.height / 2)
}

fn view(app: &App, g: GroupId) -> &crate::view::View {
    &app.groups.get(g).unwrap().active_tab().unwrap().view
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_text_tab_wheel_scrolls_sideways_without_moving_the_cursor() {
    let (_d, root, state) = dirs();
    fs::write(root.join("a.txt"), long_line_text(200)).unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    // Two groups: the wheel scrolls the one under the pointer and leaves the focus alone.
    app.test_execute(Command::SplitRight);
    let other = app.groups.active;
    let g = app.groups.ids().into_iter().find(|&id| id != other).unwrap();
    draw(&mut app, 120, 24);
    let width = view(&app, g).width;
    assert!(width < 200, "the long line is wider than the view");
    let before = view(&app, g).selections.clone();
    let (x, y) = text_cell(&app, g);
    let side = SIDE;

    app.mouse(Mouse::ScrollRight, x, y);
    assert_eq!(view(&app, g).left_col, side, "a sideways wheel step");
    app.mouse_with(Mouse::ScrollDown, x, y, M::SHIFT);
    assert_eq!(
        view(&app, g).left_col,
        2 * side,
        "Shift+wheel down scrolls right (VS Code)"
    );
    assert_eq!(view(&app, g).top_line(), 0, "and not down");
    app.mouse_with(Mouse::ScrollUp, x, y, M::SHIFT);
    assert_eq!(view(&app, g).left_col, side, "Shift+wheel up scrolls left");
    app.mouse(Mouse::ScrollLeft, x, y);
    app.mouse(Mouse::ScrollLeft, x, y);
    assert_eq!(view(&app, g).left_col, 0, "never left of the first column");
    assert_eq!(app.groups.active, other, "the focus stays");
    assert_eq!(view(&app, other).left_col, 0, "the other group doesn't scroll");

    // The clamp: one blank column after the widest line's end.
    for _ in 0..100 {
        app.mouse(Mouse::ScrollRight, x, y);
        draw(&mut app, 120, 24);
    }
    let max = 201 - width;
    assert_eq!(view(&app, g).left_col, max, "stops at the widest line's end");
    // On screen: the long line's last `width - 1` columns, then one blank column.
    let term = draw(&mut app, 120, 24);
    let t = app.hits.group(g).unwrap().text;
    let row: String = (t.x..t.x + t.width)
        .map(|x| term.backend().buffer()[(x, t.y + 1)].symbol().to_string())
        .collect();
    assert_eq!(
        row.trim_end(),
        "x".repeat(width - 1),
        "the end of the long line in view: {row:?}"
    );
    assert_eq!(view(&app, g).selections, before, "the cursor unmoved");

    // The width only grows: scrolled down past the long line, it still scrolls as far.
    app.mouse(Mouse::ScrollDown, x, y);
    draw(&mut app, 120, 24);
    assert!(view(&app, g).top_line() > 1, "the long line is off screen");
    for _ in 0..100 {
        app.mouse(Mouse::ScrollLeft, x, y);
        draw(&mut app, 120, 24);
    }
    assert_eq!(view(&app, g).left_col, 0);
    for _ in 0..100 {
        app.mouse(Mouse::ScrollRight, x, y);
        draw(&mut app, 120, 24);
    }
    assert_eq!(view(&app, g).left_col, max, "the widest line drawn so far still counts");

    // The wheel over the vertical scroll bar scrolls sideways too.
    let bar = app.hits.group(g).unwrap().scrollbar.expect("a vertical bar").track;
    app.mouse(Mouse::ScrollLeft, bar.x, bar.y + 1);
    assert_eq!(view(&app, g).left_col, max - side, "over the bar");
    assert_eq!(app.groups.active, other);

    // The next cursor reveal scrolls back.
    app.test_execute(Command::FocusGroup(1));
    assert_eq!(app.groups.active, g);
    app.press(KeyCode::Right, M::NONE);
    draw(&mut app, 120, 24);
    assert_eq!(
        view(&app, g).left_col,
        1,
        "revealing the cursor (now in column 1) scrolls back to it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_text_tab_whose_lines_fit_does_not_scroll_sideways() {
    let (_d, root, state) = dirs();
    fs::write(root.join("a.txt"), "short\nlines\n").unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    draw(&mut app, 80, 24);
    let g = app.groups.active;
    let (x, y) = text_cell(&app, g);
    app.mouse(Mouse::ScrollRight, x, y);
    app.mouse_with(Mouse::ScrollDown, x, y, M::SHIFT);
    assert_eq!(view(&app, g).left_col, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_reload_from_disk_resets_the_scroll_width() {
    let (_d, root, state) = dirs();
    fs::write(root.join("a.txt"), long_line_text(200)).unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    draw(&mut app, 80, 24);
    let g = app.groups.active;
    let (x, y) = text_cell(&app, g);
    app.mouse(Mouse::ScrollRight, x, y);
    assert_eq!(view(&app, g).left_col, SIDE);
    // The file loses its long line on disk: the clean buffer reloads.
    fs::write(root.join("a.txt"), "short\n").unwrap();
    assert!(
        app.settle(|a| a.active_buffer().unwrap().rope == "short\n").await,
        "reloaded"
    );
    draw(&mut app, 80, 24);
    app.mouse(Mouse::ScrollLeft, x, y);
    app.mouse(Mouse::ScrollRight, x, y);
    assert_eq!(view(&app, g).left_col, 0, "only the new text's lines count");
}

/// A repository with `f.txt` committed, then changed on disk so one changed line (the second
/// of its change) is `long` columns wide; the unstaged diff open and loaded.
async fn diff_app(long: usize) -> (tempfile::TempDir, App) {
    let (d, root, state) = dirs();
    init(&root);
    let old: String = (0..40).map(|i| format!("line {i}\n")).collect();
    fs::write(root.join("f.txt"), &old).unwrap();
    commit_all(&root, "one");
    // The long line second in its change: the change's first row has its buttons over the end.
    let new = old.replacen("line 3\nline 4\n", &format!("changed\n{}\n", "y".repeat(long)), 1);
    fs::write(root.join("f.txt"), new).unwrap();
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(
        app.settle(|a| a.git.status.as_ref().is_some_and(|s| !s.entries.is_empty()))
            .await
    );
    app.open_diff(root.join("f.txt"), Pair::UNSTAGED);
    assert!(app.settle(|a| a.diffs_settled() && a.git.status.is_some()).await);
    (d, app)
}

/// Scrolls the active diff tab sideways with the wheel until it stops; returns where.
fn diff_wheel_to_end(app: &mut App, w: u16) -> usize {
    let rows = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap().rows;
    for _ in 0..100 {
        app.mouse(Mouse::ScrollRight, rows.x + 10, rows.y + 1);
        draw(app, w, 24);
    }
    app.group().active_diff().unwrap().left_col
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_side_by_side_diff_stops_at_its_widest_line() {
    let (_d, mut app) = diff_app(200).await;
    draw(&mut app, 150, 24);
    let d = app.group().active_diff().unwrap();
    assert!(!d.inline);
    let width = d.text_width;
    assert!(width > 0 && width < 200, "{width}");
    let cursor = d.cursor;
    assert_eq!(
        diff_wheel_to_end(&mut app, 150),
        201 - width,
        "no blank columns past the lines"
    );
    // On screen: the changed line's end in view on the right side, nothing past it.
    let term = draw(&mut app, 150, 24);
    let rows = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap().rows;
    let ys: usize = (rows.y..rows.y + rows.height)
        .flat_map(|y| (rows.x..rows.x + rows.width).map(move |x| (x, y)))
        .filter(|&p| term.backend().buffer()[p].symbol() == "y")
        .count();
    assert_eq!(ys, width - 1, "the long line's last columns drawn");
    // Shift+wheel too, and the cursor stays.
    let rows = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap().rows;
    app.mouse_with(Mouse::ScrollUp, rows.x + 10, rows.y + 1, M::SHIFT);
    let d = app.group().active_diff().unwrap();
    assert_eq!(d.left_col, 201 - width - SIDE);
    assert_eq!((d.cursor, d.top), (cursor, 0), "sideways only");
    // The keyboard's Right stops there too.
    for _ in 0..100 {
        app.press(KeyCode::Right, M::NONE);
    }
    assert_eq!(app.group().active_diff().unwrap().left_col, 201 - width);
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_inline_diff_stops_at_its_widest_line() {
    let (_d, mut app) = diff_app(150).await;
    draw(&mut app, 80, 24);
    let d = app.group().active_diff().unwrap();
    assert!(d.inline);
    let width = d.text_width;
    assert!(width > 0 && width < 150, "{width}");
    assert_eq!(diff_wheel_to_end(&mut app, 80), 151 - width);
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_diff_whose_lines_fit_does_not_scroll_sideways() {
    let (_d, mut app) = diff_app(20).await;
    draw(&mut app, 150, 24);
    assert_eq!(diff_wheel_to_end(&mut app, 150), 0, "every line fits");
}
