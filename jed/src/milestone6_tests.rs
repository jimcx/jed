//! Milestone 6 end to end (6b): Cmd+D, Cmd+Shift+L, Cmd+Alt+D, Cmd+U and Esc through the real
//! keymap, the drawn selections and status bar, two groups on one buffer, and the "done when":
//! Cmd+Shift+L on a 1 MB file with tens of thousands of matches, then typing, stays smooth, and
//! one undo restores the selection list from before the typing.
//!
//! 6c: add cursor above / below, Shift+Alt+I, Alt+Click and Alt+drag, box select from the
//! keyboard and with Shift+Alt+drag, paste spread across cursors, and move / copy lines at every
//! cursor, each one undo step; the line and cursor commands at the 1 MB file's 63k selections.
//!
//! 6d: the find / replace widget through its keys: incremental find, next / previous with wrap,
//! the option toggles (and Cmd+D borrowing them), an invalid regex, replace one and replace all
//! with regex groups (replace all one undo step, selections mapped), zero-length regex matches,
//! Alt+Enter, Esc closing only the widget, keys and paste staying in the widget, drawn highlights
//! and count following edits, undo and tab switches, seeding (regex-escaped, not multi-line), a
//! read-only buffer refusing replace, a `keybindings.toml` find-context binding; and the last
//! "done when": replace all on the 1 MB file is one undo step (text and selections), and fast.
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
/// Cmd+Shift+L and undo in any build, also debug (~0.2 s and ~0.3 s alone on Linux x86, up to
/// ~0.9 s for undo with the other tests running): only catches going quadratic, which at 63k
/// selections takes seconds even in release and far longer in debug.
const DEBUG_BOUND: Duration = Duration::from_secs(5);
/// One keystroke in any build (debug runs a 250 KB file; ~0.15 s a key at 63k cursors alone): the
/// keys must land within the 1 s undo grouping pause of each other for the one-undo check, so
/// this can't be raised like `DEBUG_BOUND`.
const KEY_DEBUG_BOUND: Duration = Duration::from_millis(900);
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
    app.test_plain_whitespace();
    select_all(&mut app);
    let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    let buf = term.backend().buffer().clone();
    let row_text = |y: u16| (0..80).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
    let y = (0..12).find(|&y| row_text(y).contains("ab ab ab")).unwrap();
    let row = row_text(y);
    let x0 = row[..row.find("ab ab ab").unwrap()].chars().count() as u16; // one char per cell

    let selection_bg = app.theme.get("editor.selection").bg;
    // Secondary cursors under the default `cursor_style = "line"`: underlined cells.
    let marked = app.theme.get("editor.cursor.secondaryUnderline").add_modifier;
    let cell = |b: u16| &buf[(x0 + b, y)];
    for b in [0, 1, 3, 4, 6, 7] {
        assert_eq!(cell(b).bg, selection_bg.unwrap(), "byte {b} is selected");
    }
    assert_ne!(cell(2).bg, selection_bg.unwrap());
    // Heads are at the ends: the primary's (2) is the terminal cursor, the others underlined.
    assert!(!cell(2).modifier.contains(marked));
    assert!(cell(5).modifier.contains(marked));
    assert!(cell(8).modifier.contains(marked), "a cursor at the line end");
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
    app.test_plain_whitespace();
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
        assert!(buf[(x0 + 2, y)].modifier.contains(marked), "row {y}: secondary cursor");
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
        ranges_of(&g.tabs[g.active].text().unwrap().view.selections)
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
    // Debug builds use a quarter of the file (~16k selections): still catches going quadratic,
    // and keeps each key well inside the 1 s undo grouping pause on a loaded, slow machine.
    let size = if cfg!(debug_assertions) { 250_000 } else { 1_000_000 };
    let mut original = String::new();
    let mut i = 0;
    while original.len() < size {
        original.push_str(&format!("let item = value_{i}; // item\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, &original, 5);
    // Frames draw every space as `·` (the default): the costliest drawing.
    assert_eq!(app.render_whitespace(), crate::edit::RenderWhitespace::All);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();

    let t = Instant::now();
    select_all(&mut app);
    let select_time = t.elapsed();
    assert!(select_time < DEBUG_BOUND, "Cmd+Shift+L took {select_time:?}");
    let count = ranges(&app).len();
    assert_eq!(count, 2 * i, "two whole-word `item`s per line");
    assert!(cfg!(debug_assertions) || count > 50_000);
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
        // Debug builds are ~15x slower; this only catches going quadratic.
        assert!(key.elapsed() < KEY_DEBUG_BOUND, "typing {c:?} took {:?}", key.elapsed());
    }
    let typing = t.elapsed();
    println!(
        "typing at {count} cursors: {n} keys in {typing:?}, avg {:?}",
        typing / n
    );
    assert!(text(&app).starts_with("let thing = value_0; // thing\n"));
    let g2 = app.groups.ids()[1];
    let other = app.groups.get(g2).unwrap().tabs[0]
        .text()
        .unwrap()
        .view
        .selections
        .clone();
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

    // Jed's own pieces spread even when a piece holds a line break (three lines, two cursors).
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
    // Frames draw every space as `·` (the default): the costliest drawing.
    assert_eq!(app.render_whitespace(), crate::edit::RenderWhitespace::All);
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

// ---- 6d: the find / replace widget ----

/// Replace all on the 1 MB file's 63k matches, plus a frame; also its undo (~11 ms and ~3 ms
/// measured on Linux x86).
const REPLACE_ALL_BUDGET: Duration = Duration::from_millis(50);
/// A key typed in the find input on the 1 MB file (one `find_all` of up to 126k matches), plus
/// a frame (~10 ms measured).
const FIND_KEY_BUDGET: Duration = Duration::from_millis(30);
/// Debug builds (~250 ms a find key, ~190 ms replace all): only catches going quadratic.
const FIND_DEBUG_BOUND: Duration = Duration::from_secs(3);

fn mac() -> bool {
    cfg!(target_os = "macos")
}

fn open_find(app: &mut App) {
    app.press(K::Char('f'), cmd());
}

/// Cmd+Alt+F on macOS, Ctrl+H on Linux.
fn open_replace(app: &mut App) {
    if mac() {
        app.press(K::Char('f'), M::SUPER | M::ALT);
    } else {
        app.press(K::Char('h'), M::CONTROL);
    }
}

/// Cmd+Alt+C / W / R on macOS, Alt+C / W / R on Linux.
fn toggle(app: &mut App, c: char) {
    let mods = if mac() { M::SUPER | M::ALT } else { M::ALT };
    app.press(K::Char(c), mods);
}

fn replace_all(app: &mut App) {
    app.press(K::Enter, cmd() | M::ALT);
}

fn count(app: &App) -> String {
    let sel = *app.group().active_tab().unwrap().view.selections.primary();
    app.find.count_label((sel.start(), sel.end()))
}

/// The screen's row `y` as text.
fn screen_row(term: &Terminal<TestBackend>, y: u16) -> String {
    let buf = term.backend().buffer();
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

#[test]
fn milestone_6_find_is_incremental_and_wraps() {
    use crate::app::Focus;
    let dir = tempfile::tempdir().unwrap();
    let original = "foo bar\nFoo baz foo\n";
    let mut app = app_with(&dir, original, 5);
    app.test_plain_whitespace();
    // Wide enough that the widget (top right) leaves the start of line 1 visible.
    let mut term = Terminal::new(TestBackend::new(160, 10)).unwrap();

    // Seeded from the word under the cursor, which becomes the current match.
    open_find(&mut app);
    assert!(app.find.visible);
    assert_eq!(app.ui.focus, Focus::Find);
    assert_eq!(app.find.query.text, "bar");
    assert_eq!(ranges(&app), vec![(4, 7)]);
    assert_eq!(count(&app), "1 of 1");

    // Typing replaces the selected query and moves to the nearest match from the selection.
    app.type_text("f");
    assert_eq!(app.find.query.text, "f");
    assert_eq!(ranges(&app), vec![(8, 9)]);
    app.type_text("oo");
    assert_eq!(ranges(&app), vec![(8, 11)], "case-insensitive by default");
    assert_eq!(count(&app), "2 of 3");
    assert_eq!(text(&app), original, "keys typed in the widget don't reach the buffer");

    // Enter / Shift+Enter, F3 / Shift+F3: next and previous, wrapping.
    app.press(K::Enter, M::NONE);
    assert_eq!(ranges(&app), vec![(16, 19)]);
    app.press(K::Enter, M::NONE);
    assert_eq!(ranges(&app), vec![(0, 3)], "wraps to the top");
    app.press(K::Enter, M::SHIFT);
    assert_eq!(ranges(&app), vec![(16, 19)], "wraps to the bottom");
    app.press(K::F(3), M::SHIFT);
    assert_eq!(ranges(&app), vec![(8, 11)]);
    app.press(K::Backspace, M::NONE);
    assert_eq!(app.find.query.text, "fo");
    assert_eq!(ranges(&app), vec![(8, 10)]);

    // Drawn: the widget with its count, the current match and the others highlighted.
    draw(&mut term, &mut app);
    let top = screen_row(&term, 1);
    assert!(
        top.contains("fo") && top.contains("Aa ab .*") && top.contains("2 of 3"),
        "{top:?}"
    );
    let editor_bg = |x: u16, y: u16| Some(term.backend().buffer()[(x, y)].bg);
    let current = app.theme.get("editor.findMatch").bg;
    let other = app.theme.get("editor.findMatchHighlight").bg;
    let (x0, y0) = screen_of(&term, &app, 0);
    let (x1, y1) = screen_of(&term, &app, 1);
    assert_eq!(editor_bg(x1, y1), current);
    assert_eq!(editor_bg(x1 + 1, y1), current);
    assert_eq!(
        editor_bg(x1 + 2, y1),
        app.theme.get("editor.lineHighlight").bg,
        "past the match"
    );
    assert_eq!(editor_bg(x0, y0), other);

    // Esc closes only the widget and gives the editor back its focus; the selection stays.
    app.press(K::Esc, M::NONE);
    assert!(!app.find.visible);
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(ranges(&app), vec![(8, 10)]);
    // F3 from the editor shows it again (focus stays) and goes on with the last query.
    app.press(K::F(3), M::NONE);
    assert!(app.find.visible);
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(ranges(&app), vec![(16, 18)]);
    // Esc in the editor closes the widget first.
    app.press(K::Esc, M::NONE);
    assert!(!app.find.visible);
    assert_eq!(ranges(&app), vec![(16, 18)]);
    assert_eq!(text(&app), original);
}

#[test]
fn milestone_6_find_toggles_and_invalid_regex() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "foo Foo food (x)\n", 0);
    let mut term = Terminal::new(TestBackend::new(80, 10)).unwrap();
    open_find(&mut app);
    assert_eq!(app.find.query.text, "foo");
    assert_eq!(app.find.matches().len(), 3);

    toggle(&mut app, 'c');
    assert!(app.find_options.case_sensitive);
    assert_eq!(app.find.matches().len(), 2);
    toggle(&mut app, 'w');
    assert_eq!(app.find.matches().len(), 1, "case and whole word: only the first");
    toggle(&mut app, 'c');
    toggle(&mut app, 'w');
    assert_eq!(app.find.matches().len(), 3);

    // A regex; then an invalid one shows its message, without results or a panic.
    toggle(&mut app, 'r');
    assert!(app.find_options.regex);
    app.find.query.select_all();
    app.type_text(r"f\w+d");
    assert_eq!(app.find.matches().len(), 1);
    assert_eq!(ranges(&app), vec![(8, 12)]);
    app.press(K::Home, M::NONE);
    app.type_text("(");
    assert!(app.find.error().is_some());
    assert!(app.find.matches().is_empty());
    assert_eq!(count(&app), "");
    draw(&mut term, &mut app);
    let error = app.find.error().unwrap().to_string();
    let shown = screen_row(&term, 2);
    assert!(app.find.error().is_some());
    assert!(shown.contains(&error[..error.len().min(20)]), "{shown:?} vs {error:?}");
    // Enter, Alt+Enter and replace with an invalid regex do nothing (the first replace all
    // only shows the replace input).
    app.press(K::Enter, M::NONE);
    app.press(K::Enter, M::ALT);
    replace_all(&mut app);
    assert!(app.find.replace_visible);
    replace_all(&mut app);
    assert_eq!(text(&app), "foo Foo food (x)\n");
    assert_eq!(ranges(&app), vec![(8, 12)]);
    draw(&mut term, &mut app);

    // Clicking a toggle flips it: regex off makes "(f\w+d" plain text (no match).
    let hit = app.hits.find.expect("drawn");
    let regex = hit.toggles[2];
    app.mouse(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        regex.x,
        regex.y,
    );
    assert!(!app.find_options.regex);
    assert!(app.find.error().is_none());
    assert_eq!(count(&app), "No results");
}

/// Cmd+D on selected text uses the widget's case and whole-word options; toggling them ends a
/// session that uses them (not a word-mode one).
#[test]
fn milestone_6_cmd_d_borrows_find_options() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "foo Foo foo", 0);
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(3)));
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (4, 7)], "case-insensitive by default");

    // Toggled from the editor (widget closed): says so, ends the session.
    toggle(&mut app, 'c');
    assert!(app.find_options.case_sensitive);
    assert!(app.message.as_ref().unwrap().text.contains("Match Case on"));
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(3)));
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (8, 11)], "Match Case on: skips Foo");
    toggle(&mut app, 'c');
    add_next(&mut app);
    assert_eq!(
        ranges(&app),
        vec![(0, 3), (4, 7), (8, 11)],
        "a new session with the new option"
    );

    // A word-mode session (from an empty cursor) doesn't use the options: a toggle keeps it.
    let mut app = app_with(&dir, "foo food foo", 1);
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3)]);
    toggle(&mut app, 'r');
    add_next(&mut app);
    assert_eq!(ranges(&app), vec![(0, 3), (9, 12)], "still whole word: skips food");
}

