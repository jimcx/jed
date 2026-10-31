//! End-to-end tests for term_area-a (the terminal area's region and its first terminal): the
//! area's layout (full width above the status bar, height clamped), "Terminal: New Shell"
//! (Ctrl+`) opening a shell in it with the editor groups keeping the rest, its keys and
//! `Context::Shell`, a second shell as a tab, closing the last one hiding the area (and the
//! Jed splash for an empty editor area), dragging its top border (and the bottom panel's above
//! it), clicks, the PTY's size at open, Cmd+W / tab cycling and the terminal cursor following
//! the focus, its scroll bar, and editor-group shell tabs still working beside it. term_area-b:
//! splitting the area (empty groups, never a copied shell), the group keys following the focused
//! area, reordering and moving terminals by key and drag, edge-drop splits with their shade,
//! refused drags into the area, Esc cancelling, divider drags, hiding. term_area-c: moving a
//! terminal into an editor group (drag onto its bar, screen or edge; "Move Terminal into Editor
//! Area") and back ("... into Terminal Area"), the startup terminal, and the done-when walk.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::config::keymap::Context;
use crate::group::{TERMINAL_GROUP_BASE, is_terminal_group};
use crate::layout::{self, Hit, MIN_EDITOR_HEIGHT, MIN_TERMINAL_HEIGHT};
use crate::shell_tests::{dirs, draw, shell_app};

fn app_in(root: &std::path::Path, state: &std::path::Path) -> App {
    let mut app = shell_app(root, state);
    app.tasks.shell = "/bin/sh".into();
    app
}

fn shell_text(app: &App) -> String {
    app.terminals
        .active_group()
        .active_shell()
        .unwrap()
        .run
        .screen()
        .screen()
        .contents()
}

fn click(app: &mut App, x: u16, y: u16) {
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// The area is full width above the status bar; the editor, side panel and bottom panel are
/// above it; its height is clamped to leave the editor its minimum.
#[test]
fn term_area_layout_region_and_clamping() {
    let screen = Rect::new(0, 0, 100, 30);
    let r = layout::compute(screen, true, 25, Some(10), Some(12));
    assert_eq!(
        r.terminal,
        Some(Rect::new(0, 17, 100, 12)),
        "full width, above the status bar"
    );
    assert_eq!(r.status, Rect::new(0, 29, 100, 1));
    assert_eq!(r.side.unwrap().height, 17, "the side panel stops above it");
    assert_eq!(r.activity.height, 17);
    assert_eq!(
        r.editor.height + r.panel.unwrap().height,
        17,
        "editor and panel share the rest"
    );
    let r = layout::compute(screen, true, 25, None, None);
    assert_eq!(r.terminal, None);
    assert_eq!(r.editor.height, 29);
    let r = layout::compute(screen, true, 25, None, Some(100));
    assert_eq!(r.editor.height, MIN_EDITOR_HEIGHT, "the editor keeps its minimum");
    assert_eq!(r.terminal.unwrap().height, 29 - MIN_EDITOR_HEIGHT);
    let r = layout::compute(screen, true, 25, None, Some(1));
    assert_eq!(r.terminal.unwrap().height, MIN_TERMINAL_HEIGHT, "and the area its own");
    let r = layout::compute(Rect::new(0, 0, 100, 5), true, 25, None, Some(10));
    assert_eq!(r.terminal, None, "no room: the editor wins");
}

/// Ctrl+` opens a shell in the area (shown, focused), not in the editor group.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_ctrl_backtick_opens_a_shell_in_the_area() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    app.test_open(Buffer::new_file(&root.join("a.txt")));
    draw(&mut app, 100, 30);
    let editor_before = app.hits.regions.editor;
    assert!(!app.ui.terminal_visible);

    app.press(KeyCode::Char('`'), KeyModifiers::CONTROL);
    assert!(app.ui.terminal_visible);
    assert_eq!(app.ui.focus, Focus::Terminal);
    assert_eq!(app.context(), Context::Shell);
    assert_eq!(app.terminals.active_group().tabs.len(), 1);
    assert_eq!(app.group().tabs.len(), 1, "the editor group keeps just its text tab");
    assert!(app.group().active_shell().is_none());

    draw(&mut app, 100, 30);
    let t = app.hits.regions.terminal.expect("the area is drawn");
    assert_eq!((t.x, t.width, t.y + t.height), (0, 100, 29));
    assert_eq!(app.hits.regions.editor.height, editor_before.height - t.height);
    assert_eq!(
        app.hits.regions.editor.width, editor_before.width,
        "the editor keeps its width"
    );
    let g = app
        .hits
        .group(app.terminals.active)
        .expect("the area's group is hit-testable");
    assert!(is_terminal_group(g.id) && g.id >= TERMINAL_GROUP_BASE);
    assert_eq!(g.tabs.len(), 1, "a tab bar of its terminals");
    let size = crate::tasks::select::size(
        app.terminals
            .active_group()
            .active_shell()
            .unwrap()
            .run
            .screen()
            .screen(),
    );
    assert_eq!(size, (g.editor.height, g.editor.width), "the shell has the area's size");
}

