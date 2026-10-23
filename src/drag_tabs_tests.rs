//! drag_tabs-a: dragging a tab along its bar reorders it, the drop marker follows the pointer,
//! a press that never leaves the tab is a click, and Esc or a release outside the bar moves
//! nothing.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::group::Tab;
use crate::layout::{GroupId, TabHit};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

/// A workspace with `names` (40 numbered lines each), the first open, the others opened after
/// it in the same group (the last active).
fn open_app(dir: &tempfile::TempDir, names: &[&str]) -> (App, PathBuf) {
    let root = workspace::normalize(dir.path());
    for name in names {
        let text: String = (1..=40).map(|i| format!("{name} {i}\n")).collect();
        fs::write(root.join(name), text).unwrap();
    }
    let buffer = Buffer::open(&root.join(names[0])).unwrap();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.test_start();
    for name in &names[1..] {
        app.test_open(Buffer::open(&root.join(name)).unwrap());
    }
    (app, root)
}

/// Each group's tabs (file names, `diff:` + name for a diff tab) in reading order.
fn layout(app: &App) -> Vec<Vec<String>> {
    let name = |t: &Tab| match t {
        Tab::Text(t) => app.buffers.get(t.buffer).unwrap().name(),
        Tab::Diff(d) => format!("diff:{}", d.path.file_name().unwrap().to_string_lossy()),
        Tab::Hex(h) => format!("hex:{}", h.name()),
        Tab::Shell(s) => format!("shell:{}", s.title),
    };
    app.groups
        .ids()
        .into_iter()
        .map(|g| app.groups.get(g).unwrap().tabs.iter().map(name).collect())
        .collect()
}

fn active_name(app: &App) -> String {
    app.active_buffer().unwrap().name()
}

/// The active view's primary head (byte), top line and left column.
fn view_state(app: &App) -> (usize, usize, usize) {
    let v = &app.group().active_tab().unwrap().view;
    (v.selections.primary().head.0, v.top_line(), v.left_col)
}

fn draw(app: &mut App, w: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, 20)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// `group`'s bar: its y and its tabs as drawn by the last frame.
fn bar_of(app: &App, group: GroupId) -> (u16, Vec<TabHit>) {
    let g = app
        .hits
        .groups
        .iter()
        .find(|g| g.id == group)
        .expect("the group is drawn");
    (g.tab_bar.y, g.tabs.clone())
}

/// The first (leftmost) group's bar.
fn bar(app: &App) -> (u16, Vec<TabHit>) {
    bar_of(app, app.groups.ids()[0])
}

/// A cell in the left part of a tab (not its close marker).
fn in_tab(t: &TabHit) -> u16 {
    t.x0 + 1
}

/// The marker style's colour, from the theme as the app drew it.
fn marker(app: &App) -> ratatui::style::Color {
    app.theme.get("tab.dropMarker").bg.unwrap()
}

/// The cells of `group`'s bar drawn by the last frame in the drop marker's colour.
fn marker_cells(app: &App, term: &Terminal<TestBackend>, group: GroupId, y: u16) -> Vec<u16> {
    let bar = &app
        .hits
        .groups
        .iter()
        .find(|g| g.id == group)
        .expect("the group is drawn")
        .tab_bar;
    let colour = marker(app);
    (bar.x..bar.x + bar.width)
        .filter(|&x| term.backend().buffer()[(x, y)].bg == colour)
        .collect()
}

