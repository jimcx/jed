//! drag_tabs-a: dragging a tab along its bar reorders it, the drop marker follows the pointer,
//! a press that never leaves the tab is a click, and Esc or a release outside the bar moves
//! nothing. drag_tabs-b: a tab dropped on another group's bar moves there at the gap (an empty
//! source group closes) and dropped on another group's editor moves into that group, activated;
//! a shell moves rather than copies, and a divider, the side panel, the status bar or a banner
//! takes no drop. drag_tabs-c: a drop in an editor's edge band (its nearer quarter) splits that
//! group on that side and the tab lands in the new split, the band shaded while dragging (the
//! whole editor for a centre drop); its own group's edge splits it too, and a lone tab's own-edge
//! drop collapses back to one group.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::buffer::file::Stamp;
use crate::command::Command;
use crate::group::Tab;
use crate::layout::{DropSide, GroupId, MIN_GROUP_HEIGHT, MIN_GROUP_WIDTH, TabHit};
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

fn draw_sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

fn draw(app: &mut App, w: u16) -> Terminal<TestBackend> {
    draw_sized(app, w, 20)
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

/// `group`'s editor as the last frame drew it.
fn editor_of(app: &App, group: GroupId) -> Rect {
    app.hits
        .groups
        .iter()
        .find(|g| g.id == group)
        .expect("the group is drawn")
        .editor
}

/// A point in the middle of `rect`: no edge band reaches it.
fn centre(rect: Rect) -> (u16, u16) {
    (rect.x + rect.width / 2, rect.y + rect.height / 2)
}

/// The half of `rect` on `side`, as `tab_drop_shade` gives it.
fn half(rect: Rect, side: DropSide) -> Rect {
    match side {
        DropSide::Left => Rect {
            width: rect.width / 2,
            ..rect
        },
        DropSide::Right => Rect {
            x: rect.x + rect.width / 2,
            width: rect.width - rect.width / 2,
            ..rect
        },
        DropSide::Up => Rect {
            height: rect.height / 2,
            ..rect
        },
        DropSide::Down => Rect {
            y: rect.y + rect.height / 2,
            height: rect.height - rect.height / 2,
            ..rect
        },
    }
}

/// A point just inside `rect`'s edge on `side`.
fn edge_point(rect: Rect, side: DropSide) -> (u16, u16) {
    let (mid_x, mid_y) = centre(rect);
    match side {
        DropSide::Left => (rect.x, mid_y),
        DropSide::Right => (rect.x + rect.width - 1, mid_y),
        DropSide::Up => (mid_x, rect.y),
        DropSide::Down => (mid_x, rect.y + rect.height - 1),
    }
}

/// The number of cells in `rect` drawn with the drop shade's two colours.
fn shaded_cells(app: &App, term: &Terminal<TestBackend>, rect: Rect) -> usize {
    let style = app.theme.get("editor.dropBackground");
    let (fg, bg) = (style.fg.unwrap(), style.bg.unwrap());
    let mut n = 0;
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            let cell = &term.backend().buffer()[(x, y)];
            if cell.fg == fg && cell.bg == bg {
                n += 1;
            }
        }
    }
    n
}