/// Keys reach the focused area shell; clicking the editor moves the keys back, and clicking
/// the area takes them.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_keys_and_focus_follow_clicks() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    app.test_open(Buffer::new_file(&root.join("a.txt")));
    draw(&mut app, 100, 30);
    app.test_execute(Command::NewShell);
    draw(&mut app, 100, 30);

    app.type_text("echo $((6*7))");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        app.settle(|a| shell_text(a).contains("42")).await,
        "{}",
        shell_text(&app)
    );
    assert_eq!(
        app.active_buffer().unwrap().rope.to_string(),
        "",
        "the buffer got no keys"
    );

    let editor = app.hits.group(app.groups.active).unwrap().editor;
    click(&mut app, editor.x + 5, editor.y + 1);
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(app.context(), Context::Editor);
    app.type_text("hi");
    assert_eq!(app.active_buffer().unwrap().rope.to_string(), "hi");

    let area = app.hits.group(app.terminals.active).unwrap().editor;
    click(&mut app, area.x + 2, area.y + 1);
    assert_eq!(app.ui.focus, Focus::Terminal);
    app.type_text("echo $((7*8))");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        app.settle(|a| shell_text(a).contains("56")).await,
        "{}",
        shell_text(&app)
    );
}

/// A second shell is a second tab of the area; closing tabs one by one ends with the area
/// hidden and, with no editor tab, the splash in the editor area.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_second_shell_is_a_tab_and_closing_the_last_hides_it() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    draw(&mut app, 100, 30);
    app.test_execute(Command::NewShell);
    app.test_execute(Command::NewShell);
    assert_eq!(app.terminals.active_group().tabs.len(), 2);
    assert_eq!(app.terminals.active_group().active, 1);
    assert_eq!(app.terminals.active_group().active_shell().unwrap().number, 2);
    draw(&mut app, 100, 30);
    let hit = app.hits.group(app.terminals.active).unwrap();
    assert_eq!(hit.tabs.len(), 2);

    // Clicking the first tab activates it.
    let (x0, y) = (hit.tabs[0].x0, hit.tab_bar.y);
    click(&mut app, x0 + 1, y);
    assert_eq!(app.terminals.active_group().active, 0);

    app.test_execute(Command::CloseTab);
    assert_eq!(app.terminals.active_group().tabs.len(), 1);
    assert!(app.ui.terminal_visible);
    app.test_execute(Command::CloseTab);
    assert!(app.terminals.active_group().tabs.is_empty());
    assert!(!app.ui.terminal_visible, "the area hides with its last terminal");
    assert_eq!(app.ui.focus, Focus::Editor);
    let term = draw(&mut app, 100, 30);
    assert!(app.hits.regions.terminal.is_none());
    assert_eq!(
        app.hits.regions.editor.height, 29,
        "the editor gets the area's rows back"
    );
    let all: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(all.contains("Jim's Editor"), "the splash shows");
}

/// Dragging the area's top border resizes it, clamped.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_border_drag_resizes() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    draw(&mut app, 100, 30);
    app.test_execute(Command::NewShell);
    draw(&mut app, 100, 30);
    let t = app.hits.regions.terminal.unwrap();
    assert_eq!(app.hits.hit(10, t.y), Hit::TerminalBorder);
    app.mouse(MouseEventKind::Down(MouseButton::Left), 10, t.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), 10, t.y - 4);
    app.mouse(MouseEventKind::Up(MouseButton::Left), 10, t.y - 4);
    assert_eq!(app.ui.terminal_height, t.height + 4);
    draw(&mut app, 100, 30);
    assert_eq!(app.hits.regions.terminal.unwrap().height, t.height + 4);
    let rows = crate::tasks::select::size(
        app.terminals
            .active_group()
            .active_shell()
            .unwrap()
            .run
            .screen()
            .screen(),
    )
    .0;
    assert_eq!(rows, t.height + 4 - 2, "the shell is resized to the area");
    // Dragged to the top: the editor keeps its minimum.
    let y = app.hits.regions.terminal.unwrap().y;
    app.mouse(MouseEventKind::Down(MouseButton::Left), 10, y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), 10, 0);
    app.mouse(MouseEventKind::Up(MouseButton::Left), 10, 0);
    draw(&mut app, 100, 30);
    assert_eq!(app.hits.regions.editor.height, MIN_EDITOR_HEIGHT);
}

/// A shell tab in an editor group still works beside the area.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_editor_group_shell_tabs_still_work() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    app.new_shell_in_editor();
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(app.context(), Context::Shell);
    assert!(!app.ui.terminal_visible);
    app.test_execute(Command::NewShell);
    assert_eq!(app.ui.focus, Focus::Terminal);
    assert_eq!(app.group().tabs.len(), 1);
    assert_eq!(app.terminals.active_group().tabs.len(), 1);
    // Numbers count across both places.
    assert_eq!(app.terminals.active_group().active_shell().unwrap().number, 2);
    draw(&mut app, 100, 30);
    app.type_text("echo $((6*7))");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settle(|a| shell_text(a).contains("42")).await);
    // Closing the editor-group shell leaves the area alone.
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    click(&mut app, editor.x + 1, editor.y);
    assert_eq!(app.ui.focus, Focus::Editor);
    app.test_execute(Command::CloseTab);
    assert!(app.group().tabs.is_empty());
    assert!(app.ui.terminal_visible);
}

