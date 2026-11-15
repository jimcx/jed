//! focus_groups: the focus-group keys and the group number label. Cmd/Ctrl+1-8 focus a group in
//! reading order (one past the last splits right), and View: Focus Next / Previous Editor Group
//! (Ctrl+Shift+. / ,) cycle through the groups in reading order, wrapping. With two or more
//! groups each tab bar's corner shows its number in reading order, and a click on it focuses the
//! group; a single group shows none.

use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::Command;
use crate::layout::GroupId;
use crate::milestone10_tests::draw_sized;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn open_app(dir: &Path) -> App {
    let root = workspace::normalize(dir);
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app.test_open(Buffer::from_str("one\ntwo\nthree\n"));
    app
}

/// Splits right until there are `n` groups (a split copies the active tab).
fn split_n(app: &mut App, n: usize) {
    for _ in 1..n {
        app.test_execute(Command::SplitRight);
    }
    assert_eq!(app.groups.ids().len(), n);
}

/// Each screen row as text.
fn lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// Group `id`'s tab bar as the last drawing laid it out.
fn tab_bar(app: &App, id: GroupId) -> Rect {
    app.hits
        .groups
        .iter()
        .find(|g| g.id == id)
        .expect("the group is drawn")
        .tab_bar
}

/// The gap a group's number label takes: its tab bar's last cell.
fn corner_cell(app: &App, id: GroupId) -> (u16, u16) {
    let bar = tab_bar(app, id);
    (bar.x + bar.width - 1, bar.y)
}

/// The character drawn in group `id`'s tab-bar corner (the last frame's drawing).
fn corner(app: &App, term: &Terminal<TestBackend>, id: GroupId) -> char {
    let (x, y) = corner_cell(app, id);
    let row = &lines(term)[y as usize];
    row.chars().nth(x as usize).expect("the corner cell is drawn")
}

