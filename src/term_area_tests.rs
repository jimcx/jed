//! End-to-end tests for term_area-a (the terminal area's region and its first terminal): the
//! area's layout (full width above the status bar, height clamped), "Terminal: New Shell"
//! (Ctrl+`) opening a shell in it with the editor groups keeping the rest, its keys and
//! `Context::Shell`, a second shell as a tab, closing the last one hiding the area (and the
//! Jed splash for an empty editor area), dragging its top border (and the bottom panel's above
//! it), clicks, the PTY's size at open, Cmd+W / tab cycling and the terminal cursor following
//! the focus, its scroll bar, and editor-group shell tabs still working beside it.

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
    let size = app
        .terminals
        .active_group()
        .active_shell()
        .unwrap()
        .run
        .screen()
        .screen()
        .size();
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
    let rows = app
        .terminals
        .active_group()
        .active_shell()
        .unwrap()
        .run
        .screen()
        .screen()
        .size()
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
        shell.run.screen().screen().size()
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
    app.press(KeyCode::Char('w'), KeyModifiers::SUPER);
    assert_eq!(app.group().tabs.len(), 1);
    assert_eq!(app.terminals.active_group().tabs.len(), 2);
    // Back in the area, Cmd+W closes its tab and leaves the editor's text tab.
    click(&mut app, area.x + 1, area.y + 1);
    assert_eq!(app.ui.focus, Focus::Terminal);
    app.press(KeyCode::Char('w'), KeyModifiers::SUPER);
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