/// The PTY opens at the area's size before any frame draws it there (`terminal_size` plans it
/// from the last frame's screen), so the first frame resizes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_shell_opens_at_the_area_size() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    draw(&mut app, 100, 30);
    app.test_execute(Command::NewShell);
    let size = |a: &App| {
        let shell = a.terminals.active_group().active_shell().unwrap();
        crate::tasks::select::size(shell.run.screen().screen())
    };
    let opened = size(&app);
    draw(&mut app, 100, 30);
    let g = app.hits.group(app.terminals.active).unwrap();
    assert_eq!(opened, (g.editor.height, g.editor.width), "opened at the drawn size");
    assert_ne!(opened, crate::tasks::DEFAULT_SIZE);
}

/// Cmd+W and the tab commands act on the area while it has the keys and on the editor group
/// once that has them; the terminal cursor is drawn only in the focused one of an editor-group
/// shell and an area shell.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_tab_commands_and_cursor_follow_the_focus() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    app.new_shell_in_editor();
    app.test_open(Buffer::new_file(&root.join("a.txt")));
    app.test_execute(Command::NewShell);
    app.test_execute(Command::NewShell);
    assert_eq!(app.group().tabs.len(), 2);
    for shell in app.shell_tabs() {
        shell.run.write(b"echo RE''ADY\n");
    }
    let ready = |a: &App| {
        a.shell_tabs()
            .all(|s| s.run.screen().screen().contents().contains("READY"))
    };
    assert!(app.settle(ready).await, "the shells started");

    // Area focused: PrevTab cycles its tabs, not the editor group's.
    app.test_execute(Command::PrevTab);
    assert_eq!(app.terminals.active_group().active, 0, "PrevTab cycled the area's tabs");
    assert_eq!(app.group().active, 1, "not the editor group's");
    // The editor group shows its shell too; only the area has the cursor.
    app.group_mut().active = 0;
    let term = draw(&mut app, 100, 30);
    let area = app.hits.group(app.terminals.active).unwrap().editor;
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    assert!(term.backend().cursor_visible());
    let cursor = term.backend().cursor_position();
    assert!(area.contains(cursor), "the cursor is in the area: {cursor:?} {area:?}");

    // A click in the editor group's shell: the cursor moves there.
    click(&mut app, editor.x + 1, editor.y + 1);
    assert_eq!(app.ui.focus, Focus::Editor);
    let term = draw(&mut app, 100, 30);
    assert!(term.backend().cursor_visible());
    let cursor = term.backend().cursor_position();
    assert!(
        editor.contains(cursor),
        "the cursor is in the editor group: {cursor:?} {editor:?}"
    );

    // Cmd+W there closes the editor group's tab, not the area's.
    app.press(KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(app.group().tabs.len(), 1);
    assert_eq!(app.terminals.active_group().tabs.len(), 2);
    // Back in the area, Cmd+W closes its tab and leaves the editor's text tab.
    click(&mut app, area.x + 1, area.y + 1);
    assert_eq!(app.ui.focus, Focus::Terminal);
    app.press(KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(app.terminals.active_group().tabs.len(), 1);
    assert_eq!(app.group().tabs.len(), 1);
}

/// With the bottom panel shown too, the panel sits above the area and its border drag is
/// measured from the area's top.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_with_the_bottom_panel() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    app.test_open(Buffer::new_file(&root.join("a.txt")));
    app.test_execute(Command::NewShell);
    app.test_execute(Command::TogglePanel);
    assert!(app.ui.panel_visible);
    draw(&mut app, 100, 40);
    let (panel, t) = (app.hits.regions.panel.unwrap(), app.hits.regions.terminal.unwrap());
    assert_eq!(panel.y + panel.height, t.y, "the panel ends where the area starts");
    assert_eq!(app.hits.hit(50, panel.y), Hit::PanelBorder);
    assert_eq!(app.hits.hit(50, t.y), Hit::TerminalBorder);
    app.mouse(MouseEventKind::Down(MouseButton::Left), 50, panel.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), 50, panel.y - 3);
    app.mouse(MouseEventKind::Up(MouseButton::Left), 50, panel.y - 3);
    assert_eq!(app.ui.panel_height, panel.height + 3, "measured from the area's top");
    assert_eq!(app.ui.terminal_height, t.height, "the area keeps its height");
}

/// The area shell's scroll bar scrolls its scrollback (`ScrollTarget::Group` of an area group).
#[tokio::test(flavor = "multi_thread")]
async fn term_area_scroll_bar_scrolls_the_shell() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    draw(&mut app, 100, 30);
    app.test_execute(Command::NewShell);
    let shell = app.terminals.active_group().active_shell().unwrap();
    shell
        .run
        .write(format!("{}\n", crate::shell_tests::COUNT_TO_200).as_bytes());
    let done = |a: &App| shell_text(a).lines().any(|l| l.trim_end() == "199");
    assert!(app.settle(done).await, "{}", shell_text(&app));
    draw(&mut app, 100, 30);
    let bar = app.hits.group(app.terminals.active).unwrap().scrollbar;
    let bar = bar.expect("the area shell has a scroll bar");
    assert!(bar.thumb.is_some(), "scrollback to thumb");
    // A press at the track's top jumps back into the scrollback.
    click(&mut app, bar.track.x, bar.track.y);
    let shell = app.terminals.active_group().active_shell().unwrap();
    let back = shell.run.screen().screen().scrollback();
    assert!(back > 0, "the scroll bar scrolled the area shell back");
}