#[test]
fn milestone_6_replace_one_with_regex_groups() {
    use crate::app::Focus;
    use crate::find::Field;
    let dir = tempfile::tempdir().unwrap();
    let original = "k1=v1\nk2=v2\nk3=v3\n";
    let mut app = app_with(&dir, original, 0);
    open_replace(&mut app);
    assert!(app.find.replace_visible);
    assert_eq!(app.find.query.text, "k1");
    assert_eq!(
        app.find.field,
        Field::Replace,
        "seeded: the replace input has the cursor"
    );
    // Tab to the find input, a regex with groups, Tab back.
    app.press(K::Tab, M::NONE);
    assert_eq!(app.find.field, Field::Find);
    toggle(&mut app, 'r');
    app.type_text(r"(\w+)=(\w+)");
    assert_eq!(ranges(&app), vec![(0, 5)]);
    app.press(K::Tab, M::SHIFT);
    assert_eq!(app.find.field, Field::Replace);
    app.type_text("$2=$1");
    assert_eq!(text(&app), original, "typing in the replace input searches nothing");

    // Enter in the replace input: replace the current match, go to the next.
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "v1=k1\nk2=v2\nk3=v3\n");
    assert_eq!(ranges(&app), vec![(6, 11)]);
    // Cmd+Shift+1 too (reported by the terminal as Cmd+!).
    app.press(K::Char('!'), cmd());
    assert_eq!(text(&app), "v1=k1\nv2=k2\nk3=v3\n");
    assert_eq!(ranges(&app), vec![(12, 17)]);
    // Not on a match: replace one only moves to the next.
    set_cursor(&mut app, 5);
    app.press(K::Enter, M::NONE);
    assert_eq!(text(&app), "v1=k1\nv2=k2\nk3=v3\n");
    assert_eq!(ranges(&app), vec![(6, 11)]);

    // Each replacement is its own undo step.
    app.press(K::Esc, M::NONE);
    assert_eq!(app.ui.focus, Focus::Editor);
    undo(&mut app);
    assert_eq!(text(&app), "v1=k1\nk2=v2\nk3=v3\n");
    undo(&mut app);
    assert_eq!(text(&app), original);
}

