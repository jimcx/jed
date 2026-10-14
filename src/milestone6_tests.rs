//! Milestone 6 end to end (6b): Cmd+D, Cmd+Shift+L, Cmd+Alt+D, Cmd+U and Esc through the real
//! keymap, the drawn selections and status bar, two groups on one buffer, and the "done when":
//! Cmd+Shift+L on a 1 MB file with tens of thousands of matches, then typing, stays smooth, and
//! one undo restores the selection list from before the typing.
//!
//! Timings are asserted in release builds only: `cargo test --release milestone -- --nocapture`

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode as K, KeyModifiers as M};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::buffer::pos::ByteIdx;
use crate::milestone_tests::test_app;
use crate::selection::{Selection, Selections};

/// Cmd+Shift+L on the 1 MB file (~3 ms on an M-series Mac).
const SELECT_ALL_BUDGET: Duration = Duration::from_millis(50);
/// One typed character at every selection, with the other group following, plus a frame:
/// within one 60 Hz frame (~5 ms measured).
const KEYSTROKE_BUDGET: Duration = Duration::from_millis(16);
/// Undoing the five keystrokes (~14 ms measured).
const UNDO_BUDGET: Duration = Duration::from_millis(50);
/// Any build, also debug (~60 ms a key, ~120 ms undo): far above that, but an O(selections²)
/// step at 63k selections takes seconds even in release. Keystrokes must also stay under the
/// 1 s undo grouping pause for the one-undo check to hold.
const DEBUG_BOUND: Duration = Duration::from_millis(800);

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn app_with(dir: &tempfile::TempDir, text: &str, cursor: usize) -> App {
    let mut app = test_app(dir.path(), Buffer::from_str(text));
    set_cursor(&mut app, cursor);
    app
}

/// Places the cursor as if the file had opened there (no Cmd+U entry).
fn set_cursor(app: &mut App, at: usize) {
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::single(Selection::cursor(ByteIdx(at)));
    view.cursor_history.reset(&view.selections);
}

fn add_next(app: &mut App) {
    app.press(K::Char('d'), cmd());
}

fn select_all(app: &mut App) {
    app.press(K::Char('l'), cmd() | M::SHIFT);
}

fn skip(app: &mut App) {
    app.press(K::Char('d'), cmd() | M::ALT);
}

fn cursor_undo(app: &mut App) {
    app.press(K::Char('u'), cmd());
}

fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

fn ranges_of(sels: &Selections) -> Vec<(usize, usize)> {
    sels.iter().map(|s| (s.start().0, s.end().0)).collect()
}

fn ranges(app: &App) -> Vec<(usize, usize)> {
    ranges_of(&app.group().active_tab().unwrap().view.selections)
}

fn sels(app: &App) -> Selections {
    app.group().active_tab().unwrap().view.selections.clone()
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

#[test]
fn milestone_6_cmd_d_word_mode_then_edit_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    // Whole words, case-sensitive: not Foo, foobar or foo_x.
    let mut app = app_with(&dir, "foo Foo foobar foo_x\nfoo.foo", 1);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3)], "first Cmd+D selects the word");
    add_next(&mut app);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (21, 24), (25, 28)]);
    add_next(&mut app);
    assert_eq!(
        ranges(&app),
        vec![(0, 3), (21, 24), (25, 28)],
        "all selected: nothing to add"
    );

    let before = sels(&app);
    app.type_text("bar");
    assert_eq!(text(&app), "bar Foo foobar foo_x\nbar.bar");
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "foo Foo foobar foo_x\nfoo.foo");
    assert_eq!(
        sels(&app),
        before,
        "undo restores the selections from before the typing"
    );
}