// ---- term_area-b: reorder and split ----

use crate::drag_tabs_tests::{centre, edge_point, half, shaded_cells};
use crate::layout::{DropSide, GroupId};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

/// An app with a text tab and `n` area shells, drawn at 100x40 (the area tall enough for edge bands).
fn area_app(root: &std::path::Path, state: &std::path::Path, n: usize) -> App {
    let mut app = app_in(root, state);
    app.test_open(Buffer::new_file(&root.join("a.txt")));
    app.ui.terminal_height = 20;
    for _ in 0..n {
        app.test_execute(Command::NewShell);
    }
    draw(&mut app, 100, 40);
    app
}

fn titles(app: &App, g: GroupId) -> Vec<String> {
    app.terminals
        .get(g)
        .unwrap()
        .tabs
        .iter()
        .map(|t| match t {
            crate::group::Tab::Shell(s) => s.title.clone(),
            _ => "?".into(),
        })
        .collect()
}

fn tab_point(app: &App, g: GroupId, i: usize) -> (u16, u16) {
    let hit = app.hits.group(g).unwrap();
    (hit.tabs[i].x0 + 1, hit.tab_bar.y)
}

fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) {
    app.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1);
}

/// Drags the first tab of group `g` to `to`.
fn drag_tab(app: &mut App, g: GroupId, to: (u16, u16)) {
    let from = tab_point(app, g, 0);
    drag(app, from, to);
}

/// Cmd+\ and Cmd+Alt+\ in the area split its tree into empty groups, a shell is never copied,
/// and the group keys act on the area; the editor's groups are untouched.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_split_keys_make_empty_groups_and_focus_moves() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    app.press(KeyCode::Char('\\'), cmd());
    assert_eq!(app.terminals.len(), 2);
    assert_eq!(app.groups.len(), 1, "the editor groups are untouched");
    assert_eq!(app.ui.focus, Focus::Terminal);
    assert!(app.terminals.active_group().tabs.is_empty(), "a shell is never copied");
    assert_eq!(app.shell_tabs().count(), 1);
    // The new shell opens in the active (new) group: two live shells side by side.
    app.test_execute(Command::NewShell);
    let term = draw(&mut app, 100, 40);
    let ids = app.terminals.ids();
    let (a, b) = (
        app.hits.group(ids[0]).unwrap().editor,
        app.hits.group(ids[1]).unwrap().editor,
    );
    assert_eq!((a.y, a.height), (b.y, b.height));
    assert!(a.x + a.width <= b.x, "side by side: {a:?} {b:?}");
    let cursor = term.backend().cursor_position();
    assert!(
        b.contains(cursor) && !a.contains(cursor),
        "only the focused group has the cursor"
    );

    app.press(KeyCode::Char('\\'), cmd() | KeyModifiers::ALT);
    assert_eq!(app.terminals.len(), 3);
    draw(&mut app, 100, 40);
    let ids = app.terminals.ids();
    let (b, c) = (
        app.hits.group(ids[1]).unwrap().editor,
        app.hits.group(ids[2]).unwrap().editor,
    );
    assert!(b.y + b.height <= c.y, "stacked: {b:?} {c:?}");

    app.test_execute(Command::FocusPreviousGroup);
    assert_eq!(app.terminals.active, ids[1]);
    app.test_execute(Command::FocusGroup(1));
    assert_eq!(app.terminals.active, ids[0]);
    assert_eq!(app.groups.active, app.groups.ids()[0]);
    assert_eq!(app.ui.focus, Focus::Terminal);
    // Move the tab into the next group: the area's groups, not the editor's.
    app.test_execute(Command::MoveTabToNextGroup);
    assert_eq!(app.shell_tabs().count(), 2);
    assert_eq!(app.terminals.len(), 2, "the emptied group closed");
    assert_eq!(app.ui.focus, Focus::Terminal);
}

/// Move Editor Left / Right reorders the area's tabs.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_move_tab_keys_reorder() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 2);
    let g = app.terminals.active;
    let before = titles(&app, g);
    app.test_execute(Command::MoveTabLeft);
    assert_eq!(titles(&app, g), vec![before[1].clone(), before[0].clone()]);
    assert_eq!(app.terminals.active_group().active, 0);
}

/// Dragging a terminal tab along the bar reorders it.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_drag_reorders_along_the_bar() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 2);
    let g = app.terminals.active;
    let before = titles(&app, g);
    let hit = app.hits.group(g).unwrap().clone();
    drag_tab(&mut app, g, (hit.tabs[1].x1 + 5, hit.tab_bar.y));
    assert_eq!(titles(&app, g), vec![before[1].clone(), before[0].clone()]);
    assert_eq!(app.ui.focus, Focus::Terminal);
}

