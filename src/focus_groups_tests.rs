//! focus_groups-a: the focus-group keys. Cmd/Ctrl+1-8 focus a group in reading order (one past
//! the last splits right), and View: Focus Next / Previous Editor Group (Ctrl+Shift+. / ,) cycle
//! through the groups in reading order, wrapping. The `⌘n` corner label is focus_groups-b.

use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::Command;
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