#[test]
fn milestone_6_replace_all_is_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let original = "a=1, bb=22;\ncc=333 a=1\n";
    let mut app = app_with(&dir, original, 0);
    // Seeded with the word "a"; Tab to the find input, which keeps the seed selected.
    open_replace(&mut app);
    assert_eq!(app.find.query.text, "a");
    app.press(K::Tab, M::NONE);
    assert_eq!(app.find.field, crate::find::Field::Find);
    toggle(&mut app, 'r');
    app.type_text(r"(\w+)=(\d+)");
    assert_eq!(app.find.query.text, r"(\w+)=(\d+)", "typing replaced the selected seed");
    assert_eq!(ranges(&app), vec![(0, 3)]);
    app.press(K::Tab, M::NONE);
    assert_eq!(app.find.field, crate::find::Field::Replace);
    app.type_text("$2:$1$$");
    let before = sels(&app);
    replace_all(&mut app);
    assert_eq!(text(&app), "1:a$, 22:bb$;\n333:cc$ 1:a$\n");
    assert!(app.message.as_ref().unwrap().text.contains("Replaced 4 matches"));
    assert_eq!(count(&app), "No results");
    assert_eq!(
        ranges(&app),
        vec![(0, 4)],
        "the selection follows its match's replacement"
    );
    // Cmd+Z belongs to the editor: back there first.
    app.press(K::Esc, M::NONE);
    undo(&mut app);
    assert_eq!(text(&app), original, "one undo restores the exact text");
    assert_eq!(sels(&app), before, "and the selections");
    app.press(K::Char('z'), cmd() | M::SHIFT);
    assert_eq!(text(&app), "1:a$, 22:bb$;\n333:cc$ 1:a$\n");
}