/// Dropping on another terminal group's bar or screen moves the terminal there (never copies
/// it); an emptied group closes.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_drag_moves_between_groups() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 2);
    app.press(KeyCode::Char('\\'), cmd());
    app.test_execute(Command::NewShell);
    draw(&mut app, 100, 40);
    let ids = app.terminals.ids();
    assert_eq!(app.shell_tabs().count(), 3);
    // Group 0 holds two shells: drag its first onto group 1's bar.
    let bar = app.hits.group(ids[1]).unwrap().tab_bar;
    drag_tab(&mut app, ids[0], (bar.x + bar.width - 2, bar.y));
    assert_eq!(app.terminals.get(ids[0]).unwrap().tabs.len(), 1);
    assert_eq!(app.terminals.get(ids[1]).unwrap().tabs.len(), 2);
    assert_eq!(app.terminals.active, ids[1]);
    assert_eq!(app.shell_tabs().count(), 3);
    draw(&mut app, 100, 40);
    // Group 0's last tab onto group 1's screen: group 0 closes.
    let screen = app.hits.group(ids[1]).unwrap().editor;
    drag_tab(&mut app, ids[0], centre(screen));
    assert_eq!(app.terminals.len(), 1);
    assert_eq!(app.terminals.active_group().tabs.len(), 3);
    assert_eq!(app.ui.focus, Focus::Terminal);
}

/// Dropping on a terminal group's edge splits it there, the zone shaded while dragging.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_drag_to_an_edge_splits_with_a_shade() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 2);
    let g = app.terminals.active;
    let screen = app.hits.group(g).unwrap().editor;
    let (x, y) = edge_point(screen, DropSide::Right);
    let from = tab_point(&app, g, 0);
    app.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
    let term = draw(&mut app, 100, 40);
    let zone = half(screen, DropSide::Right);
    assert!(
        shaded_cells(&app, &term, zone) >= usize::from(zone.width) * usize::from(zone.height) / 2,
        "the right half is shaded"
    );
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(app.terminals.len(), 2);
    let ids = app.terminals.ids();
    assert_eq!(app.terminals.get(ids[0]).unwrap().tabs.len(), 1);
    assert_eq!(app.terminals.get(ids[1]).unwrap().tabs.len(), 1);
    assert_eq!(app.terminals.active, ids[1], "the new split took it");
    assert_eq!(app.ui.focus, Focus::Terminal);
    assert_eq!(app.groups.len(), 1);
}

/// Closing a group's last tab closes the group; the area hides when its last terminal goes.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_closing_tabs_closes_groups_then_hides() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    app.press(KeyCode::Char('\\'), cmd());
    app.test_execute(Command::NewShell);
    assert_eq!(app.terminals.len(), 2);
    app.press(KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(app.terminals.len(), 1, "the emptied group closed");
    assert!(app.ui.terminal_visible);
    assert_eq!(app.shell_tabs().count(), 1);
    app.press(KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert!(!app.ui.terminal_visible, "no terminal anywhere: hidden");
    assert_eq!(app.ui.focus, Focus::Editor);
}

/// Hiding also happens when the only terminal's group sits beside an empty one.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_hides_with_an_empty_group_left() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    app.press(KeyCode::Char('\\'), cmd());
    app.test_execute(Command::FocusPreviousGroup);
    app.press(KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert!(!app.ui.terminal_visible);
}

/// A divider drag resizes the area's tree, not the editor's.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_divider_drag_resizes_the_area() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    app.press(KeyCode::Char('\\'), cmd());
    draw(&mut app, 100, 40);
    let ids = app.terminals.ids();
    let a = app.hits.group(ids[0]).unwrap().editor;
    let d = app
        .hits
        .dividers
        .iter()
        .find(|d| d.terminal)
        .expect("a terminal divider")
        .rect;
    let before = app.groups.tree.clone();
    drag(&mut app, (d.x, d.y + 1), (d.x - 10, d.y + 1));
    draw(&mut app, 100, 40);
    let after = app.hits.group(ids[0]).unwrap().editor;
    assert!(after.width < a.width, "{a:?} -> {after:?}");
    assert_eq!(app.groups.tree, before, "the editor's tree is untouched");
}

/// A text tab can't be dragged into the area (a shell can: todo_tests::drag_to_terminal_area): no marker or
/// shade, nothing moves.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_refuses_drags_between_the_area_and_editor_groups() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    let eg = app.groups.active;
    let tg = app.terminals.active;
    // Text tab -> the area's screen and edge.
    let screen = app.hits.group(tg).unwrap().editor;
    let from = tab_point(&app, eg, 0);
    app.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    let c = centre(screen);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), c.0, c.1);
    let term: Terminal<TestBackend> = draw(&mut app, 100, 40);
    assert_eq!(shaded_cells(&app, &term, screen), 0, "no shade");
    assert_eq!(app.tab_drop_in(tg), None);
    let edge = edge_point(screen, DropSide::Left);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), edge.0, edge.1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), edge.0, edge.1);
    assert_eq!(app.terminals.len(), 1);
    assert_eq!(app.groups.get(eg).unwrap().tabs.len(), 1);
    assert_eq!(app.terminals.get(tg).unwrap().tabs.len(), 1);
    assert_eq!(app.groups.len(), 1, "no editor split either");
    // Text tab -> the area's bar: no marker.
    let tbar = app.hits.group(tg).unwrap().tab_bar;
    app.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), tbar.x + tbar.width - 2, tbar.y);
    assert!(app.dragging_tab());
    assert_eq!(app.tab_drop_in(tg), None, "no marker in the area's bar");
    app.mouse(MouseEventKind::Up(MouseButton::Left), tbar.x + tbar.width - 2, tbar.y);
    assert_eq!(app.terminals.get(tg).unwrap().tabs.len(), 1);
    assert_eq!(app.groups.get(eg).unwrap().tabs.len(), 1);
}