fn first_group(app: &App) -> GroupId {
    app.groups.ids()[0]
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_drag_right_reorders_at_the_end_and_keeps_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    // A place in a.txt: activated, scrolled down, with a selection.
    app.test_activate_tab(0);
    for _ in 0..19 {
        app.press(K::Down, M::NONE);
    }
    app.press(K::Right, M::SHIFT);
    draw(&mut app, 80);
    let before = view_state(&app);
    assert!(before.1 > 0, "the view is scrolled");
    let (y, tabs) = bar(&app);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"]]);

    // Press on a.txt (it activates), drag past c.txt's midpoint, release in the empty bar.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    assert_eq!(active_name(&app), "a.txt", "the press activates it");
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[2].x1, y);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"]], "nothing moves yet");
    // The marker is at the end of the last tab drawn.
    let term = draw(&mut app, 80);
    assert_eq!(
        marker_cells(&app, &term, first_group(&app), y),
        vec![tabs[2].x1],
        "the marker at the end"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[2].x1, y);

    assert_eq!(layout(&app), [vec!["b.txt", "c.txt", "a.txt"]], "moved to the end");
    assert_eq!(active_name(&app), "a.txt");
    assert_eq!(view_state(&app), before, "its selections and scroll are kept");
    let term = draw(&mut app, 80);
    assert!(
        marker_cells(&app, &term, first_group(&app), y).is_empty(),
        "no marker once dropped"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_drag_left_reorders_at_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    // The last opened (c.txt) is active; give it a selection.
    app.press(K::Right, M::SHIFT);
    draw(&mut app, 80);
    let before = view_state(&app);
    let (y, tabs) = bar(&app);

    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[2]), y);
    // Left of a.txt's midpoint: the gap before the first tab.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[0].x0, y);
    let term = draw(&mut app, 80);
    assert_eq!(
        marker_cells(&app, &term, first_group(&app), y),
        vec![tabs[0].x0],
        "the marker at the start"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[0].x0, y);

    assert_eq!(layout(&app), [vec!["c.txt", "a.txt", "b.txt"]]);
    assert_eq!(active_name(&app), "c.txt");
    assert_eq!(view_state(&app), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_drop_marker_follows_the_pointer_and_clears_outside() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    draw(&mut app, 80);
    let (y, tabs) = bar(&app);
    let editor_y = y + 3;

    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    // Right of b.txt's midpoint: the gap before c.txt.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[1].x1 - 1, y);
    let term = draw(&mut app, 80);
    assert_eq!(
        marker_cells(&app, &term, first_group(&app), y),
        vec![tabs[2].x0],
        "between b and c"
    );

    // Right of a.txt's midpoint: the gap before b.txt: the marker moves.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[0].x1 - 1, y);
    let term = draw(&mut app, 80);
    assert_eq!(
        marker_cells(&app, &term, first_group(&app), y),
        vec![tabs[1].x0],
        "between a and b"
    );

    // Over the editor there is no target: no marker.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[0].x0 + 1, editor_y);
    let term = draw(&mut app, 80);
    assert!(
        marker_cells(&app, &term, first_group(&app), y).is_empty(),
        "no target: no marker"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_press_and_release_on_a_tab_is_just_a_click() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    draw(&mut app, 80);
    let (y, tabs) = bar(&app);
    assert_eq!(active_name(&app), "c.txt", "the last opened tab is active");

    // No Drag event at all; released in the same tab's right half.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    assert_eq!(active_name(&app), "b.txt", "still activates");
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[1].x1 - 1, y);

    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"]], "nothing reordered");
    assert_eq!(active_name(&app), "b.txt");

    // A Drag that stays inside the tab is a click too, even across its midpoint.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[1].x1 - 1, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[1].x1 - 1, y);
    assert_eq!(
        layout(&app),
        [vec!["a.txt", "b.txt", "c.txt"]],
        "still nothing reordered"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_press_on_a_close_marker_starts_no_drag() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    draw(&mut app, 80);
    let (y, tabs) = bar(&app);

    // `×` on b.txt closes it; the press must not leave a drag behind.
    app.mouse(MouseEventKind::Down(MouseButton::Left), tabs[1].close_x, y);
    assert_eq!(layout(&app), [vec!["a.txt", "c.txt"]], "the close marker closed b.txt");
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[0].x0, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[0].x0, y);
    assert_eq!(layout(&app), [vec!["a.txt", "c.txt"]], "no drag followed the close");
    let term = draw(&mut app, 80);
    assert!(marker_cells(&app, &term, first_group(&app), y).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_esc_cancels_and_the_release_moves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    draw(&mut app, 80);
    let (y, tabs) = bar(&app);

    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[2].x1, y);
    let term = draw(&mut app, 80);
    assert_eq!(
        marker_cells(&app, &term, first_group(&app), y),
        vec![tabs[2].x1],
        "the drag is live"
    );

    app.press(K::Esc, M::NONE);
    let term = draw(&mut app, 80);
    assert!(
        marker_cells(&app, &term, first_group(&app), y).is_empty(),
        "Esc cleared the drag"
    );
    // The release at the same place no longer reorders.
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[2].x1, y);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_release_outside_any_target_moves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    draw(&mut app, 80);
    let (y, tabs) = bar(&app);

    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[2].x1 - 1, y);
    // Released over the editor rather than the bar.
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[0].x0 + 1, y + 3);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"]]);
    // The drag ended there: no stale marker is left on the bar.
    let term = draw(&mut app, 80);
    assert!(
        marker_cells(&app, &term, first_group(&app), y).is_empty(),
        "the release outside ended the drag"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_marker_marks_the_end_of_the_last_drawn_tab_on_a_scrolled_bar() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"]);
    // Tab 0 is active, so the bar stays scrolled to the start and later tabs aren't drawn.
    app.test_activate_tab(0);
    draw(&mut app, 41);
    let group = first_group(&app);
    let (y, tabs) = bar_of(&app, group);
    let last = *tabs.last().unwrap();
    assert!(tabs.len() < 6, "the bar overflows ({} tabs drawn)", tabs.len());
    assert!(last.index + 1 < 6, "tabs stay undrawn after the last drawn one");
    let bar = app.hits.groups.iter().find(|g| g.id == group).unwrap();
    assert!(bar.tab_arrows.is_some(), "an overflowing bar has arrows");
    let arrow_x = bar.tab_bar.x + bar.tab_bar.width - 1;

    let mut ordered = layout(&app);
    let before = ordered.remove(0);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    // Every drawn tab's midpoint is left of the arrow cell: the gap is the undrawn tab after
    // the last drawn one, whose marker belongs at that tab's end, not at the bar's start.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), arrow_x, y);
    let term = draw(&mut app, 41);
    assert_eq!(
        marker_cells(&app, &term, group, y),
        vec![arrow_x - 1],
        "the marker is at the last drawn tab's end"
    );

    app.mouse(MouseEventKind::Up(MouseButton::Left), arrow_x, y);
    let mut expected = before;
    let moved = expected.remove(0);
    expected.insert(last.index, moved);
    assert_eq!(layout(&app), [expected], "the drop lands after the last drawn tab");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_another_groups_bar_gets_no_marker_and_no_drop() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    fs::write(root.join("d.txt"), "d.txt 1\n").unwrap();
    app.test_execute(Command::SplitRight);
    // A second tab in the split's group, so a release there with the guard removed would move
    // one of its tabs (not be a no-op out of range).
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
    draw(&mut app, 120);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"], vec!["c.txt", "d.txt"]]);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (y, tabs) = bar_of(&app, src);
    let (other_y, other_tabs) = bar_of(&app, other);
    assert_eq!(other_y, y, "side by side: the bars share a row");
    assert_eq!(other_tabs.len(), 2, "the split's group got another tab");

    // Drag the source's second tab (index 1) towards a gap 0 in the other group.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    let term = draw(&mut app, 120);
    assert!(
        marker_cells(&app, &term, src, y).is_empty(),
        "no marker in the source bar"
    );
    assert!(
        marker_cells(&app, &term, other, other_y).is_empty(),
        "a drop on another group's bar moves nothing until drag_tabs-b: no marker"
    );

    // Back over its own bar the marker is there again: the drag is still live.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[1].x1, y);
    let term = draw(&mut app, 120);
    assert_eq!(
        marker_cells(&app, &term, src, y),
        vec![tabs[2].x0],
        "its own bar marks the gap"
    );

    // Released on the other bar: nothing moves (not even its own tabs).
    app.mouse(MouseEventKind::Drag(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"], vec!["c.txt", "d.txt"]]);
    let term = draw(&mut app, 120);
    assert!(
        marker_cells(&app, &term, src, y).is_empty(),
        "the release ended the drag"
    );
}