/// Zero-length regex matches (`^`, `a*`): next / previous move on instead of sticking, and
/// replace all inserts once at each (no endless loop).
#[test]
fn milestone_6_find_zero_length_matches() {
    let dir = tempfile::tempdir().unwrap();
    let original = "ab\ncd\n\nef";
    let mut app = app_with(&dir, original, 0);
    open_find(&mut app);
    toggle(&mut app, 'r');
    app.type_text("^");
    assert_eq!(app.find.matches().len(), 4);
    assert_eq!(
        ranges(&app),
        vec![(0, 0)],
        "an empty match at the cursor is the current one"
    );
    assert_eq!(count(&app), "1 of 4");
    for want in [3, 6, 7, 0, 3] {
        app.press(K::Enter, M::NONE);
        assert_eq!(ranges(&app), vec![(want, want)]);
    }
    for want in [0, 7, 6] {
        app.press(K::Enter, M::SHIFT);
        assert_eq!(ranges(&app), vec![(want, want)]);
    }

    open_replace(&mut app);
    assert_eq!(app.find.query.text, "^", "no word at the empty line: the query stays");
    assert_eq!(app.find.field, crate::find::Field::Replace);
    app.type_text("> ");
    replace_all(&mut app);
    assert_eq!(text(&app), "> ab\n> cd\n> \n> ef");
    app.press(K::Esc, M::NONE);
    undo(&mut app);
    assert_eq!(text(&app), original);

    // `a*` matches empty between other characters and the `aa` run once.
    let mut app = app_with(&dir, "baac", 0);
    open_replace(&mut app);
    app.press(K::Tab, M::NONE);
    toggle(&mut app, 'r');
    app.type_text("a*");
    app.press(K::Tab, M::NONE);
    app.type_text("X");
    replace_all(&mut app);
    assert_eq!(text(&app), "XbXcX");
}

