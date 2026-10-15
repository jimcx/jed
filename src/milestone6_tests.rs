//! Milestone 6 end to end (6b): Cmd+D, Cmd+Shift+L, Cmd+Alt+D, Cmd+U and Esc through the real
//! keymap, the drawn selections and status bar, two groups on one buffer, and the "done when":
//! Cmd+Shift+L on a 1 MB file with tens of thousands of matches, then typing, stays smooth, and
//! one undo restores the selection list from before the typing.
//!
//! 6c: add cursor above / below, Shift+Alt+I, Alt+Click and Alt+drag, box select from the
//! keyboard and with Shift+Alt+drag, paste spread across cursors, and move / copy lines at every
//! cursor, each one undo step; the line and cursor commands at the 1 MB file's 63k selections.
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
/// A line or cursor command at every one of those selections, plus a frame (~25-35 ms measured
/// on Linux x86).
const LINE_COMMAND_BUDGET: Duration = Duration::from_millis(80);
/// The same in debug builds (~0.5 s, more with other tests running): only catches going
/// quadratic, which at 60k selections takes minutes.
const LINE_COMMAND_DEBUG_BOUND: Duration = Duration::from_secs(5);

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

// ---- 6c: add cursor above / below, Shift+Alt+I, Alt+Click, box select, paste spreading,
// ---- move / copy lines.

fn add_below(app: &mut App) {
    app.press(K::Down, cmd() | M::ALT);
}

fn add_above(app: &mut App) {
    app.press(K::Up, cmd() | M::ALT);
}

fn box_key(app: &mut App, key: K) {
    app.press(key, cmd() | M::ALT | M::SHIFT);
}

fn undo(app: &mut App) {
    app.press(K::Char('z'), cmd());
}

fn paste_text(app: &mut App, text: &str) {
    app.handle_test_event(crossterm::event::Event::Paste(text.into()));
}

/// Where the text starts on screen: the row showing line `line` and the column of its byte 0.
/// Lines must start with distinct text.
fn screen_of(term: &Terminal<TestBackend>, app: &App, line: usize) -> (u16, u16) {
    let rope = &app.active_buffer().unwrap().rope;
    let start = crate::buffer::pos::line_text(rope, line);
    let buf = term.backend().buffer();
    let (w, h) = (buf.area.width, buf.area.height);
    for y in 0..h {
        let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
        if let Some(i) = row.find(&format!(" {} {start}", line + 1)) {
            let x = row[..i].chars().count() + format!(" {} ", line + 1).len();
            return (x as u16, y);
        }
    }
    panic!("line {line} not on screen");
}

#[test]
fn milestone_6_add_cursor_above_and_below() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "abcdef\nab\nabcdef\nabcdef", 5);
    add_below(&mut app);
    add_below(&mut app);
    assert_eq!(
        ranges(&app),
        vec![(5, 5), (9, 9), (15, 15)],
        "same column, clamped on the short line"
    );
    app.type_text("X");
    assert_eq!(text(&app), "abcdeXf\nabX\nabcdeXf\nabcdef");
    undo(&mut app);
    assert_eq!(text(&app), "abcdef\nab\nabcdef\nabcdef");
    assert_eq!(ranges(&app).len(), 3, "undo restores the three cursors");

    // On the first line there's nothing above: no change.
    set_cursor(&mut app, 2);
    add_above(&mut app);
    assert_eq!(ranges(&app), vec![(2, 2)]);
    // Esc goes back to one cursor; Cmd+U undoes an added cursor.
    add_below(&mut app);
    assert_eq!(ranges(&app), vec![(2, 2), (9, 9)]);
    cursor_undo(&mut app);
    assert_eq!(ranges(&app), vec![(2, 2)]);
    add_below(&mut app);
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app), vec![(2, 2)], "the primary stays the original cursor");

    // Adding below past the bottom of the screen scrolls to the new cursor.
    let lines: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&dir, &lines, 0);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    draw(&mut term, &mut app);
    for _ in 0..30 {
        add_below(&mut app);
    }
    draw(&mut term, &mut app);
    let view = &app.group().active_tab().unwrap().view;
    assert_eq!(view.selections.len(), 31);
    assert_eq!(view.top_line() + view.height, 31, "line 30 is the last one shown");
}

#[test]
fn milestone_6_cursors_at_line_ends() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "let a = 1\nlet bb = 2\nlet c = 3\nend", 4);
    app.press(K::Down, M::SHIFT);
    app.press(K::Down, M::SHIFT);
    app.press(K::End, M::SHIFT);
    // Terminals report Shift+Alt+I as `I` or `i` with Shift.
    app.press(K::Char('I'), M::ALT | M::SHIFT);
    assert_eq!(ranges(&app), vec![(9, 9), (20, 20), (30, 30)]);
    app.type_text(";");
    assert_eq!(text(&app), "let a = 1;\nlet bb = 2;\nlet c = 3;\nend");
    // Only cursors: nothing to do.
    app.press(K::Char('i'), M::ALT | M::SHIFT);
    assert_eq!(ranges(&app), vec![(10, 10), (22, 22), (33, 33)]);
}