#[test]
fn milestone_6_selected_text_and_session_reset() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "foo foobar Foo foo", 0);
    // Word mode from an empty cursor: the next whole-word, same-case foo.
    add_next(&mut app);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (15, 18)]);
    // Esc changes the selections: the session ends. The next Cmd+D searches for the selected
    // text instead: any case, inside words (VS Code with the find widget's options off).
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app), vec![(0, 3)]);
    add_next(&mut app);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (4, 7), (11, 14)]);

    // Selecting by hand starts in selection mode too.
    set_cursor(&mut app, 11);
    for _ in 0..3 {
        app.press(K::Right, M::SHIFT);
    }
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(11, 14), (15, 18)]);
}

#[test]
fn milestone_6_touching_and_multi_byte_matches() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "aaaa", 0);
    app.press(K::Right, M::SHIFT);
    for _ in 0..3 {
        add_next(&mut app);
    }
    assert_eq!(
        ranges(&app),
        vec![(0, 1), (1, 2), (2, 3), (3, 4)],
        "touching stay separate"
    );
    app.type_text("b");
    assert_eq!(text(&app), "bbbb");

    let mut app = app_with(&dir, "héllo wörld héllo héllos", 3);
    add_next(&mut app);
    add_next(&mut app);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 6), (14, 20)]);
    select_all(&mut app);
    assert_eq!(ranges(&app), vec![(0, 6), (14, 20)]);
}

#[test]
fn milestone_6_skip_and_cursor_undo() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "ab ab ab ab", 0);
    add_next(&mut app);
    add_next(&mut app);
    skip(&mut app);
    assert_eq!(ranges(&app), vec![(0, 2), (6, 8)], "the second ab was skipped");
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 2), (6, 8), (9, 11)]);
    add_next(&mut app);
    assert_eq!(
        ranges(&app),
        vec![(0, 2), (3, 5), (6, 8), (9, 11)],
        "wraps to the skipped one"
    );

    // Cmd+U walks back through every selection change: 4, 3, skip, 2, word, cursor.
    let expected = [
        vec![(0, 2), (6, 8), (9, 11)],
        vec![(0, 2), (6, 8)],
        vec![(0, 2), (3, 5)],
        vec![(0, 2)],
        vec![(0, 0)],
    ];
    for want in expected {
        cursor_undo(&mut app);
        assert_eq!(ranges(&app), want);
    }
    cursor_undo(&mut app);
    assert_eq!(ranges(&app), vec![(0, 0)], "nothing left to undo");

    // Cmd+U after Cmd+Shift+L: back to the one cursor.
    app.press(K::Right, M::NONE);
    select_all(&mut app);
    assert_eq!(ranges(&app).len(), 4);
    assert_eq!(app.group().active_tab().unwrap().view.selections.primary().start().0, 0);
    cursor_undo(&mut app);
    assert_eq!(ranges(&app), vec![(1, 1)]);
    cursor_undo(&mut app);
    assert_eq!(ranges(&app), vec![(0, 0)], "and the move before it");

    // Typing clears the cursor history (it can't restore positions in other text).
    select_all(&mut app);
    app.type_text("x");
    let after = ranges(&app);
    cursor_undo(&mut app);
    assert_eq!(ranges(&app), after);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_6_esc_in_editor_and_palette() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "ab ab ab", 0);
    select_all(&mut app);
    assert_eq!(ranges(&app).len(), 3);
    // Esc in the palette only closes it.
    app.press(K::Char('p'), cmd() | M::SHIFT);
    assert!(app.palette.is_some());
    app.press(K::Esc, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(ranges(&app).len(), 3);
    // In the editor: back to the primary, then clear its selection.
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app), vec![(0, 2)]);
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app), vec![(2, 2)]);
    // Cmd+Shift+L keeps the primary at the match under the cursor.
    set_cursor(&mut app, 4);
    select_all(&mut app);
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app), vec![(3, 5)]);
}

/// Leaving the editor (here for the palette) ends the Cmd+D session, as blurring does in VS Code.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_6_focus_loss_ends_word_mode() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "foo foobar Foo foo", 0);
    add_next(&mut app);
    app.press(K::Char('p'), cmd() | M::SHIFT);
    app.press(K::Esc, M::NONE);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (4, 7)], "selection mode now: inside foobar");
}