/// With the editor focused, the split and group keys act on the editor's groups even while the
/// area is split; with the area focused, on the area's.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_group_keys_follow_the_focused_area() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    app.press(KeyCode::Char('\\'), cmd());
    assert_eq!(app.terminals.len(), 2);
    let area_active = app.terminals.active;
    draw(&mut app, 100, 40);
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    click(&mut app, editor.x + 2, editor.y + 1);
    assert_eq!(app.ui.focus, Focus::Editor);
    app.press(KeyCode::Char('\\'), cmd());
    assert_eq!(app.groups.len(), 2, "the editor split");
    assert_eq!(app.terminals.len(), 2, "the area didn't");
    let ids = app.groups.ids();
    assert_eq!(app.groups.active, ids[1]);
    app.test_execute(Command::FocusPreviousGroup);
    assert_eq!(app.groups.active, ids[0]);
    app.test_execute(Command::FocusGroup(2));
    assert_eq!(app.groups.active, ids[1]);
    assert_eq!(app.terminals.active, area_active, "the area's active group stays");
    assert_eq!(app.ui.focus, Focus::Editor);
    // Back in the area: FocusGroup(n) counts the area's groups.
    let shell = app.terminals.ids()[0];
    app.test_execute(Command::NewShell);
    app.test_execute(Command::FocusGroup(1));
    assert_eq!(app.terminals.active, shell);
    assert_eq!(app.groups.active, ids[1]);
    assert_eq!(app.ui.focus, Focus::Terminal);
}

/// Esc during a terminal drag cancels it: the release on an edge splits nothing.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_esc_cancels_a_terminal_drag() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 2);
    let g = app.terminals.active;
    let before = titles(&app, g);
    let (x, y) = edge_point(app.hits.group(g).unwrap().editor, DropSide::Right);
    let from = tab_point(&app, g, 0);
    app.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.dragging_tab());
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(app.terminals.len(), 1);
    assert_eq!(titles(&app, g), before);
}

/// The area hiding with two empty groups left keeps them; the next shell opens in the active
/// one, showing the area and focusing it, with the cursor in that group.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_new_shell_after_hiding_with_empty_groups() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    app.press(KeyCode::Char('\\'), cmd());
    app.press(KeyCode::Char('\\'), cmd() | KeyModifiers::ALT);
    app.test_execute(Command::FocusGroup(1));
    app.press(KeyCode::Char('w'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert!(!app.ui.terminal_visible);
    assert_eq!(app.ui.focus, Focus::Editor);
    assert_eq!(app.terminals.len(), 2, "the empty groups are kept");
    draw(&mut app, 100, 40);
    app.test_execute(Command::NewShell);
    assert!(app.ui.terminal_visible);
    assert_eq!(app.ui.focus, Focus::Terminal);
    assert_eq!(app.terminals.active_group().tabs.len(), 1);
    assert_eq!(app.shell_tabs().count(), 1);
    let term = draw(&mut app, 100, 40);
    let screen = app.hits.group(app.terminals.active).unwrap().editor;
    assert!(screen.contains(term.backend().cursor_position()));
}

// ---- term_area-c: between the area and editor groups, the startup terminal ----

/// Sends `echo $((n*n))` to the focused shell and waits for the result in the shell `tab` of
/// `group` (an editor group's or the area's).
async fn shell_answers(app: &mut App, group: GroupId, n: u32) {
    app.type_text(&format!("echo $(({n}*{n}))"));
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    let want = (n * n).to_string();
    let seen = move |a: &App| {
        (if is_terminal_group(group) {
            &a.terminals
        } else {
            &a.groups
        })
        .get(group)
        .unwrap()
        .tabs
        .iter()
        .filter_map(|t| t.shell())
        .any(|s| {
            let text = s.run.screen().screen().contents();
            // Typed before the prompt, the tty echoes the command first and the answer lands after the prompt.
            text.lines().any(|l| l.trim().trim_start_matches("$ ") == want)
        })
    };
    assert!(app.settle(seen).await, "no answer from the shell");
}

/// A terminal dropped on an editor group's bar, screen centre or edge band becomes an editor
/// shell tab of that group, alive (it answers keys), and the area hides with its last terminal.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_drag_a_terminal_onto_an_editor_group() {
    let (_d, root, state) = dirs();
    for target in ["bar", "centre", "edge"] {
        let mut app = area_app(&root, &state, 2);
        let eg = app.groups.active;
        let tg = app.terminals.active;
        let editor = app.hits.group(eg).unwrap().editor;
        let bar = app.hits.group(eg).unwrap().tab_bar;
        let to = match target {
            "bar" => (bar.x + bar.width - 2, bar.y),
            "centre" => centre(editor),
            _ => edge_point(editor, DropSide::Right),
        };
        let from = tab_point(&app, tg, 0);
        app.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
        app.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
        if target != "bar" {
            let term = draw(&mut app, 100, 40);
            assert!(shaded_cells(&app, &term, editor) > 0, "{target}: the zone is shaded");
        }
        app.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1);
        let eg_now = if target == "edge" {
            assert_eq!(app.groups.len(), 2, "the edge splits the editor");
            *app.groups.ids().iter().find(|&&g| g != eg).unwrap()
        } else {
            assert_eq!(app.groups.len(), 1);
            eg
        };
        assert_eq!(app.groups.active, eg_now, "{target}");
        assert_eq!(
            app.terminals.get(tg).unwrap().tabs.len(),
            1,
            "{target}: one terminal left"
        );
        assert!(app.ui.terminal_visible);
        let moved = app.groups.get(eg_now).unwrap();
        assert!(moved.active_shell().is_some(), "{target}: an editor Tab::Shell, active");
        assert_eq!(app.ui.focus, Focus::Editor);
        assert_eq!(app.context(), Context::Shell);
        shell_answers(&mut app, eg_now, 12).await;
        assert_eq!(app.shell_tabs().count(), 2);
        // The last terminal leaving hides the area.
        draw(&mut app, 100, 40);
        let from = tab_point(&app, tg, 0);
        let bar = app.hits.group(eg_now).unwrap().tab_bar;
        drag(&mut app, from, (bar.x + bar.width - 2, bar.y));
        assert!(!app.ui.terminal_visible, "{target}: the area hides");
        assert_eq!(app.shell_tabs().count(), 2);
        assert_eq!(app.ui.focus, Focus::Editor);
    }
}