/// The matches (count and highlights) follow edits made while the widget is open, undo, and
/// switching to another tab.
#[test]
fn milestone_6_find_follows_edits_and_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "ab ab ab\n", 0);
    app.test_plain_whitespace();
    let mut term = Terminal::new(TestBackend::new(160, 10)).unwrap();
    let bg = |term: &Terminal<TestBackend>, x: u16, y: u16| Some(term.backend().buffer()[(x, y)].bg);
    let highlight = app.theme.get("editor.findMatchHighlight").bg;
    open_find(&mut app);
    draw(&mut term, &mut app);
    assert!(screen_row(&term, 1).contains("1 of 3"));

    // Another buffer in a new tab (same revision 0): its own matches, drawn.
    app.test_open(Buffer::from_str("zz\nzz ab\n"));
    app.press(K::PageUp, M::CONTROL);
    app.press(K::PageDown, M::CONTROL);
    assert_eq!(app.find.matches().len(), 1);
    draw(&mut term, &mut app);
    assert!(screen_row(&term, 1).contains("? of 1"), "{:?}", screen_row(&term, 1));
    let (x, y) = screen_of(&term, &app, 1);
    assert_eq!(bg(&term, x + 3, y), highlight);
    assert_ne!(bg(&term, x, y), highlight);
    app.press(K::PageUp, M::CONTROL);
    assert_eq!(app.find.matches().len(), 3);

    // Esc, then F3 shows the widget again with the focus in the editor; edits update it.
    app.press(K::Esc, M::NONE);
    app.press(K::F(3), M::NONE);
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    app.press(K::End, M::NONE);
    app.type_text(" ab");
    assert_eq!(text(&app), "ab ab ab ab\n");
    draw(&mut term, &mut app);
    assert!(screen_row(&term, 1).contains("? of 4"), "{:?}", screen_row(&term, 1));
    let (x, y) = screen_of(&term, &app, 0);
    assert_eq!(bg(&term, x + 9, y), highlight, "the typed match is highlighted");
    undo(&mut app);
    draw(&mut term, &mut app);
    assert!(screen_row(&term, 1).contains("of 3"), "{:?}", screen_row(&term, 1));
    assert_ne!(bg(&term, x + 9, y), highlight);
}