#[test]
fn milestone_6_draws_every_selection_and_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "ab ab ab\nzz\n", 0);
    select_all(&mut app);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    let row_text = |y: u16| (0..80).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
    let y = (0..12).find(|&y| row_text(y).contains("ab ab ab")).unwrap();
    let row = row_text(y);
    let x0 = row[..row.find("ab ab ab").unwrap()].chars().count() as u16; // one char per cell

    let selection_bg = app.theme.get("editor.selection").bg;
    let reversed = app.theme.get("editor.cursor.secondary").add_modifier;
    let cell = |b: u16| &buf[(x0 + b, y)];
    for b in [0, 1, 3, 4, 6, 7] {
        assert_eq!(cell(b).bg, selection_bg.unwrap(), "byte {b} is selected");
    }
    assert_ne!(cell(2).bg, selection_bg.unwrap());
    // Heads are at the ends: the primary's (2) is the terminal cursor, the others are reversed.
    assert!(!cell(2).modifier.contains(reversed));
    assert!(cell(5).modifier.contains(reversed));
    assert!(cell(8).modifier.contains(reversed), "a cursor at the line end");
    term.backend_mut().assert_cursor_position((x0 + 2, y));
    assert!(row_text(11).contains("3 selections"), "status: {}", row_text(11));
    assert!(!row_text(11).contains("Ln "));

    // Cmd+D scrolls to show the new selection (centred when it was off screen).
    let mut lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    lines[90] = "target".into();
    lines[1] = "target".into();
    let mut app = app_with(&dir, &lines.join("\n"), 7);
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    add_next(&mut app);
    add_next(&mut app);
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let view = &app.group().active_tab().unwrap().view;
    assert_eq!(view.top_line(), 90 - view.height / 2, "line 90 centred");

    // Scrolled down, every visible line's selection and secondary cursor is drawn (only the
    // selections overlapping the viewport are looked at).
    let text: String = (0..100).map(|_| "ab cd\n").collect();
    let mut app = app_with(&dir, &text, 0);
    select_all(&mut app);
    assert_eq!(ranges(&app).len(), 100);
    let rope = app.active_buffer().unwrap().rope.clone();
    let view = app.active_mut().unwrap().1;
    view.set_top_line(&rope, 50);
    view.reveal_cursor = false;
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let height = app.group().active_tab().unwrap().view.height as u16;
    assert_eq!(app.group().active_tab().unwrap().view.top_line(), 50);
    let buf = term.backend().buffer().clone();
    let row_text = |y: u16| (0..80).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
    let first = (0..12).find(|&y| row_text(y).contains(" 51 ")).unwrap();
    let row = row_text(first);
    let x0 = row[..row.find("ab cd").unwrap()].chars().count() as u16;
    for y in first..first + height {
        assert!(row_text(y).contains(&format!(" {} ab cd", 51 + y - first)), "row {y}");
        assert_eq!(buf[(x0, y)].bg, selection_bg.unwrap(), "row {y} selected");
        assert_eq!(buf[(x0 + 1, y)].bg, selection_bg.unwrap(), "row {y} selected");
        assert_ne!(buf[(x0 + 3, y)].bg, selection_bg.unwrap(), "row {y}: cd isn't");
        assert!(
            buf[(x0 + 2, y)].modifier.contains(reversed),
            "row {y}: secondary cursor"
        );
    }
}