/// The two palette commands move the shell both ways, with a real palette pick for one; a text
/// tab (or no terminal) is refused with a message.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_move_commands() {
    let (_d, root, state) = dirs();
    let mut app = area_app(&root, &state, 1);
    let eg = app.groups.active;
    let tg = app.terminals.active;
    for c in [Command::MoveTerminalToEditor, Command::MoveTerminalToArea] {
        assert!(Command::all().contains(&c));
    }
    assert_eq!(Command::MoveTerminalToEditor.name(), "terminal.moveToEditor");
    assert_eq!(Command::MoveTerminalToArea.name(), "terminal.moveToTerminal");
    // A text tab is not a terminal.
    app.test_execute(Command::MoveTerminalToArea);
    assert!(app.message.as_ref().is_some_and(|m| m.text.contains("not a terminal")));
    assert_eq!(app.groups.get(eg).unwrap().tabs.len(), 1);

    // Into the editor area, through the palette.
    app.press(KeyCode::Char('p'), cmd() | KeyModifiers::SHIFT);
    app.type_text("Move Terminal into Editor Area");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.palette.is_none());
    assert_eq!(app.groups.get(eg).unwrap().tabs.len(), 2);
    assert!(app.group().active_shell().is_some());
    assert!(!app.ui.terminal_visible, "the area hid with its last terminal");
    assert_eq!(app.ui.focus, Focus::Editor);
    shell_answers(&mut app, eg, 9).await;
    // With none left there is nothing to move.
    app.test_execute(Command::MoveTerminalToEditor);
    assert!(app.message.as_ref().is_some_and(|m| m.text.contains("No terminal")));

    // Back: the area shows again, focused, the shell alive.
    app.test_execute(Command::MoveTerminalToArea);
    assert!(app.ui.terminal_visible);
    assert_eq!(app.ui.focus, Focus::Terminal);
    assert_eq!(app.terminals.get(tg).unwrap().tabs.len(), 1);
    assert_eq!(app.groups.get(eg).unwrap().tabs.len(), 1);
    assert_eq!(app.shell_tabs().count(), 1);
    draw(&mut app, 100, 40);
    shell_answers(&mut app, tg, 11).await;

    // With the area split, the active terminal group's shell is the one that moves; its emptied
    // group closes, the other's shell stays and the area stays shown.
    app.test_execute(Command::SplitRight);
    app.test_execute(Command::NewShell);
    let second = app.terminals.active;
    assert_ne!(second, tg);
    assert_eq!(app.terminals.len(), 2);
    app.test_execute(Command::MoveTerminalToEditor);
    assert_eq!(app.terminals.ids(), vec![tg]);
    assert_eq!(app.terminals.active, tg);
    assert_eq!(app.terminals.get(tg).unwrap().tabs.len(), 1);
    assert!(app.ui.terminal_visible);
    assert!(app.groups.get(eg).unwrap().active_shell().is_some());
    assert_eq!((app.groups.len(), app.ui.focus), (1, Focus::Editor));
}

/// A new session opens one terminal in the shown area (the focus stays on the editor), a
/// restored session isn't disturbed and brings no shell back, the saved file holds none, and
/// single file mode opens none.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_startup_terminal() {
    let (_d, root, state) = dirs();
    std::fs::write(root.join("a.txt"), "hello").unwrap();
    let build = |mode: crate::app::Mode| {
        let ws = crate::workspace::Workspace::load(root.clone());
        let trust = crate::TrustStore::load(state.join("trust.toml"));
        let initial = Some(crate::app::InitialFile {
            buffer: Buffer::open(&root.join("a.txt")).unwrap(),
            goto: None,
        });
        let identity = App::start_identity(&ws, mode, initial.as_ref());
        let mut app = App::new_in(mode, ws, trust, initial, identity, true);
        app.tasks.shell = "/bin/sh".into();
        app.state_dir = Some(state.clone());
        app.startup_terminal = true;
        app.test_start();
        app
    };
    let mut app = build(crate::app::Mode::Workspace);
    assert!(app.settle(|a| !a.session.loading()).await);
    assert!(app.ui.terminal_visible);
    assert_eq!(app.shell_tabs().count(), 1);
    assert_eq!(app.terminals.active_group().tabs.len(), 1);
    assert_eq!(app.ui.focus, Focus::Editor, "typing still goes to the file");
    draw(&mut app, 100, 30);
    assert!(app.hits.regions.terminal.is_some());
    app.test_execute(Command::Quit);
    app.shutdown().await;
    let saved = std::fs::read_to_string(crate::session::file(&state, &root)).unwrap();
    assert!(
        !saved.to_lowercase().contains("shell") && !saved.contains("terminal"),
        "{saved}"
    );

    // The next session restores the file and opens exactly one terminal, not two.
    let mut app = build(crate::app::Mode::Workspace);
    assert!(app.settle(|a| !a.session.loading()).await);
    assert_eq!(app.shell_tabs().count(), 1);
    assert_eq!(app.session.restored, 1);
    assert!(
        app.groups.active_group().active_tab().is_some(),
        "the file is still open"
    );
    app.shutdown().await;

    let app = build(crate::app::Mode::SingleFile);
    assert_eq!(app.shell_tabs().count(), 0, "single file mode: none");
    assert!(!app.ui.terminal_visible);
}

