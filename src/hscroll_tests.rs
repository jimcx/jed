//! Horizontal scrolling (hscroll-a, the wheel): in a text tab with long lines a sideways wheel
//! event and Shift+wheel scroll the tab under the pointer sideways without focusing it or
//! moving its cursor, stopping one column past the widest line drawn so far (which only grows,
//! so scrolling down past that line keeps it); the wheel over the vertical bar does the same;
//! the next cursor reveal scrolls back. Side-by-side and inline diff tabs scroll the same way
//! and no longer scroll past their lines.
//!
//! hscroll-b, the bar: a one-row bar on the editor's last row appears once a line wider than the
//! view has been drawn (a second frame is asked for, so no row is lost before then), its thumb
//! follows the scroll, a click on the track and a drag scroll, hover and the corner cell look as
//! the vertical bar's; diff tabs get one bar spanning both halves; hex tabs none.

use std::fs;

use crossterm::event::{KeyCode, KeyModifiers as M, MouseEventKind as Mouse};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

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
    // The keyboard's Right moves the caret (diff_edit), scrolling only to keep it in view.
    for _ in 0..100 {
        app.press(KeyCode::Right, M::NONE);
    }
    let d = app.group().active_diff().unwrap();
    let col = d.caret.head.1;
    assert!(
        col > 0 && d.left_col <= col && col < d.left_col + width,
        "the caret's column is shown"
    );
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

/// One frame drawn on its own (no second one): whether it asked for another.
fn one_frame(app: &mut App, w: u16, h: u16) -> bool {
    app.test_take_redraw();
    draw(app, w, h);
    app.test_take_redraw()
}

/// A frame as `run` draws it (`App::redraw`: the second at once when the first asks for it,
/// the bar's row appearing or going); after it nothing more is asked for.
fn frame(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    app.needs_redraw = true;
    app.redraw(&mut term).unwrap();
    assert!(!app.test_take_redraw(), "settled: no further frame asked for");
    term
}

fn hbar(app: &App, g: GroupId) -> crate::layout::ScrollBarHit {
    app.hits
        .scrollbar(crate::layout::ScrollTarget::Columns(g))
        .expect("a horizontal bar")
}

/// The track's columns drawn in slider colour `key`, as (start, len), checked to be one run.
fn hthumb(app: &App, term: &Terminal<TestBackend>, g: GroupId, key: &str) -> Option<(u16, u16)> {
    let t = hbar(app, g).track;
    let colour = app.theme.get(key).bg.unwrap();
    let cols: Vec<u16> = (0..t.width)
        .filter(|&x| term.backend().buffer()[(t.x + x, t.y)].fg == colour)
        .collect();
    let first = *cols.first()?;
    assert_eq!(cols, (first..first + cols.len() as u16).collect::<Vec<_>>(), "one run");
    Some((first, cols.len() as u16))
}

