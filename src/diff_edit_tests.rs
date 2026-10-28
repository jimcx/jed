//! The diff_edit task, subtask diff_edit-a, end to end (a git repo's unstaged diff, through the
//! keys, the mouse and the drawn frame). Done when: a diff tab has a caret and a text selection in
//! one side's own text (a line and a byte into that side), starting on the cursor's row; the
//! editor's movement keys move it and Shift extends, Up / Down keep their column, a click sets
//! the side and the caret, a drag / Shift+click extends inside the side (a drag past the divider
//! stops at it), a double click selects a word and a triple click the line; the selection is drawn
//! in `editor.selection` over the diff backgrounds with the terminal cursor on the caret; Cmd+C
//! copies from either side, Cmd+X only on an editable side (and only copies until diff_edit-b),
//! Cmd+A selects the active side, Esc clears; inline, the caret follows the side a row draws;
//! next / previous change keeps the caret on the cursor's row; the buffer is never touched.

use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::command::{Command, Motion};
use crate::diff_view::{DiffTab, Pair};
use crate::milestone10_tests::{bg, dirs, draw_sized, open_file_app, repo_with, run_command, settle_diffs};

const ORIGINAL: &str = "alpha one\nbeta two\ngamma three\n";
const CHANGED: &str = "alpha one\nbeta TWO\ngamma three\ndelta four\n";

/// An app with the unstaged diff of `f.txt` open (the cursor on the changed line `beta`).
async fn open(root: &Path, state: &Path) -> App {
    open_with(root, state, ORIGINAL, CHANGED).await
}

/// An app with the unstaged diff of `f.txt` (`original` committed, `changed` in the work tree).
async fn open_with(root: &Path, state: &Path, original: &str, changed: &str) -> App {
    repo_with(root, &[("f.txt", original)], &[("f.txt", changed)]);
    let mut app = open_file_app(root, state, "f.txt");
    assert!(crate::milestone10_tests::settle_markers(&mut app).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    app
}

fn diff(app: &App) -> &DiffTab {
    app.group().active_diff().expect("a diff tab")
}

fn mv(app: &mut App, motion: Motion, select: bool) {
    app.test_execute(Command::Move { motion, select });
}

fn copied(app: &mut App) -> String {
    app.test_execute(Command::Copy);
    app.clipboard.read().unwrap()
}

/// The screen cell of byte `byte` of the caret's row text on side `right` (ASCII text).
fn cell(app: &App, right: bool, col: usize) -> (u16, u16) {
    let g = app.groups.active;
    let h = app.hits.group(g).unwrap().diff.clone().unwrap();
    let d = diff(app);
    (
        h.text_x[right as usize] + col as u16,
        h.rows.y + (d.cursor - d.top) as u16,
    )
}

fn click(app: &mut App, at: (u16, u16)) {
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.0, at.1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), at.0, at.1);
}