/// The task's Done when, on one app.
#[tokio::test(flavor = "multi_thread")]
async fn term_area_done_when() {
    let (_d, root, state) = dirs();
    let mut app = app_in(&root, &state);
    app.test_open(Buffer::new_file(&root.join("a.txt")));
    app.ui.terminal_height = 20;
    let editor_width = {
        draw(&mut app, 100, 40);
        app.hits.regions.editor.width
    };
    // Ctrl+` opens a shell in the area, the editor keeping its width.
    app.press(KeyCode::Char('`'), KeyModifiers::CONTROL);
    draw(&mut app, 100, 40);
    let tg = app.terminals.active;
    assert_eq!(app.terminals.get(tg).unwrap().tabs.len(), 1);
    assert_eq!(app.hits.regions.editor.width, editor_width);
    assert!(app.hits.regions.editor.height >= MIN_EDITOR_HEIGHT);
    // A second shell is a tab, reordered by dragging.
    app.test_execute(Command::NewShell);
    draw(&mut app, 100, 40);
    let first = titles(&app, tg);
    let hit = app.hits.group(tg).unwrap();
    let to = (hit.tabs[1].x1.saturating_sub(1), hit.tab_bar.y);
    drag_tab(&mut app, tg, to);
    assert_eq!(titles(&app, tg), [first[1].clone(), first[0].clone()]);
    // Split by Cmd+\ into two live shells side by side (the new group empty, so move one in
    // by dragging onto the first group's edge instead: a second split).
    draw(&mut app, 100, 40);
    let screen = app.hits.group(tg).unwrap().editor;
    let from = tab_point(&app, tg, 0);
    drag(&mut app, from, edge_point(screen, DropSide::Right));
    assert_eq!(app.terminals.len(), 2);
    draw(&mut app, 100, 40);
    let ids = app.terminals.ids();
    let (a, b) = (
        app.hits.group(ids[0]).unwrap().editor,
        app.hits.group(ids[1]).unwrap().editor,
    );
    assert!(a.x + a.width <= b.x, "side by side");
    // Both live: each answers in its own group.
    for (i, &g) in ids.iter().enumerate() {
        assert_eq!(app.terminals.get(g).unwrap().tabs.len(), 1);
        app.test_execute(Command::FocusGroup(i as u8 + 1));
        assert_eq!((app.terminals.active, app.ui.focus), (g, Focus::Terminal));
        shell_answers(&mut app, g, 5 + i as u32).await;
    }
    // Dropping a text tab into the area is refused.
    draw(&mut app, 100, 40);
    let eg = app.groups.active;
    let from = tab_point(&app, eg, 0);
    let to = centre(app.hits.group(tg).unwrap().editor);
    drag(&mut app, from, to);
    assert_eq!(app.groups.get(eg).unwrap().tabs.len(), 1);
    assert_eq!(app.shell_tabs().count(), 2);
    // A terminal dragged onto the editor moves there; the command brings it back.
    draw(&mut app, 100, 40);
    let from = tab_point(&app, tg, 0);
    let to = centre(app.hits.group(eg).unwrap().editor);
    drag(&mut app, from, to);
    assert!(app.groups.get(eg).unwrap().active_shell().is_some());
    assert_eq!(app.terminals.len(), 1, "its emptied terminal group closed");
    app.test_execute(Command::MoveTerminalToArea);
    assert!(app.groups.get(eg).unwrap().tabs.iter().all(|t| t.shell().is_none()));
    assert_eq!(
        app.terminals.active_group().tabs.len(),
        2,
        "both shells in the area again"
    );
    assert_eq!(app.ui.focus, Focus::Terminal);
    // Closing the last terminal hides the area; with no editor tabs the splash shows.
    app.test_execute(Command::CloseTab);
    assert!(app.ui.terminal_visible, "one terminal left");
    app.test_execute(Command::CloseTab);
    assert!(!app.ui.terminal_visible, "the last closed: hidden");
    assert_eq!(app.ui.focus, Focus::Editor);
    app.test_execute(Command::CloseTab);
    let term = draw(&mut app, 100, 40);
    assert!(app.hits.regions.terminal.is_none());
    let all: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(all.contains("Jim's Editor"), "the splash shows");
    // (A new session opening one terminal: `term_area_startup_terminal`.)
}