#[test]
fn milestone_6_other_group_follows_multi_cursor_edit() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "ab ab ab\nend", 0);
    let first = app.groups.active;
    select_all(&mut app);
    // Split: the second group starts with a copy of the three selections.
    app.press(K::Char('\\'), cmd());
    let second = app.groups.active;
    assert_ne!(first, second);
    let first_sels = |app: &App| {
        let g = app.groups.get(first).unwrap();
        ranges_of(&g.tabs[g.active].view.selections)
    };
    assert_eq!(first_sels(&app).len(), 3);

    app.type_text("X");
    assert_eq!(text(&app), "X X X\nend");
    assert_eq!(
        first_sels(&app),
        vec![(0, 1), (2, 3), (4, 5)],
        "still around each replaced word"
    );
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "ab ab ab\nend");
    assert_eq!(ranges(&app), vec![(0, 2), (3, 5), (6, 8)]);
    assert_eq!(first_sels(&app), vec![(0, 2), (3, 5), (6, 8)]);

    // The other view's cursor history was cleared by the edit.
    app.press(K::Char('1'), cmd());
    let before = first_sels(&app);
    cursor_undo(&mut app);
    assert_eq!(first_sels(&app), before);
}

/// The milestone's "done when", with a second group on the same buffer following each edit.
#[test]
fn milestone_6_select_all_matches_on_one_mb() {
    let mut original = String::new();
    let mut i = 0;
    while original.len() < 1_000_000 {
        original.push_str(&format!("let item = value_{i}; // item\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, &original, 5);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();

    let t = Instant::now();
    select_all(&mut app);
    let select_time = t.elapsed();
    assert!(select_time < DEBUG_BOUND, "Cmd+Shift+L took {select_time:?}");
    let count = ranges(&app).len();
    assert_eq!(count, 2 * i, "two whole-word `item`s per line");
    assert!(count > 50_000);
    println!("Cmd+Shift+L: {count} selections in {select_time:?}");

    // A second group on the buffer with the same selections, mapping through every edit.
    app.press(K::Char('\\'), cmd());
    app.press(K::Char('1'), cmd());
    let before = sels(&app);

    let n = 5u32;
    let t = Instant::now();
    for c in "thing".chars() {
        let key = Instant::now();
        app.press(K::Char(c), M::NONE);
        term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
        // Debug builds are ~15x slower (~60 ms a key); this only catches going quadratic.
        assert!(key.elapsed() < DEBUG_BOUND, "typing {c:?} took {:?}", key.elapsed());
    }
    let typing = t.elapsed();
    println!(
        "typing at {count} cursors: {n} keys in {typing:?}, avg {:?}",
        typing / n
    );
    assert!(text(&app).starts_with("let thing = value_0; // thing\n"));
    let g2 = app.groups.ids()[1];
    let other = app.groups.get(g2).unwrap().tabs[0].view.selections.clone();
    assert_eq!(other.len(), count);
    let first = other.as_slice()[0];
    assert_eq!(
        (first.start(), first.end()),
        (ByteIdx(4), ByteIdx(5)),
        "other group mapped through the edit"
    );

    let t = Instant::now();
    app.press(K::Char('z'), cmd());
    let undo_time = t.elapsed();
    assert!(undo_time < DEBUG_BOUND, "undo took {undo_time:?}");
    println!("undo: {undo_time:?}");
    assert_eq!(text(&app), original);
    assert_eq!(
        sels(&app),
        before,
        "one undo restores the selection list from before typing"
    );

    if !cfg!(debug_assertions) {
        assert!(select_time < SELECT_ALL_BUDGET, "Cmd+Shift+L too slow: {select_time:?}");
        assert!(typing / n < KEYSTROKE_BUDGET, "typing too slow: avg {:?}", typing / n);
        assert!(undo_time < UNDO_BUDGET, "undo too slow: {undo_time:?}");
    }
}

/// Cmd+U puts the scroll back with the selections: after Cmd+D scrolled to a far match, after
/// a sideways scroll, and after Page Down (which scrolls during the key itself).
#[test]
fn milestone_6_cursor_undo_restores_scroll() {
    let dir = tempfile::tempdir().unwrap();
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    let mut lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    lines[1] = "target".into();
    lines[90] = "target".into();
    let mut app = app_with(&dir, &lines.join("\n"), 7);
    let top = |app: &App| app.group().active_tab().unwrap().view.top_line();
    let left = |app: &App| app.group().active_tab().unwrap().view.left_col;
    draw(&mut term, &mut app);
    add_next(&mut app);
    draw(&mut term, &mut app);
    add_next(&mut app);
    draw(&mut term, &mut app);
    assert!(top(&app) > 80);
    cursor_undo(&mut app);
    draw(&mut term, &mut app);
    assert_eq!(ranges(&app), vec![(7, 13)]);
    assert_eq!(top(&app), 0, "scrolled back up");

    // Page Down moves the view during the event; the entry keeps the scroll from before it.
    set_cursor(&mut app, 0);
    draw(&mut term, &mut app);
    app.press(K::PageDown, M::NONE);
    draw(&mut term, &mut app);
    assert!(top(&app) > 0);
    cursor_undo(&mut app);
    draw(&mut term, &mut app);
    assert_eq!((ranges(&app), top(&app)), (vec![(0, 0)], 0));

    // Sideways: End scrolls right, Home back; Cmd+U returns to the line end, scrolled right.
    let long = format!("{}\nshort", "x".repeat(300));
    let mut app = app_with(&dir, &long, 0);
    draw(&mut term, &mut app);
    app.press(K::End, M::NONE);
    draw(&mut term, &mut app);
    let right = left(&app);
    assert!(right > 200);
    app.press(K::Home, M::NONE);
    draw(&mut term, &mut app);
    assert_eq!(left(&app), 0);
    cursor_undo(&mut app);
    draw(&mut term, &mut app);
    assert_eq!((ranges(&app), left(&app)), (vec![(300, 300)], right));
}

/// Cmd+U changes the selections, so it ends the session (VS Code resets it on any selection
/// change it didn't make): the next Cmd+D searches for the selected text, not the whole word.
#[test]
fn milestone_6_cursor_undo_ends_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "foo foobar Foo foo", 0);
    add_next(&mut app);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (15, 18)]);
    cursor_undo(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3)]);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (4, 7)]);
}

