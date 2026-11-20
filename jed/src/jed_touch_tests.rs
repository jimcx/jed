//! jed_touch-a, "Done when": with `--touch` (`App::touch_flag`) or `touch_layout`, every group's tab
//! bar (editor and terminal area, top and bottom) is 3 rows with padded tabs, the label, `×` and
//! group number on the middle row and the drop marker across all rows; tapping any row of a tab
//! focuses it on release, the close range (3 cells) closes it, the arrows and a swipe scroll an
//! overflowing bar, the `+` opens a tab in its group, a held tab (the hold event, and once the real
//! timer) is drawn lifted and drags to another group and onto a split edge; the middle click, the
//! right click and the wheel work on the pad rows; shells and editors are sized to the area left
//! over; the setting turns the layout on and off with a settings reload; with neither set nothing
//! changes.
//!
//! jed_touch-b, "Done when" (the `jed_touch_b_*` tests): a swipe in an editor, a diff, a hex view and a
//! shell scrolls it without selecting (a shell that reports the mouse gets wheel events), a tap moves
//! the cursor or caret, a hold then drag selects, double- and triple-tap select a word and a line, a
//! hold released in place opens the context menu on the editor, an Explorer row and a tab; with the
//! layout off, presses behave as today.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, InitialFile};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::config::settings;
use crate::drag_tabs_tests::edge_point;
use crate::group::Tab;
use crate::layout::{DropSide, GroupId, Hit, TabHit};
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

const DOWN: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
const DRAG: MouseEventKind = MouseEventKind::Drag(MouseButton::Left);
const UP: MouseEventKind = MouseEventKind::Up(MouseButton::Left);

/// An app on a workspace holding `names`, the first open and the others opened after it in the
/// same group; `touch` is `--touch`.
fn open_app(dir: &tempfile::TempDir, names: &[&str], touch: bool) -> (App, PathBuf) {
    let root = workspace::normalize(dir.path());
    for name in names {
        fs::write(root.join(name), format!("{name} 1\n{name} 2\n")).unwrap();
    }
    let buffer = Buffer::open(&root.join(names[0])).unwrap();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.touch_flag = touch;
    app.test_start();
    for name in &names[1..] {
        app.test_open(Buffer::open(&root.join(name)).unwrap());
    }
    (app, root)
}

fn draw_sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

fn draw(app: &mut App) -> Terminal<TestBackend> {
    draw_sized(app, 120, 30)
}

fn row_text(term: &Terminal<TestBackend>, y: u16, x0: u16, x1: u16) -> String {
    (x0..x1).map(|x| term.backend().buffer()[(x, y)].symbol()).collect()
}

fn bar_of(app: &App, group: GroupId) -> (Rect, Vec<TabHit>) {
    let g = app.hits.group(group).expect("the group is drawn");
    (g.tab_bar, g.tabs.clone())
}

fn names(app: &App, group: GroupId) -> Vec<String> {
    app.groups
        .get(group)
        .unwrap()
        .tabs
        .iter()
        .map(|t| match t {
            Tab::Text(t) => app.buffers.get(t.buffer).unwrap().name(),
            _ => "other".into(),
        })
        .collect()
}

fn active_name(app: &App) -> String {
    app.active_buffer().unwrap().name()
}