fn term(app: &mut App, w: u16) -> Terminal<TestBackend> {
    draw_sized(app, w, 24)
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_edit_caret_starts_on_the_cursor_row_and_moves_by_keys() {
    let (_d, root, state) = dirs();
    let mut app = open(&root, &state).await;
    term(&mut app, 150);
    let d = diff(&app);
    // The first change is `beta` (right line 1); the caret is there, at its start.
    assert_eq!((d.caret.right, d.caret.head, d.caret.anchor), (true, (1, 0), (1, 0)));
    assert_eq!(d.rows()[d.cursor].right, Some(1));
    app.press(KeyCode::Right, M::NONE);
    app.press(KeyCode::Right, M::NONE);
    assert_eq!(diff(&app).caret.head, (1, 2));
    // Shift+End selects to the line's end, the text of the right side.
    app.press(KeyCode::End, M::SHIFT);
    assert_eq!((diff(&app).caret.anchor, diff(&app).caret.head), ((1, 2), (1, 8)));
    assert_eq!(copied(&mut app), "ta TWO");
    // Down keeps the column (the goal 8 clamps to `gamma three`'s 8), Up returns.
    app.press(KeyCode::Down, M::NONE);
    assert_eq!(diff(&app).caret.head, (2, 8));
    assert_eq!(diff(&app).caret.anchor, (2, 8), "a plain move collapses");
    app.press(KeyCode::Up, M::NONE);
    assert_eq!(diff(&app).caret.head, (1, 8));
    // Right at the end of a line goes to the next line's start; Left back.
    app.press(KeyCode::Right, M::NONE);
    assert_eq!(diff(&app).caret.head, (2, 0));
    app.press(KeyCode::Left, M::NONE);
    assert_eq!(diff(&app).caret.head, (1, 8));
    // Doc end / start.
    mv(&mut app, Motion::DocEnd, false);
    assert_eq!(diff(&app).caret.head, (3, 10));
    mv(&mut app, Motion::DocStart, true);
    assert_eq!(diff(&app).caret.head, (0, 0));
    assert_eq!(diff(&app).caret.anchor, (3, 10));
    // Next change moves the row cursor: the caret follows it, the selection dropped.
    app.test_execute(Command::NextChange);
    term(&mut app, 150);
    let d = diff(&app);
    assert_eq!(d.caret.anchor, d.caret.head);
    assert_eq!(d.rows()[d.cursor].right, Some(d.caret.head.0));
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_edit_word_moves_and_select_all_and_escape() {
    let (_d, root, state) = dirs();
    let mut app = open(&root, &state).await;
    term(&mut app, 150);
    mv(&mut app, Motion::WordRight, true);
    assert_eq!(copied(&mut app), "beta");
    mv(&mut app, Motion::WordRight, true);
    assert_eq!(copied(&mut app), "beta TWO");
    app.press(KeyCode::Esc, M::NONE);
    assert_eq!(diff(&app).caret.anchor, diff(&app).caret.head);
    app.test_execute(Command::SelectAll);
    assert_eq!(copied(&mut app), CHANGED.trim_end());
    assert!(diff(&app).caret.right);
    // Nothing reached the file's buffer.
    let (_, buf) = app.buffers.iter().find(|(_, b)| b.has_file()).unwrap();
    assert_eq!(buf.rope.to_string(), CHANGED);
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_edit_click_drag_and_multi_click_in_either_side() {
    let (_d, root, state) = dirs();
    let mut app = open(&root, &state).await;
    term(&mut app, 150);
    // A click in the left (index) side's text: that side is active, in its own lines.
    let at = cell(&app, false, 5);
    click(&mut app, at);
    let d = diff(&app);
    assert_eq!((d.caret.right, d.caret.head), (false, (1, 5)));
    // Double click: the word (`two` is the left side's), triple: the line.
    let at = cell(&app, false, 6);
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.0, at.1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), at.0, at.1);
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.0, at.1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), at.0, at.1);
    assert_eq!(copied(&mut app), "two");
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.0, at.1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), at.0, at.1);
    assert_eq!(copied(&mut app), "beta two");
    // A drag from the left side across the divider stays in the left side, to its line end.
    let from = cell(&app, false, 2);
    app.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    let to = cell(&app, true, 3);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1 + 1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1 + 1);
    let d = diff(&app);
    assert_eq!((d.caret.right, d.caret.anchor, d.caret.head), (false, (1, 2), (2, 11)));
    assert_eq!(copied(&mut app), "ta two\ngamma three");
    // Cut on a read-only side copies nothing new.
    app.clipboard.write(crate::clipboard::Copied {
        text: "kept".into(),
        whole_line: false,
        pieces: Vec::new(),
    });
    app.test_execute(Command::Cut);
    assert_eq!(app.clipboard.read().unwrap(), "kept");
    // Shift+click in the right side extends from the caret... in the caret's own side.
    let at = cell(&app, false, 1);
    click(&mut app, at);
    let to = cell(&app, true, 4);
    app.mouse_with(MouseEventKind::Down(MouseButton::Left), to.0, to.1, M::SHIFT);
    app.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1);
    let d = diff(&app);
    assert_eq!((d.caret.right, d.caret.anchor, d.caret.head), (false, (2, 1), (2, 4)));
    assert_eq!(
        copied(&mut app),
        "amm",
        "extended inside the caret's side (the left one), on the cursor's row"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_edit_selection_and_cursor_are_drawn() {
    let (_d, root, state) = dirs();
    let mut app = open(&root, &state).await;
    term(&mut app, 150);
    app.press(KeyCode::Right, M::NONE);
    mv(&mut app, Motion::WordRight, true);
    let mut t = term(&mut app, 150);
    let sel = app.theme.get("editor.selection").bg;
    assert!(sel.is_some());
    // `eta` (bytes 1..4 of the right side's `beta TWO`) is selected, the rest keeps the diff colour.
    let inside = cell(&app, true, 2);
    let outside = cell(&app, true, 6);
    assert_eq!(bg(&t, inside), sel);
    assert_ne!(bg(&t, outside), sel);
    assert_ne!(bg(&t, cell(&app, false, 2)), sel, "the other side has no selection");
    // The terminal cursor is at the caret.
    let pos = t.get_cursor_position().unwrap();
    assert_eq!((pos.x, pos.y), cell(&app, true, 4));
    // A selection over a line break paints one cell past the line.
    app.test_execute(Command::SelectAll);
    t = term(&mut app, 150);
    let h = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap();
    let row0 = h.rows.y; // `alpha one` (9 bytes): its break is selected
    assert_eq!(bg(&t, (h.text_x[1] + 9, row0)), sel);
    assert_eq!(bg(&t, (h.text_x[1], row0)), sel);
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_edit_inline_caret_follows_the_side_a_row_draws() {
    let (_d, root, state) = dirs();
    let mut app = open(&root, &state).await;
    term(&mut app, 70);
    assert!(diff(&app).inline);
    // Inline, `beta two` (removed) comes before `beta TWO`: the cursor is on one of them.
    let d = diff(&app);
    let drawn = d.rows()[d.cursor];
    assert_eq!(d.caret.right, drawn.right.is_some());
    // Up from the first change moves to a context row (right side), Down goes through the
    // removed row (left side) to the added one.
    mv(&mut app, Motion::Down, false);
    mv(&mut app, Motion::Down, false);
    let d = diff(&app);
    let row = d.rows()[d.cursor];
    assert_eq!(d.caret.right, row.right.is_some());
    assert_eq!(d.caret.head.0, row.right.or(row.left).unwrap());
    let sides: Vec<bool> = (0..4)
        .map(|_| {
            mv(&mut app, Motion::Up, false);
            diff(&app).caret.right
        })
        .collect();
    assert!(sides.contains(&false) && sides.contains(&true), "{sides:?}");
    // A clicked row's text is the one selected: the removed line is read from the left side.
    term(&mut app, 70);
    let h = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap();
    let y = h.rows.y
        + diff(&app)
            .rows()
            .iter()
            .position(|r| r.left == Some(1) && r.right.is_none())
            .unwrap() as u16;
    click(&mut app, (h.text_x[0] + 2, y));
    assert!(!diff(&app).caret.right);
    app.test_execute(Command::SelectAll);
    assert_eq!(copied(&mut app), ORIGINAL.trim_end());
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_edit_only_the_current_text_is_editable() {
    let (_d, root, state) = dirs();
    let mut app = open(&root, &state).await;
    term(&mut app, 150);
    assert_eq!(diff(&app).editable_sides(), [false, true]);
    // Cut on the editable side copies the selection (the removal is diff_edit-b's), the
    // buffer is untouched either way.
    app.test_execute(Command::SelectAll);
    app.test_execute(Command::Cut);
    assert_eq!(app.clipboard.read().unwrap(), CHANGED.trim_end());
    let (_, buf) = app.buffers.iter().find(|(_, b)| b.has_file()).unwrap();
    assert_eq!(buf.rope.to_string(), CHANGED);
    let path = diff(&app).path.clone();
    app.open_diff(path, Pair::STAGED);
    assert!(settle_diffs(&mut app).await);
    assert_eq!(diff(&app).editable_sides(), [false, false]);
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_edit_left_side_cursor_empty_copy_and_drag_past_the_divider() {
    let (_d, root, state) = dirs();
    let long = format!("{}\n", "abcdefghij".repeat(20));
    let changed = format!("{}\n", "ABCDEFGHIJ".repeat(20));
    let mut app = open_with(&root, &state, &long, &changed).await;
    term(&mut app, 150);
    assert!(!diff(&app).inline);
    // A click on the left side: the terminal cursor is drawn there, not on the right.
    let at = cell(&app, false, 3);
    assert_ne!(at, cell(&app, true, 3));
    click(&mut app, at);
    let mut t = term(&mut app, 150);
    let pos = t.get_cursor_position().unwrap();
    assert_eq!((pos.x, pos.y), cell(&app, false, 3));
    // Copy with nothing selected leaves the clipboard alone.
    app.clipboard.write(crate::clipboard::Copied {
        text: "kept".into(),
        whole_line: false,
        pieces: Vec::new(),
    });
    assert_eq!(copied(&mut app), "kept");
    // A drag from the left side across the divider to the screen's right edge stops at the
    // divider: the selection ends at the left side's last shown column, nothing scrolls.
    let (width, left_col) = (diff(&app).text_width, diff(&app).left_col);
    assert_eq!(left_col, 0);
    assert!(width < 200, "the line is wider than the side");
    let divider = app
        .hits
        .group(app.groups.active)
        .unwrap()
        .diff
        .clone()
        .unwrap()
        .divider
        .unwrap();
    // (From the next cell: the same one would be a double click.)
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.0 + 1, at.1);
    for x in [at.0 + 10, divider + 5, 149] {
        app.mouse(MouseEventKind::Drag(MouseButton::Left), x, at.1);
    }
    app.mouse(MouseEventKind::Up(MouseButton::Left), 149, at.1);
    let d = diff(&app);
    assert!(!d.caret.right);
    assert_eq!(d.left_col, 0, "no sideways scroll");
    assert!(
        d.caret.head.1 <= width && d.caret.head.1 + 2 >= width,
        "the head {:?} stops at the divider (width {width})",
        d.caret.head
    );
    let text = copied(&mut app);
    assert!(text.starts_with("efghij") && !text.contains('A'), "{text}");
}