/// A session belongs to one view: Cmd+D in the other group on the same buffer starts afresh.
#[test]
fn milestone_6_session_belongs_to_its_view() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "foo foobar foo", 0);
    add_next(&mut app); // word mode: selects foo
    app.press(K::Char('\\'), cmd());
    assert_eq!(ranges(&app), vec![(0, 3)], "the new group copies the selection");
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (4, 7)], "selected-text mode in this view");
}

/// Esc drops extra cursors only in the editor: not while the explorer has focus, and not when
/// it answers a prompt.
#[test]
fn milestone_6_esc_outside_the_editor_keeps_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "ab ab ab", 0);
    select_all(&mut app);
    app.press(K::Char('e'), cmd() | M::SHIFT);
    assert_eq!(app.ui.focus, crate::app::Focus::SidePanel);
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app).len(), 3, "Esc in the explorer");

    let mut app = app_with(&dir, "ab ab ab", 0);
    app.type_text("x ");
    select_all(&mut app);
    let before = ranges(&app);
    assert_eq!(before.len(), 3);
    app.press(K::Char('w'), cmd());
    assert!(app.prompt.is_some(), "save before closing?");
    app.press(K::Esc, M::NONE);
    assert!(app.prompt.is_none());
    assert_eq!(ranges(&app), before, "Esc in a prompt");
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app).len(), 1, "then in the editor");
}

/// Cmd+Shift+L from the last match makes the first one the newest (VS Code swaps the primary to
/// the front of its list); typing keeps that, and the next Cmd+D searches after it.
#[test]
fn milestone_6_newest_survives_typing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "xx ab xx ab", 9);
    select_all(&mut app);
    assert_eq!(ranges(&app), vec![(3, 5), (9, 11)]);
    app.type_text("xx");
    assert_eq!(text(&app), "xx xx xx xx");
    add_next(&mut app); // several cursors: expand each to its word first
    assert_eq!(ranges(&app), vec![(3, 5), (9, 11)]);
    add_next(&mut app);
    assert_eq!(
        ranges(&app),
        vec![(3, 5), (6, 8), (9, 11)],
        "after the newest, not the last"
    );
}