/// The cells of `rect` (a whole area) that are shaded.
fn whole_area_is_shaded(app: &App, term: &Terminal<TestBackend>, rect: Rect) -> bool {
    shaded_cells(app, term, rect) == usize::from(rect.width) * usize::from(rect.height)
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

    // Over the editor there is no bar marker: the shade marks the zone there instead.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[0].x0 + 1, editor_y);
    let term = draw(&mut app, 80);
    assert!(
        marker_cells(&app, &term, first_group(&app), y).is_empty(),
        "no bar target: no marker"
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
    // Released over the status bar rather than the bar or an editor.
    let status = app.hits.regions.status;
    app.mouse(MouseEventKind::Up(MouseButton::Left), status.x + 2, status.y);
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
async fn drag_tabs_moves_a_tab_to_another_groups_bar_and_keeps_its_view() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    fs::write(root.join("d.txt"), "d.txt 1\n").unwrap();
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
    draw(&mut app, 120);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"], vec!["c.txt", "d.txt"]]);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (y, tabs) = bar_of(&app, src);
    let (other_y, other_tabs) = bar_of(&app, other);
    assert_eq!(other_y, y, "side by side: the bars share a row");

    // b.txt (the source's index 1) is scrolled down, with a selection.
    app.test_execute(Command::FocusGroup(1));
    app.test_activate_tab(1);
    for _ in 0..19 {
        app.press(K::Down, M::NONE);
    }
    app.press(K::Right, M::SHIFT);
    draw(&mut app, 120);
    let before = view_state(&app);
    assert!(before.1 > 0, "the view is scrolled");

    // Drag b.txt to the other bar's first gap: only that bar shows the marker.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    let term = draw(&mut app, 120);
    assert!(
        marker_cells(&app, &term, src, y).is_empty(),
        "the source bar has no marker"
    );
    assert_eq!(
        marker_cells(&app, &term, other, other_y),
        vec![other_tabs[0].x0],
        "the other bar marks the gap"
    );

    // Back over its own bar: the marker follows, and the other bar's clears.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[1].x1, y);
    let term = draw(&mut app, 120);
    assert!(
        marker_cells(&app, &term, other, other_y).is_empty(),
        "leaving the other bar clears its marker"
    );
    assert_eq!(marker_cells(&app, &term, src, y), vec![tabs[2].x0], "its own bar marks");
    app.mouse(MouseEventKind::Drag(MouseButton::Left), in_tab(&other_tabs[0]), other_y);

    app.mouse(MouseEventKind::Up(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    assert_eq!(
        layout(&app),
        [vec!["a.txt", "c.txt"], vec!["b.txt", "c.txt", "d.txt"]],
        "moved to the other bar's first gap"
    );
    assert_eq!(app.groups.active, other, "the target group is active");
    assert_eq!(active_name(&app), "b.txt");
    assert_eq!(view_state(&app), before, "its selections and scroll are kept");
    let term = draw(&mut app, 120);
    assert!(marker_cells(&app, &term, src, y).is_empty());
    assert!(
        marker_cells(&app, &term, other, other_y).is_empty(),
        "no marker once dropped"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_drops_at_a_middle_gap_in_another_groups_bar_and_moves_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    fs::write(root.join("d.txt"), "d.txt 1\n").unwrap();
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
    // b.txt in the source is scrolled with a selection.
    app.test_execute(Command::FocusGroup(1));
    app.test_activate_tab(1);
    for _ in 0..19 {
        app.press(K::Down, M::NONE);
    }
    app.press(K::Right, M::SHIFT);
    draw(&mut app, 120);
    let before = view_state(&app);
    assert!(before.1 > 0, "the view is scrolled");
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"], vec!["c.txt", "d.txt"]]);
    let (y, tabs) = bar_of(&app, src);
    let (other_y, other_tabs) = bar_of(&app, other);

    // Between c.txt and d.txt in the other bar: gap 1, not the start or the end.
    let x = other_tabs[0].x1 - 1;
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, other_y);
    let term = draw(&mut app, 120);
    assert_eq!(
        marker_cells(&app, &term, other, other_y),
        vec![other_tabs[1].x0],
        "the marker at the middle gap"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, other_y);
    assert_eq!(
        layout(&app),
        [vec!["a.txt", "c.txt"], vec!["c.txt", "b.txt", "d.txt"]],
        "moved to the gap between c.txt and d.txt"
    );
    assert_eq!(view_state(&app), before, "its view is kept");

    // A second hop: b.txt back into the source group's editor's centre, its view still its own.
    let editor = app.hits.groups.iter().find(|g| g.id == src).unwrap().editor;
    let (ox, oy) = centre(editor);
    let (other_y, other_tabs) = bar_of(&app, other);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&other_tabs[1]), other_y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), ox, oy);
    app.mouse(MouseEventKind::Up(MouseButton::Left), ox, oy);
    assert_eq!(
        layout(&app),
        [vec!["a.txt", "c.txt", "b.txt"], vec!["c.txt", "d.txt"]],
        "moved back right of the source's active tab"
    );
    assert_eq!(view_state(&app), before, "its view survives a second move");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_moving_a_groups_only_tab_out_closes_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"]);
    // The active b.txt moves into a new group, leaving a.txt alone in the first.
    app.test_execute(Command::MoveTabToNextGroup);
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    assert_eq!(layout(&app), [vec!["a.txt"], vec!["b.txt"]]);
    let (y, tabs) = bar_of(&app, src);
    let (other_y, other_tabs) = bar_of(&app, other);

    // Drop a.txt after b.txt in the other bar: the source group is left empty and closes.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), other_tabs[0].x1, other_y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), other_tabs[0].x1, other_y);

    assert_eq!(layout(&app), [vec!["b.txt", "a.txt"]], "the source group closed");
    assert_eq!((app.groups.len(), app.groups.ids()), (1, vec![other]));
    assert_eq!(app.groups.active, other);
    assert_eq!(active_name(&app), "a.txt", "the moved tab is active");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_dropping_on_another_groups_editor_moves_it_there_activated() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    fs::write(root.join("d.txt"), "d.txt 1\n").unwrap();
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (y, tabs) = bar_of(&app, src);
    // The source's a.txt is scrolled with a selection.
    app.test_execute(Command::FocusGroup(1));
    app.test_activate_tab(0);
    for _ in 0..19 {
        app.press(K::Down, M::NONE);
    }
    app.press(K::Right, M::SHIFT);
    draw(&mut app, 120);
    let before = view_state(&app);
    assert!(before.1 > 0, "the view is scrolled");
    // The target's active tab is c.txt, not its last: the drop must not land at the bar's end.
    app.test_execute(Command::FocusGroup(2));
    app.test_activate_tab(0);
    draw(&mut app, 120);
    let editor = app.hits.groups.iter().find(|g| g.id == other).unwrap().editor;
    assert!(
        !editor.contains(ratatui::layout::Position { x: tabs[0].x0, y }),
        "the editor is not the bar"
    );
    let (x, ey) = centre(editor);

    // a.txt to the other group's editor: into that group, right of its active tab.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    let term = draw(&mut app, 120);
    assert!(
        marker_cells(&app, &term, src, y).is_empty() && marker_cells(&app, &term, other, y).is_empty(),
        "an editor target shows no marker"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);

    assert_eq!(
        layout(&app),
        [vec!["b.txt", "c.txt"], vec!["c.txt", "a.txt", "d.txt"]],
        "moved into the target, right of its active tab (not the end)"
    );
    assert_eq!(app.groups.active, other, "the target group is active");
    assert_eq!(active_name(&app), "a.txt");
    assert_eq!(view_state(&app), before, "its selections and scroll are kept");

    // Released on the dragged tab's own editor: already there, so nothing moves.
    let (y, tabs) = bar_of(&app, other);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);
    assert_eq!(
        layout(&app),
        [vec!["b.txt", "c.txt"], vec!["c.txt", "a.txt", "d.txt"]],
        "a drop on its own group's editor moves nothing"
    );
    assert_eq!(active_name(&app), "c.txt", "the press's tab stays active");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_released_over_a_divider_side_panel_status_bar_or_banner_moves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    app.test_execute(Command::SplitRight);
    // A changed-on-disk banner over the split's editor row: it is not the editor underneath.
    let other = app.groups.ids()[1];
    let buffer = app.groups.get(other).unwrap().active_tab().unwrap().buffer;
    app.buffers.get_mut(buffer).unwrap().disk_conflict = Some(Stamp::of(b"changed on disk"));
    draw(&mut app, 120);
    let before = layout(&app);
    let groups = app.groups.ids();
    let y = bar_of(&app, groups[0]).0;
    let tabs = bar_of(&app, groups[0]).1;
    let divider = app.hits.dividers[0].rect;
    let side = app.hits.regions.side.unwrap();
    let status = app.hits.regions.status;
    let banner = app
        .hits
        .groups
        .iter()
        .find(|g| g.id == other)
        .and_then(|g| g.banner.clone())
        .expect("the split's editor shows a banner");
    let points = [
        (divider.x, divider.y + divider.height / 2),
        (side.x + side.width / 2, side.y + 3),
        (status.x + status.width / 2, status.y),
        (banner.row.x + banner.row.width - 1, banner.row.y),
    ];

    for (x, py) in points {
        app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), x, py);
        let term = draw(&mut app, 120);
        for &group in &groups {
            let gy = bar_of(&app, group).0;
            assert!(
                marker_cells(&app, &term, group, gy).is_empty(),
                "({x}, {py}) marks no gap"
            );
        }
        assert_eq!(
            shaded_cells(&app, &term, banner.row),
            0,
            "({x}, {py}) does not shade the banner"
        );
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, py);
        assert_eq!(layout(&app), before, "({x}, {py}) moves nothing");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_a_shell_tab_moves_rather_than_copies() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt"]);
    let program = root.join("myshell");
    std::os::unix::fs::symlink("/bin/sh", &program).unwrap();
    app.tasks.shell = program;
    app.test_execute(Command::NewShell);
    assert_eq!(app.group().tabs.len(), 2, "a.txt and the shell");
    // A split of a shell's group is left empty: the shell can only be in one group.
    app.test_execute(Command::SplitRight);
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    assert_eq!(layout(&app), [vec!["a.txt", "shell:myshell"], vec![]]);
    let run = app.groups.get(src).unwrap().active_shell().unwrap().run.id;
    let (y, tabs) = bar_of(&app, src);
    let editor = app.hits.groups.iter().find(|g| g.id == other).unwrap().editor;

    let (x, ey) = centre(editor);

    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);

    assert_eq!(layout(&app), [vec!["a.txt"], vec!["shell:myshell"]]);
    assert_eq!(app.groups.shell_tabs().count(), 1, "moved, not copied");
    assert_eq!(
        app.groups.shell_tabs().next().unwrap().run.id,
        run,
        "the same run, not a new shell"
    );
    assert_eq!(app.groups.active, other);
    assert!(
        app.group().active_shell().is_some(),
        "the shell is the target's active tab"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_edge_drops_split_the_group_on_that_side() {
    for side in [DropSide::Left, DropSide::Right, DropSide::Up, DropSide::Down] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
        fs::write(root.join("d.txt"), "d.txt 1\n").unwrap();
        app.test_execute(Command::SplitRight);
        app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
        // b.txt in the source is scrolled, with a selection.
        app.test_execute(Command::FocusGroup(1));
        app.test_activate_tab(1);
        for _ in 0..19 {
            app.press(K::Down, M::NONE);
        }
        app.press(K::Right, M::SHIFT);
        draw(&mut app, 120);
        let before = view_state(&app);
        assert!(before.1 > 0, "the view is scrolled");
        let ids = app.groups.ids();
        let (src, other) = (ids[0], ids[1]);
        assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"], vec!["c.txt", "d.txt"]]);
        let (y, tabs) = bar_of(&app, src);
        let editor = editor_of(&app, other);
        let (other_y, _) = bar_of(&app, other);

        // Drag b.txt into the target editor's band on `side`.
        app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
        let (x, ey) = edge_point(editor, side);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
        let term = draw(&mut app, 120);
        let band = half(editor, side);
        assert!(whole_area_is_shaded(&app, &term, band), "{side:?}: the band is shaded");
        let opposite = match side {
            DropSide::Left => DropSide::Right,
            DropSide::Right => DropSide::Left,
            DropSide::Up => DropSide::Down,
            DropSide::Down => DropSide::Up,
        };
        assert_eq!(
            shaded_cells(&app, &term, half(editor, opposite)),
            0,
            "{side:?}: the other half is not"
        );
        assert_eq!(
            shaded_cells(&app, &term, editor_of(&app, src)),
            0,
            "{side:?}: the source's editor is not"
        );
        assert!(
            marker_cells(&app, &term, src, y).is_empty() && marker_cells(&app, &term, other, other_y).is_empty(),
            "{side:?}: an editor target marks no gap"
        );
        app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);

        let first = matches!(side, DropSide::Left | DropSide::Up);
        let expected = if first {
            vec![vec!["a.txt", "c.txt"], vec!["b.txt"], vec!["c.txt", "d.txt"]]
        } else {
            vec![vec!["a.txt", "c.txt"], vec!["c.txt", "d.txt"], vec!["b.txt"]]
        };
        assert_eq!(layout(&app), expected, "{side:?}: split on that side");
        let new = *app
            .groups
            .ids()
            .iter()
            .find(|id| !ids.contains(id))
            .expect("a new group");
        assert_eq!(app.groups.active, new, "{side:?}: the new split is active");
        assert_eq!(active_name(&app), "b.txt");
        assert_eq!(view_state(&app), before, "{side:?}: its view is kept");
        let term = draw(&mut app, 120);
        for group in app.groups.ids() {
            assert_eq!(
                shaded_cells(&app, &term, editor_of(&app, group)),
                0,
                "{side:?}: no shade once dropped"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_own_edge_drop_splits_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    // b.txt is scrolled, with a selection.
    app.test_activate_tab(1);
    for _ in 0..19 {
        app.press(K::Down, M::NONE);
    }
    app.press(K::Right, M::SHIFT);
    draw(&mut app, 100);
    let before = view_state(&app);
    let group = first_group(&app);
    let (y, tabs) = bar_of(&app, group);
    let editor = editor_of(&app, group);
    let (x, ey) = edge_point(editor, DropSide::Left);

    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    let term = draw(&mut app, 100);
    assert!(
        whole_area_is_shaded(&app, &term, half(editor, DropSide::Left)),
        "its own left band is shaded"
    );
    assert_eq!(shaded_cells(&app, &term, half(editor, DropSide::Right)), 0);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);

    assert_eq!(
        layout(&app),
        [vec!["b.txt"], vec!["a.txt", "c.txt"]],
        "its own group split on the left"
    );
    let new = app.groups.ids()[0];
    assert_ne!(new, group, "the new group is on the left");
    assert_eq!(app.groups.active, new);
    assert_eq!(active_name(&app), "b.txt");
    assert_eq!(view_state(&app), before, "its view is kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_a_lone_tabs_own_edge_drop_leaves_one_group() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt"]);
    draw(&mut app, 80);
    let group = first_group(&app);
    let (y, tabs) = bar_of(&app, group);
    let editor = editor_of(&app, group);
    let (x, ey) = edge_point(editor, DropSide::Right);

    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    let term = draw(&mut app, 80);
    assert!(
        whole_area_is_shaded(&app, &term, half(editor, DropSide::Right)),
        "the right band is shaded"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);

    assert_eq!(layout(&app), [vec!["a.txt"]], "the empty source collapsed");
    assert_eq!((app.groups.len(), app.groups.ids().len()), (1, 1), "one group again");
    assert_eq!(active_name(&app), "a.txt");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_the_shade_marks_the_drop_zone_and_clears() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    fs::write(root.join("d.txt"), "d.txt 1\n").unwrap();
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (y, tabs) = bar_of(&app, src);
    let (other_y, other_tabs) = bar_of(&app, other);
    let src_editor = editor_of(&app, src);
    let other_editor = editor_of(&app, other);

    // An edge band: only that half.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    let (lx, ly) = edge_point(other_editor, DropSide::Left);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), lx, ly);
    let term = draw(&mut app, 120);
    assert!(
        whole_area_is_shaded(&app, &term, half(other_editor, DropSide::Left)),
        "the left band is shaded"
    );
    assert_eq!(
        shaded_cells(&app, &term, half(other_editor, DropSide::Right)),
        0,
        "the right half is not"
    );
    assert_eq!(shaded_cells(&app, &term, src_editor), 0, "the source's editor is not");
    let target_hit = app.hits.groups.iter().find(|g| g.id == other).unwrap();
    let scrollbar = target_hit.scrollbar.expect("a text tab has a bar").track;
    assert_eq!(
        shaded_cells(&app, &term, scrollbar),
        0,
        "the scroll bar column is not shaded"
    );
    assert_eq!(
        shaded_cells(&app, &term, target_hit.tab_bar),
        0,
        "the tab bar is not shaded"
    );
    for d in &app.hits.dividers {
        assert_eq!(shaded_cells(&app, &term, d.rect), 0, "a divider is not shaded");
    }

    // The centre: the whole editor.
    let (cx, cy) = centre(other_editor);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), cx, cy);
    let term = draw(&mut app, 120);
    assert!(
        whole_area_is_shaded(&app, &term, other_editor),
        "a centre drop shades the whole editor"
    );
    assert_eq!(shaded_cells(&app, &term, src_editor), 0);

    // A bar target: no shade, and the marker instead.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    let term = draw(&mut app, 120);
    assert_eq!(shaded_cells(&app, &term, src_editor), 0, "a bar target shades nothing");
    assert_eq!(shaded_cells(&app, &term, other_editor), 0);
    assert!(
        !marker_cells(&app, &term, other, other_y).is_empty(),
        "the bar shows its marker"
    );

    // No target at all (the status bar): the shade goes.
    let status = app.hits.regions.status;
    app.mouse(MouseEventKind::Drag(MouseButton::Left), status.x + 1, status.y);
    let term = draw(&mut app, 120);
    assert_eq!(
        shaded_cells(&app, &term, other_editor),
        0,
        "a no-target drag shades nothing"
    );

    // Esc: the shade goes.
    app.mouse(MouseEventKind::Drag(MouseButton::Left), lx, ly);
    app.press(K::Esc, M::NONE);
    let term = draw(&mut app, 120);
    assert_eq!(shaded_cells(&app, &term, other_editor), 0, "Esc clears the shade");

    // Released in the band: no shade is left either.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), lx, ly);
    let term = draw(&mut app, 120);
    assert!(whole_area_is_shaded(&app, &term, half(other_editor, DropSide::Left)));
    app.mouse(MouseEventKind::Up(MouseButton::Left), lx, ly);
    let term = draw(&mut app, 120);
    for group in app.groups.ids() {
        assert_eq!(
            shaded_cells(&app, &term, editor_of(&app, group)),
            0,
            "no shade once dropped"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_the_shade_stops_at_the_banners_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    app.test_execute(Command::SplitRight);
    // A changed-on-disk banner over the target group's first row: the editor starts below it.
    let other = app.groups.ids()[1];
    let buffer = app.groups.get(other).unwrap().active_tab().unwrap().buffer;
    app.buffers.get_mut(buffer).unwrap().disk_conflict = Some(Stamp::of(b"changed on disk"));
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (y, tabs) = bar_of(&app, src);
    let target = editor_of(&app, other);
    let banner = app
        .hits
        .groups
        .iter()
        .find(|g| g.id == other)
        .and_then(|g| g.banner.clone())
        .expect("the target shows a banner");
    assert_eq!(banner.row.y + banner.row.height, target.y, "the editor starts below it");

    // The left half of the editor below the banner is shaded; the banner's row is not.
    let (x, ey) = edge_point(target, DropSide::Left);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    let term = draw(&mut app, 120);
    assert!(
        whole_area_is_shaded(&app, &term, half(target, DropSide::Left)),
        "the editor's left half is shaded"
    );
    assert_eq!(shaded_cells(&app, &term, banner.row), 0, "the banner's row is not");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_done_when_reorder_move_split_esc_and_click() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    fs::write(root.join("d.txt"), "d.txt 1\n").unwrap();
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    assert_eq!(layout(&app), [vec!["a.txt", "b.txt", "c.txt"], vec!["c.txt", "d.txt"]]);

    // 1. Along its own bar: a.txt to the end.
    let (y, tabs) = bar_of(&app, src);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tabs[2].x1, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), tabs[2].x1, y);
    assert_eq!(
        layout(&app),
        [vec!["b.txt", "c.txt", "a.txt"], vec!["c.txt", "d.txt"]],
        "the bar reordered"
    );

    // 2. Onto the other group's bar at its first gap.
    draw(&mut app, 120);
    let (y, tabs) = bar_of(&app, src);
    let (other_y, other_tabs) = bar_of(&app, other);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), in_tab(&other_tabs[0]), other_y);
    assert_eq!(
        layout(&app),
        [vec!["c.txt", "a.txt"], vec!["b.txt", "c.txt", "d.txt"]],
        "the other bar took it"
    );

    // 3. Into the other group, at its centre.
    draw(&mut app, 120);
    let (y, tabs) = bar_of(&app, src);
    let (cx, cy) = centre(editor_of(&app, other));
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), cx, cy);
    app.mouse(MouseEventKind::Up(MouseButton::Left), cx, cy);
    assert_eq!(
        layout(&app),
        [vec!["c.txt"], vec!["b.txt", "a.txt", "c.txt", "d.txt"]],
        "the centre drop moved into the group"
    );

    // 4. The source's right edge: split, and d.txt lands in the new split.
    draw(&mut app, 120);
    let (other_y, other_tabs) = bar_of(&app, other);
    let target = editor_of(&app, src);
    let (rx, ry) = edge_point(target, DropSide::Right);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&other_tabs[3]), other_y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), rx, ry);
    let term = draw(&mut app, 120);
    assert!(
        whole_area_is_shaded(&app, &term, half(target, DropSide::Right)),
        "the right band is shaded"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), rx, ry);
    assert_eq!(
        layout(&app),
        [vec!["c.txt"], vec!["d.txt"], vec!["b.txt", "a.txt", "c.txt"]],
        "the edge drop split the group on the right"
    );
    assert_eq!(active_name(&app), "d.txt", "the new split's tab is active");

    // 5. Esc cancels a drag.
    draw(&mut app, 120);
    let (y, tabs) = bar_of(&app, src);
    let (ex, ey) = centre(editor_of(&app, other));
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), ex, ey);
    app.press(K::Esc, M::NONE);
    app.mouse(MouseEventKind::Up(MouseButton::Left), ex, ey);
    assert_eq!(
        layout(&app),
        [vec!["c.txt"], vec!["d.txt"], vec!["b.txt", "a.txt", "c.txt"]],
        "Esc left every group as it was"
    );

    // 6. A press and release on one tab is a click: it activates, it reorders nothing.
    let (other_y, other_tabs) = bar_of(&app, other);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&other_tabs[1]), other_y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), other_tabs[1].x1 - 1, other_y);
    assert_eq!(
        layout(&app),
        [vec!["c.txt"], vec!["d.txt"], vec!["b.txt", "a.txt", "c.txt"]],
        "a click reorders nothing"
    );
    assert_eq!(active_name(&app), "a.txt", "but activates the tab");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_a_pane_too_narrow_to_split_takes_a_centre_drop() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"]);
    app.test_execute(Command::SplitRight);
    draw(&mut app, 80);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let target = editor_of(&app, other);
    assert!(
        target.width <= 2 * MIN_GROUP_WIDTH,
        "the split pane is too narrow for two groups ({} wide)",
        target.width
    );

    // Its left edge is no edge: the drop is the group's centre move, not a split.
    let (y, tabs) = bar_of(&app, src);
    let (x, ey) = edge_point(target, DropSide::Left);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    let term = draw(&mut app, 80);
    assert!(
        whole_area_is_shaded(&app, &term, target),
        "the whole editor shades, not a band"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);
    assert_eq!(
        layout(&app),
        [vec!["b.txt"], vec!["b.txt", "a.txt"]],
        "the tab moved into the group"
    );
    assert_eq!(app.groups.ids().len(), 2, "no split was made");
    assert_eq!(active_name(&app), "a.txt");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_a_pane_too_short_to_split_takes_a_centre_drop() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"]);
    app.test_execute(Command::SplitDown);
    draw_sized(&mut app, 80, 10);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let target = editor_of(&app, other);
    assert!(
        target.height <= 2 * MIN_GROUP_HEIGHT,
        "the split pane is too short for two groups ({} rows)",
        target.height
    );

    // Its top edge is no edge either.
    let (y, tabs) = bar_of(&app, src);
    let (x, ey) = edge_point(target, DropSide::Up);
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    let term = draw_sized(&mut app, 80, 10);
    assert!(
        whole_area_is_shaded(&app, &term, target),
        "the whole editor shades, not a band"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);
    assert_eq!(
        layout(&app),
        [vec!["b.txt"], vec!["b.txt", "a.txt"]],
        "the tab moved into the group"
    );
    assert_eq!(app.groups.ids().len(), 2, "no split was made");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_a_tab_closed_mid_drag_leaves_no_empty_split() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"]);
    app.test_execute(Command::SplitRight);
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (y, tabs) = bar_of(&app, src);
    let (x, ey) = edge_point(editor_of(&app, other), DropSide::Left);

    // Only Esc ends a drag: a key can close the dragged tab while it is in flight.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[1]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    app.test_execute(Command::CloseTab);
    draw(&mut app, 120);
    assert_eq!(layout(&app), [vec!["a.txt"], vec!["b.txt"]], "the dragged tab closed");

    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);
    assert_eq!(layout(&app), [vec!["a.txt"], vec!["b.txt"]], "and nothing moved");
    assert_eq!(app.groups.ids(), ids, "no empty split left behind");
    assert_eq!(active_name(&app), "a.txt", "the source's remaining tab stays active");
}

#[tokio::test(flavor = "multi_thread")]
async fn drag_tabs_a_closed_drag_never_moves_its_neighbour() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"]);
    app.test_execute(Command::SplitRight);
    draw(&mut app, 120);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (y, tabs) = bar_of(&app, src);
    let (x, ey) = edge_point(editor_of(&app, other), DropSide::Left);

    // Start the drag on the first tab, then close it: the tab now at index 0 is its neighbour
    // (the stale index is still valid), and must not be taken for the dragged one.
    app.mouse(MouseEventKind::Down(MouseButton::Left), in_tab(&tabs[0]), y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, ey);
    app.test_execute(Command::CloseTab);
    draw(&mut app, 120);
    assert_eq!(
        layout(&app),
        [vec!["b.txt", "c.txt"], vec!["c.txt"]],
        "the dragged tab closed"
    );

    app.mouse(MouseEventKind::Up(MouseButton::Left), x, ey);
    assert_eq!(
        layout(&app),
        [vec!["b.txt", "c.txt"], vec!["c.txt"]],
        "its neighbour stayed where it was"
    );
    assert_eq!(app.groups.ids(), ids, "no split and no group closed");
}