fn tap(app: &mut App, x: u16, y: u16) {
    app.mouse(DOWN, x, y);
    app.mouse(UP, x, y);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_tab_bar_is_three_rows_with_padded_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    assert!(app.touch());
    let term = draw(&mut app);
    let (bar, tabs) = bar_of(&app, app.groups.active);
    assert_eq!(bar.height, 3);
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    assert_eq!(editor.y, bar.y + 3, "the editor starts under the bar");
    // `  a.txt  ×  `: label on the middle row, two cells of padding, `×` two cells from the end.
    let t = tabs[0];
    assert_eq!(t.x1 - t.x0, 12);
    assert_eq!(row_text(&term, bar.y + 1, t.x0, t.x1), "  a.txt  ×  ", "the label row");
    for y in [bar.y, bar.y + 2] {
        assert_eq!(row_text(&term, y, t.x0, t.x1).trim(), "", "pad row {y}");
    }
    // Every row of the tab carries its background (b.txt, the active one, differs from a.txt).
    let bg = |x: u16, y: u16| term.backend().buffer()[(x, y)].bg;
    for y in bar.y..bar.y + 3 {
        assert_eq!(bg(tabs[1].x0, y), app.theme.get("tab.active").bg.unwrap(), "row {y}");
        assert_eq!(bg(tabs[0].x0, y), app.theme.get("tab.inactive").bg.unwrap(), "row {y}");
    }
    // The `+` ends the bar.
    let plus = app
        .hits
        .group(app.groups.active)
        .unwrap()
        .new_tab
        .clone()
        .expect("a + target");
    assert_eq!(plus.end - plus.start, 3);
    assert_eq!(row_text(&term, bar.y + 1, plus.start, plus.end), " + ");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_nothing_changes_without_touch() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], false);
    assert!(!app.touch());
    let term = draw(&mut app);
    let g = app.hits.group(app.groups.active).unwrap().clone();
    assert_eq!(g.tab_bar.height, 1);
    assert!(g.new_tab.is_none());
    let t = g.tabs[0];
    assert_eq!(row_text(&term, g.tab_bar.y, t.x0, t.x1), " a.txt × ");
    assert_eq!((t.close_x0, t.close_x1), (t.x0 + 7, t.x0 + 8));
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_tabs_at_the_bottom_and_two_groups() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    app.settings.tabs_at_bottom = true;
    app.test_execute(Command::SplitRight);
    let term = draw(&mut app);
    for id in app.groups.ids() {
        let g = app.hits.group(id).unwrap();
        assert_eq!(g.tab_bar.height, 3);
        assert_eq!(
            g.editor.y + g.editor.height,
            g.tab_bar.y,
            "the bar is right under the editor"
        );
    }
    // The group number sits on the label row at the bar's end.
    let (bar, _) = bar_of(&app, app.groups.ids()[0]);
    let last = bar.x + bar.width - 1;
    assert_eq!(row_text(&term, bar.y + 1, last, last + 1), "1");
    assert_eq!(row_text(&term, bar.y, last, last + 1), " ");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_tapping_any_row_focuses_on_release() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"], true);
    draw(&mut app);
    let (bar, tabs) = bar_of(&app, app.groups.active);
    assert_eq!(active_name(&app), "c.txt");
    for y in bar.y..bar.y + 3 {
        for (i, name) in [(0, "a.txt"), (1, "b.txt")] {
            let x = tabs[i].x0 + 1;
            app.mouse(DOWN, x, y);
            assert_eq!(active_name(&app), "c.txt", "a press alone acts on nothing (row {y})");
            app.mouse(UP, x, y);
            assert_eq!(active_name(&app), name, "row {y}");
            draw(&mut app);
            tap(&mut app, tabs[2].x0 + 1, y);
            assert_eq!(active_name(&app), "c.txt");
            draw(&mut app);
        }
    }
    // Jitter within a cell is still a tap.
    app.mouse(DOWN, tabs[0].x0 + 3, bar.y + 1);
    app.mouse(DRAG, tabs[0].x0 + 4, bar.y + 1);
    app.mouse(UP, tabs[0].x0 + 4, bar.y + 1);
    assert_eq!(active_name(&app), "a.txt");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_the_close_range_is_three_cells_on_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt", "c.txt"], true);
    draw(&mut app);
    let id = app.groups.active;
    let (bar, tabs) = bar_of(&app, id);
    let t = tabs[1];
    assert_eq!(t.close_x1 - t.close_x0, 3);
    assert_eq!(t.close_x0 + 1, t.x1 - 3, "the `×` is the middle of its target");
    for y in bar.y..bar.y + 3 {
        for x in t.close_x0..t.close_x1 {
            assert_eq!(app.hits.hit(x, y), Hit::TabClose(id, 1), "({x},{y})");
        }
        assert_eq!(app.hits.hit(t.close_x0 - 1, y), Hit::Tab(id, 1));
        assert_eq!(app.hits.hit(t.close_x1, y), Hit::Tab(id, 1));
    }
    // A tap on the cell beside the `×` closes the tab.
    tap(&mut app, t.close_x0, bar.y + 2);
    assert_eq!(names(&app, id), ["a.txt", "c.txt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_closing_an_unsaved_tab_asks() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    app.type_text("x");
    let term = draw(&mut app);
    let id = app.groups.active;
    let (bar, tabs) = bar_of(&app, id);
    let t = tabs[1];
    let marker = t.close_x0 + 1;
    assert_eq!(row_text(&term, bar.y + 1, marker, marker + 1), "●");
    tap(&mut app, marker, bar.y);
    assert!(app.prompt.is_some(), "the prompt asks first");
    assert_eq!(names(&app, id), ["a.txt", "b.txt"], "nothing closed yet");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_arrows_and_a_swipe_scroll_an_overflowing_bar() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        "file1.txt",
        "file2.txt",
        "file3.txt",
        "file4.txt",
        "file5.txt",
        "file6.txt",
    ];
    let (mut app, _) = open_app(&dir, &files, true);
    app.test_execute(Command::ToggleSidebar);
    draw_sized(&mut app, 60, 20);
    let id = app.groups.active;
    let g = app.hits.group(id).unwrap().clone();
    let arrows = g.tab_arrows.clone().expect("six tabs overflow 60 cells");
    assert_eq!((arrows.back.len(), arrows.forward.len()), (3, 3));
    let y = g.tab_bar.y;
    for row in 0..3 {
        assert_eq!(app.hits.hit(arrows.back.start, y + row), Hit::TabArrow(id, false));
        assert_eq!(app.hits.hit(arrows.forward.end - 1, y + row), Hit::TabArrow(id, true));
    }
    let term = draw_sized(&mut app, 60, 20);
    assert_eq!(
        row_text(&term, y + 1, arrows.back.start + 1, arrows.back.start + 2),
        "‹"
    );
    assert_eq!(
        row_text(&term, y + 1, arrows.forward.start + 1, arrows.forward.start + 2),
        "›"
    );

    // The active tab is the last: scroll back with the arrow, a tap acting on release.
    let before = app.groups.get(id).unwrap().scroll;
    assert!(before > 0);
    app.mouse(DOWN, arrows.back.start, y + 2);
    assert_eq!(app.groups.get(id).unwrap().scroll, before);
    app.mouse(UP, arrows.back.start, y + 2);
    assert_eq!(app.groups.get(id).unwrap().scroll, before - 1);
    draw_sized(&mut app, 60, 20);

    // A swipe left scrolls by whole tabs towards the end and focuses nothing.
    let active = active_name(&app);
    let start = app.groups.get(id).unwrap().scroll;
    let (x, row) = (arrows.back.end + 20, y + 1);
    app.mouse(DOWN, x, row);
    app.mouse(DRAG, x - 10, row);
    app.mouse(DRAG, x - 20, row);
    app.mouse(UP, x - 20, row);
    assert!(
        app.groups.get(id).unwrap().scroll > start,
        "the tabs followed the finger"
    );
    assert_eq!(active_name(&app), active, "a swipe is no tap");
    draw_sized(&mut app, 60, 20);
    // And right brings the first ones back.
    app.mouse(DOWN, x - 20, row);
    app.mouse(DRAG, x, row);
    app.mouse(DRAG, x + 30, row);
    app.mouse(UP, x + 30, row);
    assert_eq!(app.groups.get(id).unwrap().scroll, 0);
    assert!(!app.press_pending() && !app.dragging_tab());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_plus_opens_a_tab_in_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt"], true);
    app.test_execute(Command::SplitRight);
    draw(&mut app);
    let ids = app.groups.ids();
    let g = app.hits.group(ids[0]).unwrap().clone();
    let plus = g.new_tab.clone().unwrap();
    assert_eq!(app.groups.active, ids[1]);
    for y in g.tab_bar.y..g.tab_bar.y + 3 {
        assert_eq!(app.hits.hit(plus.start + 1, y), Hit::NewTab(ids[0]));
    }
    let before = names(&app, ids[0]).len();
    tap(&mut app, plus.start + 1, g.tab_bar.y);
    assert_eq!(app.groups.active, ids[0], "the group took the focus");
    assert_eq!(names(&app, ids[0]).len(), before + 1, "a new tab in that group");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_a_held_tab_is_lifted_and_drops_in_another_group() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt", "c.txt"], true);
    fs::write(root.join("d.txt"), "d\n").unwrap();
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
    draw(&mut app);
    let ids = app.groups.ids();
    let (src, other) = (ids[0], ids[1]);
    let (bar, tabs) = bar_of(&app, src);
    let (obar, otabs) = bar_of(&app, other);

    // A hold lifts the tab (drawn `tab.lifted`).
    app.mouse(DOWN, tabs[0].x0 + 3, bar.y + 2);
    assert!(app.press_pending(), "a press is pending");
    app.test_hold();
    assert!(app.dragging_tab());
    let term = draw(&mut app);
    let lifted = app.theme.get("tab.lifted").bg.unwrap();
    for y in bar.y..bar.y + 3 {
        assert_eq!(term.backend().buffer()[(tabs[0].x0, y)].bg, lifted, "row {y}");
    }

    // Over the other bar: the drop marker spans all three rows.
    app.mouse(DRAG, otabs[0].x0 + 1, obar.y + 2);
    let term = draw(&mut app);
    let marker = app.theme.get("tab.dropMarker").bg.unwrap();
    for y in obar.y..obar.y + 3 {
        assert_eq!(term.backend().buffer()[(otabs[0].x0, y)].bg, marker, "row {y}");
    }
    app.mouse(UP, otabs[0].x0 + 1, obar.y + 2);
    assert_eq!(names(&app, src), ["b.txt", "c.txt"]);
    assert_eq!(names(&app, other), ["a.txt", "c.txt", "d.txt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_a_held_tab_drops_on_a_split_edge() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    draw(&mut app);
    let id = app.groups.active;
    let (bar, tabs) = bar_of(&app, id);
    let editor = app.hits.group(id).unwrap().editor;
    app.mouse(DOWN, tabs[0].x0 + 3, bar.y + 1);
    assert!(app.press_pending());
    app.test_hold();
    let (x, y) = edge_point(editor, DropSide::Right);
    app.mouse(DRAG, x, y);
    app.mouse(UP, x, y);
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 2, "the edge drop split the group");
    assert_eq!(names(&app, ids[0]), ["b.txt"]);
    assert_eq!(names(&app, ids[1]), ["a.txt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_the_real_hold_timer_lifts_the_tab() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    draw(&mut app);
    let (bar, tabs) = bar_of(&app, app.groups.active);
    app.mouse(DOWN, tabs[0].x0 + 3, bar.y + 1);
    assert!(!app.dragging_tab());
    assert!(app.settle(|a| a.dragging_tab()).await, "the timer lifted the tab");
    app.mouse(UP, tabs[0].x0 + 3, bar.y + 1);
    assert_eq!(
        names(&app, app.groups.active),
        ["a.txt", "b.txt"],
        "dropped where it was"
    );
    assert!(!app.press_pending() && !app.dragging_tab());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_middle_right_click_and_the_wheel_work_on_the_pad_rows() {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        "file1.txt",
        "file2.txt",
        "file3.txt",
        "file4.txt",
        "file5.txt",
        "file6.txt",
    ];
    let (mut app, _) = open_app(&dir, &files, true);
    app.test_execute(Command::ToggleSidebar);
    draw_sized(&mut app, 60, 20);
    let id = app.groups.active;
    let (bar, tabs) = bar_of(&app, id);
    let t = tabs[1];
    // The wheel on the top pad row.
    let scroll = app.groups.get(id).unwrap().scroll;
    app.mouse(MouseEventKind::ScrollUp, t.x0 + 3, bar.y);
    assert_eq!(app.groups.get(id).unwrap().scroll, scroll.saturating_sub(1));
    // A right click on the bottom pad row opens the tab menu.
    app.mouse(MouseEventKind::Down(MouseButton::Right), t.x0 + 3, bar.y + 2);
    assert!(app.context_menu.is_some());
    app.mouse(MouseEventKind::Up(MouseButton::Right), t.x0 + 3, bar.y + 2);
    app.press(crossterm::event::KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
    assert!(app.context_menu.is_none());
    draw_sized(&mut app, 60, 20);
    // Middle click on a pad row closes the tab.
    let (bar, tabs) = bar_of(&app, id);
    let count = names(&app, id).len();
    app.mouse(MouseEventKind::Down(MouseButton::Middle), tabs[1].x0 + 3, bar.y + 2);
    assert_eq!(names(&app, id).len(), count - 1);
}

/// A workspace app with a user `settings.toml` (`[settings]` + `user`) in a config dir, watched.
fn open_with_settings(dir: &tempfile::TempDir, user: &str) -> (App, PathBuf) {
    let base = workspace::normalize(dir.path());
    let config_dir = base.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    let user_file = config_dir.join(settings::FILE_NAME);
    fs::write(&user_file, format!("[settings]\n{user}")).unwrap();
    let root = base.join("ws");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let loaded = settings::load(&user_file);
    let ws = Workspace::load(root.clone()).with_user_settings(loaded.overrides);
    let trust = TrustStore::load(base.join("trust.toml"));
    let initial = InitialFile {
        buffer: Buffer::open(&root.join("a.txt")).unwrap(),
        goto: None,
    };
    let mut app = App::new(ws, trust, Some(initial), true);
    app.config_dir = config_dir;
    app.enable_file_watching();
    app.test_start();
    (app, user_file)
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_the_setting_applies_on_reload_and_the_flag_wins() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, user_file) = open_with_settings(&dir, "touch_layout = true\n");
    assert!(app.touch());
    draw(&mut app);
    assert_eq!(app.hits.group(app.groups.active).unwrap().tab_bar.height, 3);

    fs::write(&user_file, "[settings]\ntouch_layout = false\n").unwrap();
    assert!(app.settle(|a| !a.settings.touch_layout).await, "the reload arrived");
    draw(&mut app);
    assert_eq!(app.hits.group(app.groups.active).unwrap().tab_bar.height, 1);

    fs::write(&user_file, "[settings]\ntouch_layout = true\n").unwrap();
    assert!(app.settle(|a| a.settings.touch_layout).await);
    draw(&mut app);
    assert_eq!(app.hits.group(app.groups.active).unwrap().tab_bar.height, 3);

    // `--touch` stays on whatever the file says.
    app.touch_flag = true;
    fs::write(&user_file, "[settings]\ntouch_layout = false\n").unwrap();
    assert!(app.settle(|a| !a.settings.touch_layout).await);
    assert!(app.touch());
    draw(&mut app);
    assert_eq!(app.hits.group(app.groups.active).unwrap().tab_bar.height, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_a_shell_is_sized_to_the_area_left_over() {
    use crate::shell_tests::{dirs, shell_app};
    let size = |a: &App| {
        let shell = a.terminals.active_group().active_shell().unwrap();
        crate::tasks::select::size(shell.run.screen().screen())
    };
    let mut opened = Vec::new();
    for touch in [false, true] {
        let (_d, root, state) = dirs();
        let mut app = shell_app(&root, &state);
        app.touch_flag = touch;
        draw_sized(&mut app, 100, 30);
        app.test_execute(Command::NewShell);
        opened.push(size(&app));
        // Before any frame draws it there, the PTY is already the size the frame gives the area.
        let term = draw_sized(&mut app, 100, 30);
        let g = app.hits.group(app.terminals.active).unwrap();
        assert_eq!(g.tab_bar.height, if touch { 3 } else { 1 });
        assert_eq!(opened.last().copied().unwrap(), (g.editor.height, g.editor.width));
        assert_eq!(
            size(&app),
            (g.editor.height, g.editor.width),
            "the frame resized nothing"
        );
        if touch {
            // The terminal area's bar is padded too, `×` on the middle row.
            let t = g.tabs[0];
            assert!(row_text(&term, g.tab_bar.y + 1, t.x0, t.x1).contains('×'));
        }
    }
    assert_eq!(opened[0].0 - opened[1].0, 2, "two rows less");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_a_the_tab_tip_sits_under_the_taller_bar() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    let term = draw(&mut app);
    let (bar, tabs) = bar_of(&app, app.groups.active);
    app.mouse(MouseEventKind::Moved, tabs[0].x0 + 3, bar.y + 1);
    app.test_tab_tip_tick(std::time::Instant::now() + std::time::Duration::from_secs(2));
    let term2 = draw(&mut app);
    let _ = term;
    let below = row_text(&term2, bar.y + 3, tabs[0].x0, tabs[0].x0 + 8);
    assert!(below.contains("a.txt"), "the path is drawn under the bar: {below:?}");
}

// ---- jed_touch-b: gestures in text, diffs, hex views and shells; the hold menus ----

/// A touch app on `big.txt` (100 lines, `word{i} beta`) drawn at 120x30; the first text cell.
fn big_app(dir: &tempfile::TempDir, touch: bool) -> (App, u16, u16) {
    let (mut app, root) = open_app(dir, &["a.txt"], touch);
    let text: String = (0..100).map(|i| format!("word{i} beta\n")).collect();
    fs::write(root.join("big.txt"), &text).unwrap();
    app.test_open(Buffer::open(&root.join("big.txt")).unwrap());
    draw(&mut app);
    let t = app.hits.group(app.groups.active).unwrap().text;
    (app, t.x, t.y)
}

fn top_line(app: &App) -> usize {
    app.group().active_tab().unwrap().view.top_line()
}

fn head(app: &App) -> usize {
    app.group().active_tab().unwrap().view.selections.primary().head.0
}

fn selected(app: &mut App) -> String {
    app.test_execute(Command::Copy);
    app.clipboard.read().unwrap()
}

/// Line `n` (`word{n} beta\n`) starts at this byte, for the lines under 10.
fn byte_of(line: usize, col: usize) -> usize {
    line * 11 + col
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_swipe_scrolls_an_editor_without_selecting() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, x, y) = big_app(&dir, true);
    let before = head(&app);
    app.mouse(DOWN, x + 3, y + 10);
    // Finger moving up five rows brings five later lines in.
    app.mouse(DRAG, x + 3, y + 5);
    app.mouse(UP, x + 3, y + 5);
    assert_eq!(top_line(&app), 5);
    assert_eq!(head(&app), before, "no cursor move");
    assert!(app.group().active_tab().unwrap().view.selections.primary().is_empty());
    // Moving down goes back.
    app.mouse(DOWN, x + 3, y + 5);
    app.mouse(DRAG, x + 3, y + 7);
    app.mouse(UP, x + 3, y + 7);
    assert_eq!(top_line(&app), 3);
    assert!(!app.press_pending());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_a_sideways_swipe_scrolls_long_lines_in_the_fingers_direction() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt"], true);
    let long: String = (0..30).map(|_| format!("{}\n", "x".repeat(300))).collect();
    fs::write(root.join("long.txt"), &long).unwrap();
    app.test_open(Buffer::open(&root.join("long.txt")).unwrap());
    draw(&mut app);
    let t = app.hits.group(app.groups.active).unwrap().text;
    let left = |app: &App| app.group().active_tab().unwrap().view.left_col;
    assert_eq!(left(&app), 0);
    // The finger moving left 7 cells brings 7 later columns in; the row stays.
    app.mouse(DOWN, x_of(t, 20), t.y + 2);
    app.mouse(DRAG, x_of(t, 13), t.y + 2);
    app.mouse(UP, x_of(t, 13), t.y + 2);
    assert_eq!(left(&app), 7);
    assert_eq!(top_line(&app), 0);
    // Moving right 3 cells goes back 3.
    app.mouse(DOWN, x_of(t, 13), t.y + 2);
    app.mouse(DRAG, x_of(t, 16), t.y + 2);
    app.mouse(UP, x_of(t, 16), t.y + 2);
    assert_eq!(left(&app), 4);
    assert!(app.group().active_tab().unwrap().view.selections.primary().is_empty());
}

fn x_of(t: Rect, dx: u16) -> u16 {
    t.x + dx
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_tap_moves_the_cursor_and_double_and_triple_tap_select() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, x, y) = big_app(&dir, true);
    tap(&mut app, x + 3, y + 2);
    assert_eq!(head(&app), byte_of(2, 3));
    tap(&mut app, x + 3, y + 2);
    assert_eq!(selected(&mut app), "word2");
    tap(&mut app, x + 3, y + 2);
    assert_eq!(selected(&mut app), "word2 beta\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_hold_then_drag_selects_from_the_press() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, x, y) = big_app(&dir, true);
    app.mouse(DOWN, x + 1, y + 1);
    assert!(app.press_pending(), "a pending press");
    // Nothing is selected until the hold.
    app.mouse(DRAG, x + 1, y + 1);
    app.test_hold();
    app.mouse(DRAG, x + 4, y + 3);
    app.mouse(UP, x + 4, y + 3);
    assert_eq!(selected(&mut app), "ord1 beta\nword2 beta\nword");
    assert!(app.context_menu.is_none(), "a drag opens no menu");
    assert!(!app.press_pending());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_hold_released_in_place_opens_the_editor_menu() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, x, y) = big_app(&dir, true);
    app.mouse(DOWN, x + 1, y + 1);
    assert!(app.press_pending());
    app.test_hold();
    assert!(app.context_menu.is_none());
    app.mouse(UP, x + 1, y + 1);
    assert!(app.context_menu.is_some());
    // A stale hold (the press ended) does nothing.
    app.context_menu = None;
    app.test_hold();
    assert!(app.context_menu.is_none() && !app.press_pending());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_a_hold_released_elsewhere_or_after_esc_opens_no_menu() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, x, y) = big_app(&dir, true);
    // The terminal sent no motion, but the release is in another cell: a drag, not a menu.
    app.mouse(DOWN, x + 1, y + 1);
    app.test_hold();
    app.mouse(UP, x + 4, y + 1);
    assert!(app.context_menu.is_none());
    // Esc ends the hold's menu.
    app.mouse(DOWN, x + 1, y + 1);
    app.test_hold();
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    app.mouse(UP, x + 1, y + 1);
    assert!(app.context_menu.is_none());
    // A new press while a hold is recorded forgets it.
    app.mouse(DOWN, x + 1, y + 1);
    app.test_hold();
    app.mouse(DOWN, x + 2, y + 2);
    app.mouse(UP, x + 2, y + 2);
    assert!(app.context_menu.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_the_real_timer_turns_a_press_on_text_into_a_hold() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, x, y) = big_app(&dir, true);
    app.mouse(DOWN, x + 1, y + 1);
    assert!(app.press_pending());
    assert!(app.settle(|a| !a.press_pending()).await);
    app.mouse(UP, x + 1, y + 1);
    assert!(app.context_menu.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_off_a_press_selects_at_once_and_a_hold_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, x, y) = big_app(&dir, false);
    app.mouse(DOWN, x + 1, y + 1);
    assert!(!app.press_pending());
    app.mouse(DRAG, x + 4, y + 1);
    app.mouse(UP, x + 4, y + 1);
    assert_eq!(selected(&mut app), "ord");
    assert_eq!(top_line(&app), 0);
    app.mouse(DOWN, x + 1, y + 1);
    app.mouse(UP, x + 1, y + 1);
    assert!(app.context_menu.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_a_held_tab_released_in_place_opens_its_menu_and_a_drag_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    draw(&mut app);
    let (bar, tabs) = bar_of(&app, app.groups.active);
    let (x, y) = (tabs[0].x0 + 3, bar.y + 1);
    let g = app.groups.active;
    let order = |app: &App| -> Vec<_> { app.group().tabs.iter().map(|t| t.buffer()).collect() };
    let (before, groups) = (order(&app), app.groups.ids());
    app.mouse(DOWN, x, y);
    assert!(app.press_pending());
    app.test_hold();
    assert!(app.tab_lifted_in(g).is_some());
    app.mouse(UP, x, y);
    assert!(app.context_menu.is_some(), "released without moving");
    assert!(app.tab_lifted_in(g).is_none() && !app.dragging_tab());
    assert_eq!(
        (order(&app), app.groups.ids(), app.groups.active),
        (before, groups, g),
        "nothing moved"
    );
    app.context_menu = None;
    app.mouse(DOWN, x, y);
    assert!(app.press_pending());
    app.test_hold();
    app.mouse(DRAG, tabs[1].x0 + 3, y);
    app.mouse(UP, tabs[1].x0 + 3, y);
    assert!(app.context_menu.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_a_held_explorer_folder_runs_find_in_folder() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(workspace::normalize(dir.path()).join("sub")).unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt"], true);
    app.ui.side_visible = true;
    app.ui.side_panel = crate::layout::SidePanel::Explorer;
    assert!(app.settle(|a| a.explorer.rows().len() >= 2).await, "explorer listed");
    draw(&mut app);
    let (x, y) = (0..30)
        .flat_map(|y| (0..30).map(move |x| (x, y)))
        .find(|&(x, y)| matches!(app.hits.hit(x, y), Hit::ExplorerRow(_)))
        .expect("an explorer row");
    app.mouse(DOWN, x + 2, y);
    assert!(app.press_pending());
    assert_eq!(app.ui.side_panel, crate::layout::SidePanel::Explorer);
    app.test_hold();
    app.mouse(UP, x + 2, y);
    assert_eq!(app.ui.side_panel, crate::layout::SidePanel::Search, "Find in Folder");
}

/// A touch app with a fake shell in the editor area (`seq`'s output scrolled off, or the mouse
/// reported to `cat`), drawn; the shell's cell area.
async fn shell_touch_app(root: &std::path::Path, state: &std::path::Path, reports: bool) -> (App, Rect) {
    let mut app = crate::shell_tests::shell_app(root, state);
    app.touch_flag = true;
    app.tasks.shell = jterm::testing::fake_shell();
    app.new_shell_in_editor();
    assert!(
        app.settle(|a| !crate::shell_tests::screen_text(a).trim().is_empty())
            .await,
        "the shell printed its prompt"
    );
    let line = if reports {
        "seq 0 199; printf '\\033[?1000h\\033[?1006h'; cat\n"
    } else {
        "seq 0 199\n"
    };
    assert!(app.group().active_shell().unwrap().run.write(line.as_bytes()));
    assert!(
        app.settle(|a| {
            let run = &a.group().active_shell().unwrap().run;
            crate::shell_tests::screen_text(a).contains("199")
                && (!reports || run.screen().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
        })
        .await,
        "the output arrived:\n{}",
        crate::shell_tests::screen_text(&app)
    );
    drop(draw(&mut app));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    (app, area)
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_swipe_scrolls_a_shells_scrollback_and_selects_nothing() {
    let (_d, root, state) = crate::shell_tests::dirs();
    let (mut app, area) = shell_touch_app(&root, &state, false).await;
    let (x, y) = (area.x + 2, area.y + 3);
    assert_eq!(crate::shell_tests::scrollback(&app), 0);
    // The finger moving down brings earlier output in.
    app.mouse(DOWN, x, y);
    app.mouse(DRAG, x, y + 4);
    app.mouse(UP, x, y + 4);
    assert_eq!(crate::shell_tests::scrollback(&app), 4);
    app.mouse(DOWN, x, y);
    app.mouse(DRAG, x, y - 2);
    app.mouse(UP, x, y - 2);
    assert_eq!(crate::shell_tests::scrollback(&app), 2);
    assert_eq!(app.group().active_shell().unwrap().run.selection(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_hold_then_drag_selects_in_a_shell_and_a_tap_clears_it() {
    let (_d, root, state) = crate::shell_tests::dirs();
    let (mut app, area) = shell_touch_app(&root, &state, false).await;
    let (x, y) = (area.x, area.y + 3);
    app.mouse(DOWN, x, y);
    assert!(app.press_pending());
    app.test_hold();
    app.mouse(DRAG, x + 2, y + 1);
    app.mouse(UP, x + 2, y + 1);
    assert!(app.group().active_shell().unwrap().run.selection().is_some());
    tap(&mut app, x + 5, y + 4);
    assert_eq!(app.group().active_shell().unwrap().run.selection(), None);
    assert_eq!(crate::shell_tests::scrollback(&app), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_a_shell_that_reports_the_mouse_gets_wheel_events_and_taps() {
    let (_d, root, state) = crate::shell_tests::dirs();
    let (mut app, area) = shell_touch_app(&root, &state, true).await;
    let (x, y) = (area.x + 2, area.y + 3);
    app.mouse(DOWN, x, y);
    app.mouse(DRAG, x, y + 2);
    app.mouse(UP, x, y + 2);
    // The finger moving down is the wheel up (button 64), once per row; nothing was clicked.
    assert!(
        app.settle(|a| crate::shell_tests::screen_text(a).matches("^[[<64;").count() == 2)
            .await,
        "wheel events:\n{}",
        crate::shell_tests::screen_text(&app)
    );
    assert!(!crate::shell_tests::screen_text(&app).contains("^[[<0;"));
    tap(&mut app, x, y);
    assert!(
        app.settle(|a| {
            let t = crate::shell_tests::screen_text(a);
            t.contains("^[[<0;") && t.contains("^[[<0;3;4m")
        })
        .await,
        "the tap's press and release:\n{}",
        crate::shell_tests::screen_text(&app)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_swipe_scrolls_a_hex_view_and_a_tap_moves_its_caret() {
    let (_d, _root, mut app) = crate::hex_tests::hex_app().await;
    crate::hex_tests::quick_open(&mut app, "bin.dat").await;
    app.touch_flag = true;
    crate::hex_tests::draw(&mut app, 100, 12);
    let rect = crate::hex_tests::rows_rect(&app);
    let (x, y) = (rect.x + 5, rect.y + 6);
    assert_eq!(app.active_hex().unwrap().top_row(), 0);
    app.mouse(DOWN, x, y);
    app.mouse(DRAG, x, y - 3);
    app.mouse(UP, x, y - 3);
    assert_eq!(app.active_hex().unwrap().top_row(), 3);
    assert_eq!(app.active_hex().unwrap().selection(), None);
    assert_eq!(app.active_hex().unwrap().cursor, 0, "no caret move");
    crate::hex_tests::draw(&mut app, 100, 12);
    let col = app.active_hex().unwrap().layout.hex_col(5);
    tap(&mut app, rect.x + col as u16, y);
    assert_eq!(app.active_hex().unwrap().cursor, 9 * 16 + 5, "the tap moved the caret");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_b_swipe_scrolls_a_diff_and_a_tap_moves_its_caret() {
    use crate::milestone10_tests::{dirs, draw_sized, open_file_app, repo_with, run_command, settle_diffs};
    let (_d, root, state) = dirs();
    let original: String = (0..80).map(|i| format!("line {i}\n")).collect();
    let changed = original.replacen("line 1\n", "line ONE\n", 1);
    repo_with(&root, &[("f.txt", &original)], &[("f.txt", &changed)]);
    let mut app = open_file_app(&root, &state, "f.txt");
    assert!(crate::milestone10_tests::settle_markers(&mut app).await);
    run_command(&mut app, "Git: Open Changes");
    assert!(settle_diffs(&mut app).await);
    app.touch_flag = true;
    draw_sized(&mut app, 150, 24);
    let h = app.hits.group(app.groups.active).unwrap().diff.clone().unwrap();
    let (x, y) = (h.text_x[0] + 2, h.rows.y + 8);
    let top = app.group().active_diff().unwrap().top;
    let caret = app.group().active_diff().unwrap().caret.head;
    app.mouse(DOWN, x, y);
    app.mouse(DRAG, x, y - 4);
    app.mouse(UP, x, y - 4);
    assert_eq!(app.group().active_diff().unwrap().top, top + 4);
    assert_eq!(app.group().active_diff().unwrap().caret.head, caret, "no caret move");
    draw_sized(&mut app, 150, 24);
    tap(&mut app, x, y);
    assert_ne!(
        app.group().active_diff().unwrap().caret.head,
        caret,
        "the tap moved the caret"
    );
}
