//! jed_touch-a, "Done when": with `--touch` (`App::touch_flag`) or `touch_layout`, every group's tab
//! bar (editor and terminal area, top and bottom) is 3 rows with padded tabs, the label, `×` and
//! group number on the middle row and the drop marker across all rows; tapping any row of a tab
//! focuses it on release, the close range (3 cells) closes it, the arrows and a swipe scroll an
//! overflowing bar, the `+` opens a tab in its group, a held tab (the hold event, and once the real
//! timer) is drawn lifted and drags to another group and onto a split edge; the middle click, the
//! right click and the wheel work on the pad rows; shells and editors are sized to the area left
//! over; the setting turns the layout on and off with a settings reload; with neither set nothing
//! changes.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{MouseButton, MouseEventKind};
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
    assert!(app.pending_press_serial().is_none() && !app.dragging_tab());
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

    // A hold stale by serial does nothing; the right one lifts the tab (drawn `tab.lifted`).
    app.mouse(DOWN, tabs[0].x0 + 3, bar.y + 2);
    let serial = app.pending_press_serial().expect("a press is pending");
    app.test_hold(serial + 1);
    assert!(!app.dragging_tab());
    app.test_hold(serial);
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
    let serial = app.pending_press_serial().unwrap();
    app.test_hold(serial);
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
    assert!(app.pending_press_serial().is_none() && !app.dragging_tab());
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