/// Seeding: a multi-line selection keeps the last query; with regex on, the selection is
/// escaped so it finds itself. A read-only buffer refuses replace one and replace all.
#[test]
fn milestone_6_find_seeding_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let original = "a.b axb\na.b\n";
    let mut app = app_with(&dir, original, 0);
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::single(Selection::new(ByteIdx(0), ByteIdx(3)));
    toggle(&mut app, 'r');
    open_find(&mut app);
    assert_eq!(app.find.query.text, r"a\.b");
    assert_eq!(app.find.matches().len(), 2, "not `axb`");
    assert_eq!(ranges(&app), vec![(0, 3)]);
    app.press(K::Esc, M::NONE);
    let view = app.active_mut().unwrap().1;
    view.selections = Selections::single(Selection::new(ByteIdx(4), ByteIdx(10)));
    open_find(&mut app);
    assert_eq!(app.find.query.text, r"a\.b", "a multi-line selection doesn't seed");
    // A paste goes to the input (replacing the selected query), without its line breaks.
    paste_text(&mut app, "a\nxb");
    assert_eq!(app.find.query.text, "axb");
    assert_eq!(ranges(&app), vec![(4, 7)]);
    assert_eq!(text(&app), original);

    let (buf, view) = app.active_mut().unwrap();
    buf.read_only = Some("test".into());
    view.selections = Selections::single(Selection::new(ByteIdx(8), ByteIdx(11)));
    open_replace(&mut app);
    assert_eq!(ranges(&app), vec![(8, 11)], "on a match");
    app.type_text("X");
    app.press(K::Enter, M::NONE);
    replace_all(&mut app);
    assert_eq!(text(&app), original);
    assert!(app.message.as_ref().unwrap().text.contains("Read-only"));
}