/// Draws and checks every group's corner against its 1-based place in `groups.ids()`.
fn assert_numbers(app: &mut App) {
    let ids = app.groups.ids();
    let term = draw_sized(app, 100, 20);
    for (i, &id) in ids.iter().enumerate() {
        let expected = char::from_digit(i as u32 + 1, 10).expect("a single digit");
        assert_eq!(
            corner(app, &term, id),
            expected,
            "group {id} is {} in reading order {ids:?}",
            i + 1
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_a_single_group_shows_no_number() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 1);
    let term = draw_sized(&mut app, 100, 20);
    let row = &lines(&term)[tab_bar(&app, ids[0]).y as usize];
    assert!(
        !row.chars().any(|c| c.is_ascii_digit()),
        "no group number on the bar: {row:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_each_group_shows_its_number_in_reading_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    app.test_execute(Command::SplitRight);
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 2);
    let term = draw_sized(&mut app, 100, 20);
    assert_eq!(corner(&app, &term, ids[0]), '1', "the left group");
    assert_eq!(corner(&app, &term, ids[1]), '2', "the right group");
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_clicking_a_number_focuses_that_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    app.test_execute(Command::SplitRight);
    // Start on the left group, so the click on `2` is a real move.
    app.test_execute(Command::FocusGroup(1));
    let ids = app.groups.ids();
    assert_eq!(app.groups.active, ids[0]);
    let term = draw_sized(&mut app, 100, 20);
    let (x, y) = corner_cell(&app, ids[1]);
    assert_eq!(corner(&app, &term, ids[1]), '2');

    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    assert_eq!(app.groups.active, ids[1], "the number's click focused the group");
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_numbers_follow_reading_order_through_splits_closes_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    app.test_execute(Command::SplitRight);
    assert_numbers(&mut app);

    // Split the right group down: reading order left, right-top, right-bottom.
    assert_eq!(app.groups.ids().len(), 2);
    app.test_execute(Command::FocusGroup(2));
    app.test_execute(Command::SplitDown);
    assert_eq!(app.groups.ids().len(), 3);
    assert_numbers(&mut app);

    // Close the focused group (its only tab is the buffer another group still shows).
    app.test_execute(Command::CloseTab);
    assert_eq!(app.groups.ids().len(), 2, "the emptied group closed");
    assert_numbers(&mut app);

    // Reopen a group beside the focused one: the numbers shift to the new reading order.
    app.test_execute(Command::SplitRight);
    assert_eq!(app.groups.ids().len(), 3);
    assert_numbers(&mut app);

    // Fold back to a single group: its bar shows no number again.
    app.test_execute(Command::FocusGroup(2));
    app.test_execute(Command::CloseTab);
    app.test_execute(Command::FocusGroup(2));
    app.test_execute(Command::CloseTab);
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 1);
    let term = draw_sized(&mut app, 100, 20);
    let row = &lines(&term)[tab_bar(&app, ids[0]).y as usize];
    assert!(
        !row.chars().any(|c| c.is_ascii_digit()),
        "a single group shows no number: {row:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_an_overflowing_bar_keeps_its_number_and_the_arrow_moves_left() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    let root = workspace::normalize(dir.path());
    // Enough long-named tabs on one bar to overflow it.
    for i in 0..6 {
        let path = root.join(format!("long_filename_{i}.txt"));
        std::fs::write(&path, "x\n").unwrap();
        app.test_open(Buffer::open(&path).unwrap());
    }
    app.test_execute(Command::SplitRight);
    let ids = app.groups.ids();
    let term = draw_sized(&mut app, 100, 20);

    // The number keeps the bar's last cell; the `›` sits one cell left of it.
    let bar = tab_bar(&app, ids[0]);
    let arrows = app
        .hits
        .groups
        .iter()
        .find(|g| g.id == ids[0])
        .and_then(|g| g.tab_arrows)
        .expect("the left group's tabs overflow its bar");
    assert_eq!(corner(&app, &term, ids[0]), '1');
    assert_eq!(
        arrows.forward,
        bar.x + bar.width - 2,
        "the arrow moved left of the number"
    );
    let row = &lines(&term)[bar.y as usize];
    assert_eq!(row.chars().nth(arrows.forward as usize), Some('›'));

    // Clicking the number still focuses the group (the arrow did not take its cell).
    assert_eq!(app.groups.active, ids[1]);
    let (x, y) = corner_cell(&app, ids[0]);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    assert_eq!(app.groups.active, ids[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_cmd_8_focuses_the_eighth_and_one_past_the_last_splits_right() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    split_n(&mut app, 8);
    let ids = app.groups.ids();
    // Move to group 1 first, so focusing 8 is a real move back to the eighth group.
    app.test_execute(Command::FocusGroup(1));
    assert_eq!(app.groups.active, ids[0]);
    app.test_execute(Command::FocusGroup(8));
    assert_eq!(app.groups.active, ids[7], "the eighth group in reading order");

    // One past the last (9 with 8 groups) splits right, as VS Code: back on group 1, the new
    // group lands right of it, so second in reading order.
    app.test_execute(Command::FocusGroup(1));
    app.test_execute(Command::FocusGroup(9));
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 9, "a new group split right");
    assert_eq!(app.groups.active, ids[1]);
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_a_high_number_with_fewer_groups_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    split_n(&mut app, 2);
    let ids = app.groups.ids();
    // Group 2 is the active one after the split; 4 is neither a group nor one past the last.
    app.test_execute(Command::FocusGroup(4));
    assert_eq!(app.groups.ids(), ids);
    assert_eq!(app.groups.active, ids[1]);

    // One past the last with 2 groups splits right; the third group takes the focus.
    app.test_execute(Command::FocusGroup(3));
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 3);
    assert!(!ids[..2].contains(&app.groups.active), "the new group is active");
    app.test_execute(Command::FocusGroup(3));
    assert_eq!(app.groups.active, ids[2]);
    app.test_execute(Command::FocusGroup(2));
    assert_eq!(app.groups.active, ids[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_next_and_previous_cycle_in_reading_order_and_wrap() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    split_n(&mut app, 3);
    // A split right lands beside the group it split; the ids are the reading order.
    let ids = app.groups.ids();
    app.test_execute(Command::FocusGroup(1));
    assert_eq!(app.groups.active, ids[0]);
    for expected in [ids[1], ids[2], ids[0]] {
        app.test_execute(Command::FocusNextGroup);
        assert_eq!(app.groups.active, expected);
    }
    for expected in [ids[2], ids[1], ids[0]] {
        app.test_execute(Command::FocusPreviousGroup);
        assert_eq!(app.groups.active, expected);
    }

    // The keys: Ctrl+Shift+. / Ctrl+Shift+,; a terminal may report the shifted keys as `>` / `<`.
    let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    app.press(KeyCode::Char('.'), ctrl_shift);
    assert_eq!(app.groups.active, ids[1]);
    app.press(KeyCode::Char('>'), ctrl_shift);
    assert_eq!(app.groups.active, ids[2]);
    app.press(KeyCode::Char(','), ctrl_shift);
    assert_eq!(app.groups.active, ids[1]);
    app.press(KeyCode::Char('<'), ctrl_shift);
    assert_eq!(app.groups.active, ids[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_groups_next_and_previous_do_nothing_with_one_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = open_app(dir.path());
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 1);
    app.test_execute(Command::FocusNextGroup);
    app.test_execute(Command::FocusPreviousGroup);
    assert_eq!(app.groups.ids(), ids);
    assert_eq!(app.groups.active, ids[0]);
}