#[test]
fn milestone_6_keyboard_box_select() {
    let dir = tempfile::tempdir().unwrap();
    // Line starts: 0, 9, 11, 19. The tab covers columns 0..4.
    let mut app = app_with(&dir, "abcdefgh\na\n\tbcdefg\nabcdefgh", 4);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    draw(&mut term, &mut app);
    for _ in 0..3 {
        box_key(&mut app, K::Down);
    }
    assert_eq!(
        ranges(&app),
        vec![(4, 4), (10, 10), (12, 12), (23, 23)],
        "a column of cursors at column 4"
    );
    box_key(&mut app, K::Right);
    box_key(&mut app, K::Right);
    assert_eq!(
        ranges(&app),
        vec![(4, 6), (12, 14), (23, 25)],
        "columns 4..6: the short line drops out"
    );
    box_key(&mut app, K::Up);
    assert_eq!(ranges(&app), vec![(4, 6), (12, 14)], "shrinks from the moving corner");
    app.type_text("X");
    assert_eq!(text(&app), "abcdXgh\na\n\tXdefg\nabcdefgh");
    undo(&mut app);
    assert_eq!(text(&app), "abcdefgh\na\n\tbcdefg\nabcdefgh");

    // Any other selection change starts a new box from the primary selection, here leftwards.
    set_cursor(&mut app, 3);
    box_key(&mut app, K::Left);
    assert_eq!(ranges(&app), vec![(2, 3)]);
    box_key(&mut app, K::PageDown);
    // Columns 2..3 are inside the tab, nearer its end: the tab is selected.
    assert_eq!(ranges(&app), vec![(2, 3), (11, 12), (21, 22)]);
    let sel = sels(&app);
    assert!(sel.iter().all(|s| s.head < s.anchor), "anchored on the right");
}