#[test]
fn milestone_6_alt_enter_selects_every_match() {
    use crate::app::Focus;
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, "ab x ab y ab", 6);
    open_find(&mut app);
    assert_eq!(ranges(&app), vec![(5, 7)]);
    app.press(K::Enter, M::ALT);
    assert_eq!(ranges(&app), vec![(0, 2), (5, 7), (10, 12)]);
    assert_eq!(sels(&app).primary_index(), 1, "the current match stays primary");
    assert_eq!(app.ui.focus, Focus::Editor, "focus goes back to the editor");
    assert!(app.find.visible);
    // Typing now edits at every selection.
    app.type_text("Z");
    assert_eq!(text(&app), "Z x Z y Z");
    // Esc closes the widget first, then drops the extra cursors.
    app.press(K::Esc, M::NONE);
    assert!(!app.find.visible);
    assert_eq!(ranges(&app).len(), 3);
    app.press(K::Esc, M::NONE);
    assert_eq!(ranges(&app).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_6_keybindings_toml_in_the_find_context() {
    use crate::config::keymap::{self, Keymap, Platform};
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join(keymap::FILE_NAME);
    std::fs::write(
        &config,
        r#"
        [[binding]]
        key = "ctrl+n"
        command = "find.next"
        context = "find"
        "#,
    )
    .unwrap();
    let mut app = app_with(&dir, "ab ab ab", 0);
    let (km, errors) = Keymap::load(&config, Platform::current());
    assert!(errors.is_empty(), "{errors:?}");
    app.set_keymap(km, errors);
    open_find(&mut app);
    assert_eq!(ranges(&app), vec![(0, 2)]);
    app.press(K::Char('n'), M::CONTROL);
    assert_eq!(ranges(&app), vec![(3, 5)]);
    // Enter lost its default binding: unbound in the widget, it does nothing.
    app.press(K::Enter, M::NONE);
    assert_eq!(ranges(&app), vec![(3, 5)]);
    assert_eq!(text(&app), "ab ab ab");
    // In the editor Ctrl+N isn't the find's and doesn't type: on Linux it's New Untitled File
    // (an empty tab), on macOS the Emacs-style cursor down.
    app.press(K::Esc, M::NONE);
    app.press(K::Char('n'), M::CONTROL);
    if cfg!(target_os = "macos") {
        assert_eq!(text(&app), "ab ab ab");
    } else {
        assert!(app.active_buffer().unwrap().is_untitled());
        assert_eq!(text(&app), "");
    }
}

/// The "done when": replace all on the 1 MB file (63k matches) is one undo step, and fast;
/// typing in the find input there stays smooth.
#[test]
fn milestone_6_replace_all_on_one_mb() {
    let mut original = String::new();
    let mut i = 0;
    while original.len() < 1_000_000 {
        original.push_str(&format!("let item = value_{i}; // item\n"));
        i += 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with(&dir, &original, 0);
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    draw(&mut term, &mut app);
    open_find(&mut app);
    draw(&mut term, &mut app);

    let mut slowest = Duration::ZERO;
    for c in "item".chars() {
        let t = Instant::now();
        app.press(K::Char(c), M::NONE);
        draw(&mut term, &mut app);
        slowest = slowest.max(t.elapsed());
    }
    let count = app.find.matches().len();
    assert_eq!(count, 2 * i);
    println!("find input at {count} matches: slowest key + frame {slowest:?}");
    assert!(slowest < FIND_DEBUG_BOUND, "find key took {slowest:?}");

    open_replace(&mut app);
    app.type_text("thing");
    let before = sels(&app);
    let t = Instant::now();
    replace_all(&mut app);
    draw(&mut term, &mut app);
    let took = t.elapsed();
    println!("replace all of {count} matches + frame: {took:?}");
    assert!(took < FIND_DEBUG_BOUND, "replace all took {took:?}");
    assert!(text(&app).starts_with("let thing = value_0; // thing\nlet thing = value_1;"));
    assert!(!text(&app).contains("item"));

    app.press(K::Esc, M::NONE);
    let t = Instant::now();
    undo(&mut app);
    draw(&mut term, &mut app);
    let undo_time = t.elapsed();
    println!("undo replace all: {undo_time:?}");
    assert!(text(&app) == original, "replace all is one undo step");
    assert_eq!(sels(&app), before, "with the selections from before");

    if !cfg!(debug_assertions) {
        assert!(slowest < FIND_KEY_BUDGET, "find key too slow: {slowest:?}");
        assert!(took < REPLACE_ALL_BUDGET, "replace all too slow: {took:?}");
        assert!(undo_time < REPLACE_ALL_BUDGET, "undo too slow: {undo_time:?}");
    }
}