async fn text_app(long: usize) -> (tempfile::TempDir, App) {
    let (d, root, state) = dirs();
    fs::write(root.join("a.txt"), long_line_text(long)).unwrap();
    (d, open_file_app(&root, &state, "a.txt"))
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_bar_is_absent_and_takes_no_row_while_lines_fit() {
    let (_d, mut app) = text_app(20).await;
    let g = app.groups.active;
    assert!(!one_frame(&mut app, 120, 24), "nothing to reserve: no second frame");
    frame(&mut app, 120, 24);
    assert!(app.hits.group(g).unwrap().hscrollbar.is_none());
    let editor = app.hits.group(g).unwrap().editor;
    assert_eq!(view(&app, g).height, editor.height as usize, "no row lost");
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_bar_appears_once_a_long_line_is_drawn() {
    let (_d, mut app) = text_app(200).await;
    let g = app.groups.active;
    // The first frame measures the long line and asks for the next, which takes the row.
    assert!(one_frame(&mut app, 120, 24), "the bar's row appearing asks for a frame");
    let full = view(&app, g).height;
    assert!(app.hits.group(g).unwrap().hscrollbar.is_none());
    let term = frame(&mut app, 120, 24);
    let hit = app.hits.group(g).unwrap().clone();
    assert_eq!(view(&app, g).height, full - 1, "the row is taken from the text");
    let bar = hbar(&app, g);
    assert_eq!(
        (bar.track.y, bar.track.x, bar.track.width),
        (hit.editor.y + hit.editor.height, hit.text.x, hit.text.width)
    );
    assert_eq!(bar.track.y, hit.text.y + hit.text.height, "the editor's last row");
    // The corner under the vertical bar (and the gutter and marks cells) are blank editor
    // background.
    let bg = app.theme.get("editor").bg;
    let v = hit.scrollbar.unwrap().track;
    let m = hit.marks.expect("a text tab's marks column");
    assert_eq!(term.backend().buffer()[(v.x, bar.track.y)].bg, bg.unwrap());
    assert_eq!(
        term.backend().buffer()[(m.x, bar.track.y)].bg,
        bg.unwrap(),
        "the marks column's corner is blank too"
    );
    assert_eq!(term.backend().buffer()[(hit.editor.x, bar.track.y)].bg, bg.unwrap());
    assert_eq!(v.y + v.height, bar.track.y, "the vertical bar stops above the corner");
    // Thumb at the left; its length is the view's share of the 201 columns.
    let (start, len) = hthumb(&app, &term, g, "scrollbarSlider.background").unwrap();
    assert_eq!(start, 0);
    let w = hit.text.width as u32;
    assert_eq!(u32::from(len), (w * w + 100) / 201);
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_bar_waits_for_the_long_line_to_be_drawn() {
    let (_d, root, state) = dirs();
    // The long line is far below the first screen.
    let text: String = (0..100)
        .map(|i| {
            if i == 60 {
                "x".repeat(300) + "\n"
            } else {
                format!("l{i}\n")
            }
        })
        .collect();
    fs::write(root.join("a.txt"), text).unwrap();
    let mut app = open_file_app(&root, &state, "a.txt");
    let g = app.groups.active;
    frame(&mut app, 120, 24);
    assert!(app.hits.group(g).unwrap().hscrollbar.is_none(), "not drawn yet: no bar");
    let (x, y) = text_cell(&app, g);
    for _ in 0..20 {
        app.mouse(Mouse::ScrollDown, x, y);
    }
    assert!(view(&app, g).top_line() >= 55);
    frame(&mut app, 120, 24);
    assert!(
        app.hits.group(g).unwrap().hscrollbar.is_some(),
        "drawn once: the bar stays"
    );
    for _ in 0..40 {
        app.mouse(Mouse::ScrollUp, x, y);
    }
    assert!(!one_frame(&mut app, 120, 24), "the bar stays: no frame asked for");
    assert!(
        app.hits.group(g).unwrap().hscrollbar.is_some(),
        "and doesn't go as the long line scrolls off"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_thumb_follows_click_and_drag() {
    let (_d, mut app) = text_app(400).await;
    let g = app.groups.active;
    let mut term = frame(&mut app, 120, 24);
    let normal = "scrollbarSlider.background";
    let bar = hbar(&app, g).track;
    let (s0, len) = hthumb(&app, &term, g, normal).unwrap();
    // The wheel moves the thumb, and the cursor stays.
    let before = view(&app, g).selections.clone();
    let (x, y) = text_cell(&app, g);
    for _ in 0..10 {
        app.mouse(Mouse::ScrollRight, x, y);
    }
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let (s1, len1) = hthumb(&app, &term, g, normal).unwrap();
    assert!(s1 > s0 && len1 == len, "{s0} -> {s1}");
    assert_eq!(view(&app, g).left_col, 10 * SIDE);
    assert_eq!(view(&app, g).selections, before);
    // The wheel over the bar scrolls sideways too.
    app.mouse(Mouse::ScrollLeft, bar.x + 3, bar.y);
    assert_eq!(view(&app, g).left_col, 9 * SIDE);
    // A click on the track far to the right centres the thumb there and grabs it.
    let target = bar.x + bar.width * 3 / 4;
    app.mouse(Mouse::Down(crossterm::event::MouseButton::Left), target, bar.y);
    let left = view(&app, g).left_col;
    assert!(left > 9 * SIDE, "{left}");
    assert_eq!(app.groups.active, g);
    assert_eq!(view(&app, g).selections, before, "no cursor moved");
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let (s, l) = hthumb(&app, &term, g, "scrollbarSlider.activeBackground").expect("active while grabbed");
    assert!(
        s + l / 2 >= target - bar.x - 1 && s + l / 2 <= target - bar.x + 1,
        "centred on the pointer"
    );
    // Dragging back left scrolls back; dragging past the end stops at the widest line's end.
    app.mouse(Mouse::Drag(crossterm::event::MouseButton::Left), bar.x, bar.y + 5);
    assert_eq!(view(&app, g).left_col, 0);
    app.mouse(
        Mouse::Drag(crossterm::event::MouseButton::Left),
        bar.x + bar.width + 30,
        bar.y,
    );
    let width = view(&app, g).width;
    assert_eq!(view(&app, g).left_col, 401 - width, "the widest line's end");
    app.mouse(Mouse::Up(crossterm::event::MouseButton::Left), bar.x, bar.y);
    // Hover over the thumb.
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let (s, _) = hthumb(&app, &term, g, normal).unwrap();
    app.mouse(Mouse::Moved, bar.x + s, bar.y);
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(hthumb(&app, &term, g, "scrollbarSlider.hoverBackground").is_some());
    app.mouse(Mouse::Moved, bar.x, bar.y.saturating_sub(3));
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    assert!(hthumb(&app, &term, g, "scrollbarSlider.hoverBackground").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_side_by_side_diff_has_one_bar_across_both_halves() {
    let (_d, mut app) = diff_app(200).await;
    let g = app.groups.active;
    assert!(
        one_frame(&mut app, 150, 24),
        "the first frame measures the diff and asks for another"
    );
    let term = frame(&mut app, 150, 24);
    let d = app.group().active_diff().unwrap();
    assert!(!d.inline);
    let hit = app.hits.group(g).unwrap().clone();
    let bar = hbar(&app, g);
    assert_eq!(
        bar.track.y,
        hit.diff.as_ref().unwrap().rows.y + hit.diff.as_ref().unwrap().rows.height
    );
    assert_eq!(bar.track.x, hit.editor.x);
    assert_eq!(bar.track.width, hit.editor.width, "both halves");
    let (start, len) = hthumb(&app, &term, g, "scrollbarSlider.background").unwrap();
    assert_eq!(start, 0);
    assert!(len < bar.track.width);
    // Wheel and drag scroll the shared left_col; the diff stops at its widest line.
    let rows = hit.diff.unwrap().rows;
    app.mouse(Mouse::ScrollRight, rows.x + 10, rows.y + 1);
    assert_eq!(app.group().active_diff().unwrap().left_col, SIDE);
    app.mouse(
        Mouse::Down(crossterm::event::MouseButton::Left),
        bar.track.x + bar.track.width - 1,
        bar.track.y,
    );
    let d = app.group().active_diff().unwrap();
    assert!(d.left_col > SIDE && d.left_col <= 201 - d.text_width, "{}", d.left_col);
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_inline_diff_has_a_bar_and_fitting_diffs_none() {
    let (_d, mut app) = diff_app(150).await;
    let g = app.groups.active;
    let term = frame(&mut app, 80, 24);
    assert!(app.group().active_diff().unwrap().inline);
    let bar = hbar(&app, g);
    assert!(hthumb(&app, &term, g, "scrollbarSlider.background").is_some());
    app.mouse(
        Mouse::Down(crossterm::event::MouseButton::Left),
        bar.track.x + bar.track.width - 1,
        bar.track.y,
    );
    let d = app.group().active_diff().unwrap();
    assert!(d.left_col > 0 && d.left_col <= 151 - d.text_width);

    let (_d2, mut fit) = diff_app(20).await;
    assert!(!one_frame(&mut fit, 150, 24));
    frame(&mut fit, 150, 24);
    let g = fit.groups.active;
    assert!(fit.hits.group(g).unwrap().hscrollbar.is_none());
}

#[test]
fn hscroll_diff_edits_to_the_live_buffer_keep_the_scroll_width() {
    let mut t = crate::diff_view::DiffTab::new(1, std::path::PathBuf::from("x"), Pair::UNSTAGED);
    t.scroll_width.measure(300);
    let g = t.begin(Some((1, 1)));
    t.finish(g, Ok(Default::default()));
    assert_eq!(t.scroll_width.widest(), 0, "a first load resets");
    t.scroll_width.measure(300);
    let g = t.begin(Some((1, 2)));
    t.finish(g, Ok(Default::default()));
    assert_eq!(t.scroll_width.widest(), 300, "an edit of the live buffer keeps it");
    // The file or index changed (a reload from disk, a stage): read afresh, measured afresh.
    t.mark_stale();
    let g = t.begin(Some((1, 3)));
    t.finish(g, Ok(Default::default()));
    assert_eq!(t.scroll_width.widest(), 0, "a stale load resets");
    // Another buffer of the file (closed and opened again), or none open.
    t.scroll_width.measure(300);
    let g = t.begin(Some((2, 1)));
    t.finish(g, Ok(Default::default()));
    assert_eq!(t.scroll_width.widest(), 0, "a new buffer resets");
    t.scroll_width.measure(300);
    let g = t.begin(None);
    t.finish(g, Ok(Default::default()));
    assert_eq!(t.scroll_width.widest(), 0, "the buffer closed resets");
    t.scroll_width.measure(300);
    let g = t.begin(Some((2, 2)));
    t.finish(g, Ok(Default::default()));
    assert_eq!(t.scroll_width.widest(), 0, "opened again resets");
    t.scroll_width.measure(300);
    let g = t.begin(Some((2, 3)));
    t.finish(g, Ok(Default::default()));
    assert_eq!(t.scroll_width.widest(), 300, "and its edits keep it");
    // A diff without a live side (staged) resets on every load.
    let mut s = crate::diff_view::DiffTab::new(2, std::path::PathBuf::from("x"), Pair::STAGED);
    for rev in 1..3 {
        s.scroll_width.measure(300);
        let g = s.begin(Some((1, rev)));
        s.finish(g, Ok(Default::default()));
        assert_eq!(s.scroll_width.widest(), 0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_hex_tab_has_no_bar() {
    let (_d, mut app) = text_app(20).await;
    let path = app.buffers.iter().next().unwrap().1.path.with_file_name("b.bin");
    app.open_initial_hex(path, crate::hex::HexDoc::new(vec![7; 4000]));
    draw(&mut app, 60, 24);
    draw(&mut app, 60, 24);
    let g = app.groups.active;
    assert!(app.hits.group(g).unwrap().hscrollbar.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_bar_settles_at_every_size() {
    // `run`'s one frame shows the bar at once, and no size (an editor too short for the row
    // included) or resize keeps asking for frames.
    let (_d, mut app) = text_app(200).await;
    let g = app.groups.active;
    frame(&mut app, 120, 24);
    assert!(
        app.hits.group(g).unwrap().hscrollbar.is_some(),
        "shown in the first frame `run` draws"
    );
    for h in [4, 5, 6, 8, 24] {
        frame(&mut app, 120, h);
        frame(&mut app, 120, h);
    }
    // Wide enough for every line: the bar goes (in the same `run` frame) and the row comes back.
    frame(&mut app, 250, 24);
    assert!(app.hits.group(g).unwrap().hscrollbar.is_none());
    assert_eq!(view(&app, g).height, app.hits.group(g).unwrap().editor.height as usize);
}

/// A diff of `f.txt` whose change's first row is a 200-column line ending in `Z` (so its
/// buttons sit over that line's end), shown `w` columns wide as `pair` (staged: the change is
/// in the index), wheeled to its right end; the cells of the drawn `Z`, Stage / Unstage and
/// Revert buttons.
async fn diff_end_under_buttons(pair: Pair, w: u16) -> (Vec<(u16, u16)>, Vec<Rect>, Vec<Rect>) {
    let (_d, root, state) = dirs();
    init(&root);
    let old: String = (0..40).map(|i| format!("line {i}\n")).collect();
    fs::write(root.join("f.txt"), &old).unwrap();
    commit_all(&root, "one");
    // An added line: its change's first row inline as well as side by side.
    let new = old.replacen("line 3\n", &format!("line 3\n{}Z\n", "y".repeat(199)), 1);
    fs::write(root.join("f.txt"), new).unwrap();
    if pair == Pair::STAGED {
        crate::git::test_repo::git(&root, &["add", "f.txt"]);
    }
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(
        app.settle(|a| a.git.status.as_ref().is_some_and(|s| !s.entries.is_empty()))
            .await
    );
    app.open_diff(root.join("f.txt"), pair);
    assert!(app.settle(|a| a.diffs_settled() && a.git.status.is_some()).await);
    frame(&mut app, w, 24);
    diff_wheel_to_end(&mut app, w);
    let term = frame(&mut app, w, 24);
    let hit = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap();
    let rows = hit.rows;
    let z: Vec<(u16, u16)> = (rows.y..rows.y + rows.height)
        .flat_map(|y| (rows.x..rows.x + rows.width).map(move |x| (x, y)))
        .filter(|&p| term.backend().buffer()[p].symbol() == "Z")
        .collect();
    let rects = |v: &[(Rect, usize)]| v.iter().map(|(r, _)| *r).collect::<Vec<_>>();
    (z, rects(&hit.buttons), rects(&hit.reverts))
}

#[tokio::test(flavor = "multi_thread")]
async fn hscroll_diff_scrolls_the_line_under_its_buttons_into_view() {
    // Side by side staged: Unstage over the right side's end, no Revert.
    // Side by side unstaged: Stage there, Revert over the left side's (empty) end.
    // Inline unstaged: Revert left of Stage, both over the line's end.
    for (pair, w, reverts) in [
        (Pair::STAGED, 150, false),
        (Pair::UNSTAGED, 150, true),
        (Pair::UNSTAGED, 80, true),
    ] {
        let case = format!("{pair:?} at {w}");
        let (z, buttons, revert) = diff_end_under_buttons(pair, w).await;
        assert!(!buttons.is_empty(), "{case}: the change's button is drawn");
        assert_eq!(!revert.is_empty(), reverts, "{case}: Revert drawn");
        assert_eq!(z.len(), 1, "{case}: the line's last character is drawn");
        let p = ratatui::layout::Position::from(z[0]);
        assert!(
            buttons.iter().chain(&revert).all(|r| !r.contains(p)),
            "{case}: and not under a button"
        );
    }
}