#[test]
fn milestone_6_alt_click_and_box_drag() {
    use crossterm::event::{MouseButton, MouseEventKind as E};
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "abcdef\nghijkl\nmnopqr\nstuvwx", 0);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    draw(&mut term, &mut app);
    let (x0, y0) = screen_of(&term, &app, 0);
    let (x2, y2) = screen_of(&term, &app, 2);
    let click = |app: &mut App, x, y, mods| {
        app.mouse_with(E::Down(MouseButton::Left), x, y, mods);
        app.mouse_with(E::Up(MouseButton::Left), x, y, mods);
    };

    // Alt+Click adds a cursor; the first one stays primary.
    click(&mut app, x0 + 3, y0, M::ALT);
    click(&mut app, x2 + 1, y2, M::ALT);
    assert_eq!(ranges(&app), vec![(0, 0), (3, 3), (15, 15)]);
    assert_eq!(sels(&app).primary().head.0, 0);
    app.type_text("-");
    assert_eq!(text(&app), "-abc-def\nghijkl\nm-nopqr\nstuvwx");
    draw(&mut term, &mut app);
    // Alt+Click on a cursor removes it.
    click(&mut app, x0 + 5, y0, M::ALT);
    assert_eq!(ranges(&app), vec![(1, 1), (18, 18)]);
    // ... but never the last one.
    click(&mut app, x2 + 2, y2, M::ALT);
    click(&mut app, x0 + 1, y0, M::ALT);
    assert_eq!(ranges(&app), vec![(1, 1)]);

    // Alt+drag turns the added cursor into a selection.
    app.mouse_with(E::Down(MouseButton::Left), x2 + 1, y2, M::ALT);
    app.mouse_with(E::Drag(MouseButton::Left), x2 + 4, y2, M::ALT);
    app.mouse_with(E::Up(MouseButton::Left), x2 + 4, y2, M::ALT);
    assert_eq!(ranges(&app), vec![(1, 1), (17, 20)]);
    // Double Alt+Click adds the word under the pointer.
    click(&mut app, x2 + 2, y2 + 1, M::ALT);
    click(&mut app, x2 + 2, y2 + 1, M::ALT);
    assert_eq!(ranges(&app), vec![(1, 1), (17, 20), (24, 30)]);

    // Shift+Alt+drag: a box from the primary's anchor (VS Code) to the pointer.
    let mut app = app_with(&dir, "abcdef\nghijkl\nmnopqr\nstuvwx", 0);
    draw(&mut term, &mut app);
    app.mouse(E::Down(MouseButton::Left), x0 + 1, y0);
    app.mouse(E::Up(MouseButton::Left), x0 + 1, y0);
    app.mouse_with(E::Down(MouseButton::Left), x0 + 2, y0 + 1, M::ALT | M::SHIFT);
    app.mouse_with(E::Drag(MouseButton::Left), x0 + 3, y2, M::ALT | M::SHIFT);
    assert_eq!(ranges(&app), vec![(1, 3), (8, 10), (15, 17)]);
    // Dragging past the line ends keeps the column; the keyboard grows the same box.
    app.mouse_with(E::Drag(MouseButton::Left), x0 + 30, y2, M::ALT | M::SHIFT);
    app.mouse_with(E::Up(MouseButton::Left), x0 + 30, y2, M::ALT | M::SHIFT);
    assert_eq!(ranges(&app), vec![(1, 6), (8, 13), (15, 20)]);
    box_key(&mut app, K::Down);
    assert_eq!(ranges(&app), vec![(1, 6), (8, 13), (15, 20), (22, 27)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_6_paste_spreads_across_cursors() {
    let dir = tempfile::tempdir().unwrap();
    // Copy three selections, paste them back at three cursors: one each.
    let mut app = app_with(&dir, "one two three\nx\ny\nz", 0);
    // Three different words selected (as Alt+drag would), copied.
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new(
        vec![
            Selection::new(ByteIdx(0), ByteIdx(3)),
            Selection::new(ByteIdx(4), ByteIdx(7)),
            Selection::new(ByteIdx(8), ByteIdx(13)),
        ],
        0,
    );
    app.press(K::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "one\ntwo\nthree");
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new([15, 17, 19].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
    paste_text(&mut app, "one\ntwo\nthree");
    assert_eq!(text(&app), "one two three\nxone\nytwo\nzthree");
    undo(&mut app);
    assert_eq!(text(&app), "one two three\nx\ny\nz", "one undo step");

    // From elsewhere: as many lines as cursors spread too; the final newline doesn't count.
    paste_text(&mut app, "1\n2\n3\n");
    assert_eq!(text(&app), "one two three\nx1\ny2\nz3");
    undo(&mut app);
    // Mismatched counts: the whole text at every cursor.
    paste_text(&mut app, "1\n2");
    assert_eq!(text(&app), "one two three\nx1\n2\ny1\n2\nz1\n2");
    undo(&mut app);

    // A whole line copied from one empty cursor goes above each cursor's line, never spread.
    set_cursor(&mut app, 0);
    app.press(K::Char('c'), cmd());
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new([15, 17].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
    paste_text(&mut app, "one two three\n");
    assert_eq!(text(&app), "one two three\none two three\nx\none two three\ny\nz");
    undo(&mut app);

    // Two cursors' lines copied (not whole-line any more, as VS Code) spread back one each.
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new([15, 17].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
    app.press(K::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "x\ny\n");
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new([17, 19].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
    paste_text(&mut app, "x\ny\n");
    assert_eq!(text(&app), "one two three\nx\nyx\nzy");
    undo(&mut app);
    // The same copy at three cursors: neither spread nor above the lines, but the whole text
    // inline at each (VS Code: only one empty cursor's copy is a whole-line copy).
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new([15, 17, 19].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
    paste_text(&mut app, "x\ny\n");
    assert_eq!(text(&app), "one two three\nxx\ny\n\nyx\ny\n\nzx\ny\n");
    undo(&mut app);
    // ... and at one cursor, inline too.
    set_cursor(&mut app, 2);
    paste_text(&mut app, "x\ny\n");
    assert_eq!(text(&app), "onx\ny\ne two three\nx\ny\nz");
    undo(&mut app);

    // Red's own pieces spread even when a piece holds a line break (three lines, two cursors).
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new(
        vec![
            Selection::new(ByteIdx(0), ByteIdx(3)),
            Selection::new(ByteIdx(8), ByteIdx(15)),
        ],
        0,
    );
    app.press(K::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "one\nthree\nx");
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::new([17, 19].map(|p| Selection::cursor(ByteIdx(p))).to_vec(), 0);
    paste_text(&mut app, "one\nthree\nx");
    assert_eq!(text(&app), "one two three\nx\nyone\nzthree\nx");
}

#[test]
fn milestone_6_move_and_copy_lines() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "a\r\nb\r\nc\r\nd", 3);
    let alt_up = |app: &mut App| app.press(K::Up, M::ALT);
    let alt_down = |app: &mut App| app.press(K::Down, M::ALT);

    // At the top already: nothing happens and there's nothing to undo.
    set_cursor(&mut app, 0);
    alt_up(&mut app);
    assert_eq!(text(&app), "a\r\nb\r\nc\r\nd");
    assert!(!app.active_buffer().unwrap().is_dirty());
    undo(&mut app);
    assert_eq!(text(&app), "a\r\nb\r\nc\r\nd");

    // Two cursors on neighbouring lines move as one block, down to the last line (no break).
    set_cursor(&mut app, 3);
    add_below(&mut app);
    alt_down(&mut app);
    assert_eq!(text(&app), "a\r\nd\r\nb\r\nc");
    assert_eq!(ranges(&app), vec![(6, 6), (9, 9)]);
    alt_down(&mut app);
    assert_eq!(text(&app), "a\r\nd\r\nb\r\nc", "at the bottom");
    alt_up(&mut app);
    alt_up(&mut app);
    assert_eq!(text(&app), "b\r\nc\r\na\r\nd");
    undo(&mut app);
    undo(&mut app);
    undo(&mut app);
    assert_eq!(text(&app), "a\r\nb\r\nc\r\nd", "one undo step per move");
    assert_eq!(ranges(&app), vec![(3, 3), (6, 6)]);

    // Copy down: the cursors go with the lower copies; copy up: they stay on the upper ones.
    app.press(K::Down, M::ALT | M::SHIFT);
    assert_eq!(text(&app), "a\r\nb\r\nb\r\nc\r\nc\r\nd");
    assert_eq!(ranges(&app), vec![(6, 6), (12, 12)]);
    undo(&mut app);
    app.press(K::Up, M::ALT | M::SHIFT);
    assert_eq!(text(&app), "a\r\nb\r\nb\r\nc\r\nc\r\nd");
    assert_eq!(ranges(&app), vec![(3, 3), (9, 9)]);
    undo(&mut app);
    assert_eq!(text(&app), "a\r\nb\r\nc\r\nd");

    // A selection across lines keeps covering the moved text.
    // Lines 1 and 2 selected whole (ending at line 3's start, which doesn't count).
    let mut app = app_with(&dir, "1\n22\n333\n4", 2);
    app.press(K::Down, M::SHIFT);
    app.press(K::Down, M::SHIFT);
    app.press(K::Up, M::ALT);
    assert_eq!(text(&app), "22\n333\n1\n4");
    assert_eq!(ranges(&app), vec![(0, 7)]);

    // Read-only buffers: line commands change nothing.
    app.active_mut().unwrap().0.read_only = Some("test".into());
    set_cursor(&mut app, 7);
    for mods in [M::ALT, M::ALT | M::SHIFT] {
        app.press(K::Up, mods);
        app.press(K::Down, mods);
    }
    assert_eq!(text(&app), "22\n333\n1\n4");
}

/// Line and cursor commands at the 1 MB file's 63k selections stay fast: copy lines down and
/// move lines down (31k blocks each), add cursor below, Shift+Alt+I on every line.
#[test]
fn milestone_6_cursor_and_line_commands_on_one_mb() {
    let mut original = String::new();
    let mut i = 0;
    while original.len() < 1_000_000 {
        original.push_str(&format!("let item = value_{i}; // item\n\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, &original, 5);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    draw(&mut term, &mut app);
    select_all(&mut app);
    let count = ranges(&app).len();
    assert_eq!(count, 2 * i);
    let before = sels(&app);

    let mut timed = |app: &mut App, what: &str, f: &dyn Fn(&mut App)| {
        let t = Instant::now();
        f(app);
        draw(&mut term, app);
        let took = t.elapsed();
        println!("{what} at {count} selections: {took:?}");
        assert!(took < LINE_COMMAND_DEBUG_BOUND, "{what} took {took:?}");
        if !cfg!(debug_assertions) {
            assert!(took < LINE_COMMAND_BUDGET, "{what} too slow: {took:?}");
        }
    };
    timed(&mut app, "copy lines down", &|app| {
        app.press(K::Down, M::ALT | M::SHIFT)
    });
    assert_eq!(ranges(&app).len(), count);
    assert!(text(&app).starts_with("let item = value_0; // item\nlet item = value_0; // item\n\n"));
    timed(&mut app, "undo", &|app| app.press(K::Char('z'), cmd()));
    assert_eq!(text(&app), original);
    assert_eq!(sels(&app), before);

    timed(&mut app, "move lines down", &|app| app.press(K::Down, M::ALT));
    assert!(text(&app).starts_with("\nlet item = value_0; // item\n\nlet item = value_1;"));
    assert_eq!(ranges(&app).len(), count);
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), original);

    // Each line's two selections land on the blank line below as one cursor.
    timed(&mut app, "add cursor below", &|app| app.press(K::Down, cmd() | M::ALT));
    assert_eq!(ranges(&app).len(), count + i);

    app.press(K::Char('a'), cmd());
    timed(&mut app, "cursors at line ends", &|app| {
        app.press(K::Char('I'), M::ALT | M::SHIFT)
    });
    assert_eq!(ranges(&app).len(), 2 * i, "every line but the empty last one");
}
