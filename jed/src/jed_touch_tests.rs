//! jed_touch-a, "Done when": with `--touch` (`App::touch_flag`) or `touch_layout`, every group's tab
//! bar (editor and terminal area, top and bottom) is 3 rows with padded tabs, the label, `×` and
//! group number on the middle row and the drop marker across all rows; tapping any row of a tab
//! focuses it on release, the close range (3 cells) closes it, the arrows and a swipe scroll an
//! overflowing bar, the `+` opens a tab in its group, a held tab (the hold event, and once the real
//! timer) is drawn lifted and drags to another group and onto a split edge (and a swipe moves a
//! long tab column by column: `..._column_by_column`); the middle click, the
//! right click and the wheel work on the pad rows; shells and editors are sized to the area left
//! over; the setting turns the layout on and off with a settings reload; with neither set nothing
//! changes.
//!
//! jed_touch-b, "Done when" (the `jed_touch_b_*` tests): a swipe in an editor, a diff, a hex view and a
//! shell scrolls it without selecting (a shell that reports the mouse gets wheel events), a tap moves
//! the cursor or caret, a hold then drag selects, double- and triple-tap select a word and a line, a
//! hold released in place opens the context menu on the editor, an Explorer row and a tab; with the
//! layout off, presses behave as today.
//!
//! jed_touch-c, "Done when" (the `jed_touch_c_*` tests): the side panel floats over the editor (nothing resized) and a tap
//! beside it hides it; opening a file, diff, result, commit or log from it hides it, expanding or staging does not;
//! a problem or location hides the bottom panel; side-by-side groups merge (start, reload), Split Right and an edge
//! drop split down; lines wrap unless `word_wrap` is set; with the layout off nothing changes.

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
    // The touch layout's side panel floats over the editor's left edge, where the first tabs are.
    app.ui.side_visible = !touch;
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

    // A swipe left scrolls towards the end and focuses nothing.
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
async fn jed_touch_a_a_swipe_moves_a_long_tab_column_by_column() {
    // A first tab wider than the bar: the swipe moves through it, not a whole tab at a time.
    let dir = tempfile::tempdir().unwrap();
    let long = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH.txt";
    let (mut app, _) = open_app(&dir, &[long, "b.txt"], true);
    let id = app.groups.active;
    app.groups.get_mut(id).unwrap().active = 0;
    let term = draw_sized(&mut app, 60, 20);
    let g = app.hits.group(id).unwrap().clone();
    let arrows = g.tab_arrows.clone().expect("the long tab overflows 60 cells");
    let row = g.tab_bar.y + 1;
    assert!(row_text(&term, row, 0, 60).contains("abcdefgh"));
    assert_eq!((g.tabs[0].x0, app.groups.get(id).unwrap().scroll), (arrows.back.end, 0));
    let swipe = |app: &mut App, x: u16, to: u16| {
        app.mouse(DOWN, x, row);
        app.mouse(DRAG, (x + to) / 2, row);
        app.mouse(DRAG, to, row);
        app.mouse(UP, to, row);
    };

    swipe(&mut app, 20, 14);
    let g = app.groups.get(id).unwrap();
    assert_eq!((g.scroll, g.shift), (0, 6), "six columns into the first tab");
    let term = draw_sized(&mut app, 60, 20);
    let text = row_text(&term, row, 0, 60);
    assert!(text.contains("efghij") && !text.contains("abcd"), "{text}");
    assert_eq!(
        bar_of(&app, id).1[0].x0,
        arrows.back.end,
        "the cut tab starts at the arrow"
    );
    assert_eq!(active_name(&app), long, "a swipe is no tap");

    // Back the same way, then forward and the back arrow, which shows the cut tab whole.
    swipe(&mut app, 14, 20);
    assert_eq!(app.groups.get(id).unwrap().shift, 0);
    swipe(&mut app, 20, 16);
    assert_eq!(app.groups.get(id).unwrap().shift, 4);
    draw_sized(&mut app, 60, 20);
    tap(&mut app, arrows.back.start + 1, row);
    let g = app.groups.get(id).unwrap();
    assert_eq!((g.scroll, g.shift), (0, 0), "the back arrow shows the cut tab");

    // A long swipe stops with the last tab's right edge at the bar's.
    draw_sized(&mut app, 60, 20);
    swipe(&mut app, 50, 0);
    draw_sized(&mut app, 60, 20);
    let g = app.groups.get(id).unwrap();
    assert!(g.scroll == 0 && g.shift > 6, "still in the long tab: {}", g.shift);
    let tabs = bar_of(&app, id).1;
    assert_eq!(tabs.last().unwrap().x1, arrows.forward.start);
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
    app.ui.side_visible = false;
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
        app.ui.side_visible = !touch;
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
    // Lines wrap in the touch layout; this scrolls the unwrapped ones.
    app.test_execute(Command::ToggleWordWrap);
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
    app.ui.side_visible = false;
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
    app.ui.side_visible = false;
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
    app.ui.side_visible = false;
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

// ---- jed_touch-c: room for the editor ----

fn text_rect(app: &App) -> Rect {
    app.hits.group(app.groups.active).unwrap().text
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_the_side_panel_floats_over_the_editor_and_a_tap_beside_it_hides_it() {
    for touch in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _) = open_app(&dir, &["a.txt"], touch);
        app.ui.side_visible = false;
        draw(&mut app);
        let (hidden, region) = (text_rect(&app), app.hits.regions.editor);
        app.test_execute(Command::ToggleSidebar);
        assert!(app.ui.side_visible);
        let term = draw(&mut app);
        let side = app.hits.regions.side.expect("the side panel shows");
        if touch {
            assert_eq!(text_rect(&app), hidden, "the editor keeps its size");
            assert_eq!(app.hits.regions.editor, region);
            assert!(side.intersects(region), "drawn over the editor's left edge");
            assert!(region.width - side.width >= 20, "20 columns of editor stay visible");
            // Its border is on its right edge, over the editor.
            let edge = side.x + side.width - 1;
            assert_eq!(term.backend().buffer()[(edge, side.y + 2)].symbol(), "│");
            // A tap in the editor outside it hides it and is not passed on.
            let before = app.group().active_tab().unwrap().view.selections.clone();
            let (x, y) = (side.x + side.width + 3, text_rect(&app).y + 1);
            tap(&mut app, x, y);
            assert!(!app.ui.side_visible);
            assert_eq!(app.group().active_tab().unwrap().view.selections, before);
            // A tap inside the panel doesn't hide it.
            app.test_execute(Command::ToggleSidebar);
            draw(&mut app);
            tap(&mut app, side.x + 2, side.y + side.height - 3);
            assert!(app.ui.side_visible);
        } else {
            assert!(text_rect(&app).width < hidden.width, "beside the editor, as ever");
            assert!(!side.intersects(app.hits.regions.editor));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_only_a_tap_beside_the_side_panel_dismisses_it() {
    // The bottom panel is beside it too; the activity bar, the status bar and the panel itself are not.
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt"], true);
    app.test_execute(Command::ToggleProblems);
    app.ui.side_visible = true;
    draw(&mut app);
    let (side, status) = (app.hits.regions.side.unwrap(), app.hits.regions.status);
    tap(&mut app, side.x + 2, side.y + side.height - 3);
    assert!(app.ui.side_visible, "a tap on the panel");
    let activity = app.hits.regions.activity;
    let icon_row = app.hits.icons[1].0;
    tap(&mut app, activity.x, icon_row);
    assert!(app.ui.side_visible, "a tap on the activity bar");
    draw(&mut app);
    tap(&mut app, status.x + 1, status.y);
    assert!(app.ui.side_visible, "a tap on the status bar");
    let panel = app.hits.regions.panel.expect("the bottom panel shows");
    tap(&mut app, side.x + side.width + 2, panel.y + 1);
    assert!(!app.ui.side_visible, "a tap on the bottom panel");
    // The layout off: a click in the editor never hides it.
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt"], false);
    assert!(app.ui.side_visible);
    draw(&mut app);
    let e = app.hits.regions.editor;
    tap(&mut app, e.x + 5, e.y + 5);
    assert!(app.ui.side_visible);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_showing_the_side_panel_leaves_a_shell_unresized() {
    use crate::shell_tests::{dirs, shell_app};
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.touch_flag = true;
    app.ui.side_visible = false;
    draw_sized(&mut app, 100, 30);
    app.test_execute(Command::NewShell);
    let size = |a: &App| {
        let shell = a.terminals.active_group().active_shell().unwrap();
        crate::tasks::select::size(shell.run.screen().screen())
    };
    draw_sized(&mut app, 100, 30);
    let before = size(&app);
    app.test_execute(Command::ToggleSidebar);
    draw_sized(&mut app, 100, 30);
    draw_sized(&mut app, 100, 30);
    assert!(app.ui.side_visible);
    assert_eq!(size(&app), before);
}

/// A touch app on a git repo with Source Control changes, the side panel shown.
fn scm_touch_app() -> (tempfile::TempDir, App, PathBuf) {
    let (d, root, state) = crate::milestone10_tests::dirs();
    crate::milestone10_tests::scm_repo(&root);
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.touch_flag = true;
    app.ui.side_visible = false;
    app.test_start();
    app.ui.side_visible = true;
    (d, app, root)
}

fn explorer_row_of(app: &App, name: &str) -> u16 {
    let i = app
        .explorer
        .rows()
        .iter()
        .position(|r| r.entry.path.file_name().is_some_and(|n| n == name))
        .unwrap_or_else(|| panic!("{name} in the Explorer"));
    app.hits.explorer_rows.y + (i - app.explorer.scroll) as u16
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_the_side_panel_hides_after_opening_a_file_and_keeps_its_view() {
    let (_d, mut app, root) = scm_touch_app();
    assert!(app.settle(|a| a.explorer.rows().len() >= 4).await);
    app.ui.side_width = 31;
    draw(&mut app);
    // Expanding a folder opens nothing: the panel stays.
    let (x, y) = (app.hits.explorer_rows.x + 3, explorer_row_of(&app, "sub"));
    tap(&mut app, x, y);
    assert!(
        app.settle(|a| a.explorer.rows().iter().any(|r| r.entry.path.ends_with("sub/e.txt")))
            .await
    );
    assert!(app.ui.side_visible);
    draw(&mut app);
    // A tap on a file opens it, hides the panel and focuses the editor.
    let y = explorer_row_of(&app, "a.txt");
    tap(&mut app, x, y);
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.path == root.join("a.txt")))
            .await
    );
    assert!(!app.ui.side_visible);
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    // Brought back as it was: the same width, the Explorer, the folder still open.
    app.test_execute(Command::ToggleSidebar);
    assert!(app.ui.side_visible);
    assert_eq!(
        (app.ui.side_width, app.ui.side_panel),
        (31, crate::layout::SidePanel::Explorer)
    );
    assert!(app.explorer.rows().iter().any(|r| r.entry.path.ends_with("sub/e.txt")));
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_the_side_panel_hides_after_a_diff_not_after_staging() {
    use crate::command::ScmCommand as S;
    let (_d, mut app, _root) = scm_touch_app();
    crate::milestone10_tests::focus_scm(&mut app);
    assert!(crate::milestone10_tests::settle_git(&mut app, |s| !s.entries.is_empty()).await);
    // A section header collapses and expands, opening nothing.
    app.test_execute(Command::Scm(S::Down));
    app.test_execute(Command::Scm(S::Open));
    assert!(app.ui.side_visible);
    app.test_execute(Command::Scm(S::Open));
    assert!(app.ui.side_visible);
    // Down to a file, then unstage it: still shown.
    app.test_execute(Command::Scm(S::Down));
    app.test_execute(Command::UnstageFile);
    assert!(crate::milestone10_tests::settle_stage(&mut app).await);
    assert!(app.ui.side_visible, "staging opens nothing");
    // Opening a change's diff hides it.
    app.test_execute(Command::Scm(S::Preview));

    assert!(app.settle(|a| a.group().active_diff().is_some()).await);
    assert!(!app.ui.side_visible);
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    // Opening from outside the panel (the keyboard) leaves a shown panel alone.
    app.test_execute(Command::ToggleSidebar);
    app.ui.focus = crate::app::Focus::Editor;
    app.test_execute(Command::NextTab);
    assert!(app.ui.side_visible);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_the_side_panel_hides_after_a_search_result_a_commit_and_a_log() {
    use crate::command::{GitHistoryCommand as H, SearchCommand as Q};
    let (_d, mut app, _root) = scm_touch_app();
    // A search result.
    app.test_execute(Command::FindInFiles);
    app.type_text("a2");
    app.search_tick(std::time::Instant::now() + crate::search_view::DEBOUNCE);
    assert!(app.settle(|a| a.search.as_ref().is_some_and(|r| r.done)).await);
    app.test_execute(Command::Search(Q::FocusResults));
    app.test_execute(Command::Search(Q::Down));
    app.test_execute(Command::Search(Q::Open));
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.name() == "a.txt"))
            .await
    );
    assert!(!app.ui.side_visible, "a search result");
    // A Git History commit.
    app.test_execute(Command::FocusExplorer);
    assert!(app.ui.side_visible);
    assert!(app.settle(|a| !a.git_history.commits.is_empty()).await);
    app.ui.focus = crate::app::Focus::SidePanel;
    app.git_history.set_focused(true);
    app.test_execute(Command::GitHistory(H::Open));
    assert!(app.settle(|a| a.active_commit().is_some()).await);
    assert!(!app.ui.side_visible, "a commit");
    // A tool's log.
    app.test_execute(Command::ShowTools);
    assert!(app.ui.side_visible);
    app.test_execute(Command::Tools(crate::command::ToolsCommand::Open));
    assert!(!app.ui.side_visible, "a tool's log");
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_with_the_layout_off_the_side_panel_stays_after_opening() {
    let (_d, mut app, _root) = scm_touch_app();
    app.touch_flag = false;
    assert!(app.settle(|a| a.explorer.rows().len() >= 4).await);
    draw(&mut app);
    let y = explorer_row_of(&app, "a.txt");
    let x = app.hits.explorer_rows.x + 3;
    app.mouse(DOWN, x, y);
    app.mouse(UP, x, y);
    app.mouse(DOWN, x, y);
    app.mouse(UP, x, y);
    assert!(app.settle(|a| a.active_buffer().is_some()).await);
    assert!(app.ui.side_visible);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_the_bottom_panel_hides_after_a_problem_and_a_location() {
    use crate::layout::PanelTab;
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _fake, _root) = crate::milestone9_tests::problems_project(dir.path()).await;
    app.touch_flag = true;
    app.test_execute(Command::ToggleProblems);
    for _ in 0..4 {
        app.press(KeyCode::Down, KeyModifiers::NONE);
    }
    let height = app.ui.panel_height;
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(!app.ui.panel_visible, "the problem was opened");
    assert_eq!(
        (app.ui.focus, app.ui.panel_tab, app.ui.panel_height),
        (crate::app::Focus::Editor, PanelTab::Problems, height)
    );
    // The toggle brings it back as it was.
    app.test_execute(Command::ToggleProblems);
    assert!(app.ui.panel_visible && app.ui.panel_tab == PanelTab::Problems);

    // A location.
    let dir = tempfile::tempdir().unwrap();
    let (a_text, b_text) = ("int target = 1;\nint use = target;\n", "int z;\n");
    let (mut app, fake, root) =
        crate::milestone9_tests::lsp_project(dir.path(), &[("a.cpp", a_text), ("b.cpp", b_text)], true).await;
    app.touch_flag = true;
    let a = root.join("a.cpp");
    crate::milestone9_tests::put_cursor(&mut app, a_text.find("= target").unwrap() + 4);
    let loc = crate::milestone9_tests::location;
    fake.respond(
        "textDocument/references",
        serde_json::json!([loc(&a, 0, 4, 10), loc(&a, 1, 10, 16)]),
    );
    app.test_execute(Command::GoToReferences);
    assert!(app.settle(|a| a.locations.is_some()).await);
    assert!(app.ui.panel_visible);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(!app.ui.panel_visible, "the location was opened");
    assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    // With the layout off the panel stays.
    app.touch_flag = false;
    app.test_execute(Command::GoToReferences);
    assert!(app.settle(|a| a.ui.panel_visible).await);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.ui.panel_visible);
}

/// Touch off, one group: a second column (`b.txt` again, the cursor moved, then `c.txt`) and a
/// third (`c.txt` again, then `d.txt`).
fn columns(app: &mut App, root: &std::path::Path) {
    app.test_execute(Command::SplitRight);
    app.press(KeyCode::Down, KeyModifiers::NONE);
    app.test_open(Buffer::open(&root.join("c.txt")).unwrap());
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("d.txt")).unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_side_by_side_groups_merge_into_the_first_and_down_splits_stay() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, root) = open_app(&dir, &["a.txt", "b.txt"], false);
    for n in ["c.txt", "d.txt"] {
        fs::write(root.join(n), format!("{n} 1\n{n} 2\n")).unwrap();
    }
    columns(&mut app, &root);
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 3);
    assert_eq!(names(&app, ids[1]), ["b.txt", "c.txt"]);
    // A stacked group under the first column.
    app.test_execute(Command::FocusGroup(1));
    app.test_execute(Command::SplitDown);
    assert_eq!(app.groups.ids().len(), 4);
    // Focus the second column (`c.txt` active there) and merge.
    let second = ids[1];
    app.test_execute(Command::FocusGroup(3));
    assert_eq!(app.groups.active, second);
    app.touch_flag = true;
    app.sync_touch_layout();
    let ids = app.groups.ids();
    assert_eq!(ids.len(), 2, "the stacked pair stays: {ids:?}");
    assert_eq!(names(&app, ids[0]), ["a.txt", "b.txt", "c.txt", "d.txt"]);
    assert_eq!(names(&app, ids[1]), ["b.txt"], "the Down split's copy");
    assert_eq!(app.groups.active, ids[0]);
    assert_eq!(active_name(&app), "c.txt");
    // `b.txt` was in the first column and the focused second: it keeps the focused group's view.
    let b = &app.groups.get(ids[0]).unwrap().tabs[1];
    let cursor = b.text().unwrap().view.selections.primary().head.0;
    assert!(cursor > 0, "the focused group's view won: {cursor}");
    // No side-by-side split is made now.
    app.test_execute(Command::SplitRight);
    draw(&mut app);
    let all = app.groups.ids();
    assert_eq!(all.len(), 3);
    let x0 = app.hits.group(all[0]).unwrap().tab_bar.x;
    assert!(
        all.iter().all(|&id| app.hits.group(id).unwrap().tab_bar.x == x0),
        "{all:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_split_right_and_an_edge_drop_split_down() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt", "b.txt"], true);
    app.ui.side_visible = false;
    draw(&mut app);
    let first = app.groups.active;
    app.test_execute(Command::SplitRight);
    draw(&mut app);
    assert_eq!(app.groups.ids().len(), 2);
    let x0 = app.hits.group(first).unwrap().tab_bar.x;
    let stacked = |app: &App| {
        app.groups
            .ids()
            .iter()
            .all(|&id| app.hits.group(id).unwrap().tab_bar.x == x0)
    };
    assert!(stacked(&app), "Split Editor Right split down");
    // A tab held and dropped on a group's right edge: the marker shows the bottom half.
    let (bar, tabs) = bar_of(&app, first);
    let editor = app.hits.group(first).unwrap().editor;
    app.mouse(DOWN, tabs[0].x0 + 3, bar.y + 1);
    assert!(app.press_pending());
    app.test_hold();
    let (x, y) = edge_point(editor, DropSide::Right);
    app.mouse(DRAG, x, y);
    let shade = app.tab_drop_shade(first, editor).unwrap();
    assert_eq!((shade.x, shade.width), (editor.x, editor.width));
    assert!(shade.y > editor.y, "the bottom half: {shade:?}");
    app.mouse(UP, x, y);
    assert_eq!(app.groups.ids().len(), 3);
    draw(&mut app);
    assert!(stacked(&app));
    // Cmd+N one past the last group.
    app.test_execute(Command::FocusGroup(4));
    draw(&mut app);
    assert_eq!(app.groups.ids().len(), 4);
    assert!(stacked(&app));
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_a_reload_turning_the_layout_on_merges_and_off_leaves_it() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, user_file) = open_with_settings(&dir, "touch_layout = false\n");
    let root = user_file.parent().unwrap().parent().unwrap().join("ws");
    for n in ["b.txt", "c.txt"] {
        fs::write(root.join(n), format!("{n}\n")).unwrap();
    }
    app.test_open(Buffer::open(&root.join("b.txt")).unwrap());
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("c.txt")).unwrap());
    assert_eq!(app.groups.ids().len(), 2);
    fs::write(&user_file, "[settings]\ntouch_layout = true\n").unwrap();
    assert!(app.settle(|a| a.groups.ids().len() == 1).await, "merged by the reload");
    let id = app.groups.active;
    assert_eq!(names(&app, id), ["a.txt", "b.txt", "c.txt"]);
    // Off again: the one group stays, and splits go right.
    fs::write(&user_file, "[settings]\ntouch_layout = false\n").unwrap();
    assert!(app.settle(|a| !a.groups.stacked).await);
    assert_eq!(app.groups.ids().len(), 1);
    app.test_execute(Command::SplitRight);
    draw(&mut app);
    let ids = app.groups.ids();
    assert!(app.hits.group(ids[1]).unwrap().tab_bar.x > app.hits.group(ids[0]).unwrap().tab_bar.x);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_lines_wrap_in_the_touch_layout_unless_word_wrap_is_set() {
    use crate::edit::WordWrap;
    let dir = tempfile::tempdir().unwrap();
    let (app, _) = open_app(&dir, &["a.txt"], false);
    assert_eq!(app.word_wrap(), WordWrap::Off);
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _) = open_app(&dir, &["a.txt"], true);
    assert_eq!(app.word_wrap(), WordWrap::On);
    // The session toggle still flips it.
    app.test_execute(Command::ToggleWordWrap);
    assert_eq!(app.word_wrap(), WordWrap::Off);
    // A setting wins: off in the user's file with the layout on.
    let dir = tempfile::tempdir().unwrap();
    let (app, _) = open_with_settings(&dir, "touch_layout = true\nword_wrap = \"off\"\n");
    assert!(app.touch());
    assert_eq!(app.word_wrap(), WordWrap::Off);
    let dir = tempfile::tempdir().unwrap();
    let (app, _) = open_with_settings(&dir, "touch_layout = true\n");
    assert_eq!(app.word_wrap(), WordWrap::On);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_a_tap_in_the_terminal_area_beside_the_side_panel_hides_it_and_is_not_passed_on() {
    use crate::shell_tests::{dirs, shell_app};
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.touch_flag = true;
    app.test_execute(Command::NewShell);
    app.ui.side_visible = true;
    draw_sized(&mut app, 100, 30);
    let (side, term) = (
        app.hits.regions.side.unwrap(),
        app.hits.regions.terminal.expect("the terminal area"),
    );
    assert!(app.ui.side_visible);
    let before = app
        .terminals
        .active_group()
        .active_shell()
        .map(|s| s.run.screen().screen().cursor_position());
    tap(&mut app, side.x + side.width + 3, term.y + 2);
    assert!(!app.ui.side_visible);
    assert_eq!(
        app.terminals
            .active_group()
            .active_shell()
            .map(|s| s.run.screen().screen().cursor_position()),
        before
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_a_workspace_reload_turning_the_layout_on_merges() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, user_file) = open_with_settings(&dir, "");
    let root = user_file.parent().unwrap().parent().unwrap().join("ws");
    fs::write(root.join("b.txt"), "b\n").unwrap();
    app.test_execute(Command::SplitRight);
    app.test_open(Buffer::open(&root.join("b.txt")).unwrap());
    assert_eq!(app.groups.ids().len(), 2);
    fs::write(root.join(".workspace"), "[settings]\ntouch_layout = true\n").unwrap();
    assert!(app.settle(|a| a.groups.ids().len() == 1).await, "merged by the reload");
    assert!(app.groups.stacked);
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_the_side_panel_hides_after_opening_from_file_history() {
    use crate::command::FileHistoryCommand as F;
    for open_version in [false, true] {
        let (_d, mut app) = crate::history_view_tests::app();
        crate::history_view_tests::two_saves(&mut app).await;
        app.touch_flag = true;
        app.test_execute(Command::ShowFileHistory);
        assert!(app.ui.side_visible && app.file_history_view.focused());
        app.test_execute(Command::FileHistory(if open_version {
            F::OpenVersion
        } else {
            F::Open
        }));
        assert!(!app.ui.side_visible, "open_version {open_version}");
        assert_eq!(app.ui.focus, crate::app::Focus::Editor);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jed_touch_c_the_side_panel_hides_after_a_code_review_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _server, _main) = crate::mr_review_tests::shown(dir.path()).await;
    app.touch_flag = true;
    assert!(app.ui.side_visible);
    let path = app
        .review
        .files()
        .first()
        .map(|f| f.path.clone())
        .expect("a review file");
    crate::mr_review_tests::open_file(&mut app, &path).await;
    assert!(!app.ui.side_visible, "a Code Review file");
}
