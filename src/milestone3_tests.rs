//! Milestone 3 end to end: splits, shared buffers across groups, focus, closing groups,
//! divider dragging. Drives the real `App` through key and mouse events.

use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode as K, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, InitialFile, Prompt};
use crate::buffer::Buffer;
use crate::layout::GroupId;
use crate::workspace::trust::TrustStore;
use crate::workspace::{self, Workspace};

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn open_app(root: &Path, file: &str) -> App {
    let buffer = Buffer::open(&root.join(file)).unwrap();
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(root.join("trust.toml"));
    let mut app = App::new(ws, trust, Some(InitialFile { buffer, goto: None }), true);
    app.test_start();
    app
}

/// Cursor heads of a group's active view.
fn heads(app: &App, group: GroupId) -> Vec<usize> {
    let g = app.groups.get(group).unwrap();
    g.tabs[g.active]
        .text()
        .unwrap()
        .view
        .selections
        .iter()
        .map(|s| s.head.0)
        .collect()
}

fn text(app: &App) -> String {
    app.active_buffer().unwrap().rope.to_string()
}

fn draw(app: &mut App, term: &mut Terminal<TestBackend>) {
    term.draw(|f| crate::ui::render(f, app)).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_3_splits_and_shared_buffers() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    fs::write(root.join("a.txt"), "hello\nworld\n").unwrap();
    let mut app = open_app(&root, "a.txt");
    let first = app.groups.active;

    // Cursor to line 2, then split right: the new group shows the same buffer, same cursor.
    app.press(K::Down, M::NONE);
    app.press(K::Char('\\'), cmd());
    let second = app.groups.active;
    assert_ne!(first, second);
    assert_eq!(app.groups.ids(), vec![first, second]);
    assert_eq!(app.groups.tabs_showing(app.group().tabs[0].buffer().unwrap()), 2);
    assert_eq!(heads(&app, second), vec![6]);

    // Type in the second group above the first group's cursor: the first group's cursor follows.
    app.press(K::Up, M::NONE);
    app.type_text("XY");
    assert_eq!(text(&app), "XYhello\nworld\n");
    assert_eq!(heads(&app, first), vec![8]);
    assert_eq!(heads(&app, second), vec![2]);

    // Undo in the second group: the first group's cursor goes back too.
    app.press(K::Char('z'), cmd());
    assert_eq!(text(&app), "hello\nworld\n");
    assert_eq!(heads(&app, first), vec![6]);

    // Focus the first group and type there; the second group sees the text.
    app.press(K::Char('1'), cmd());
    assert_eq!(app.groups.active, first);
    app.type_text("Z");
    assert_eq!(heads(&app, first), vec![7]);
    assert_eq!(heads(&app, second), vec![0], "an edit after its cursor leaves it alone");
    assert!(app.active_buffer().unwrap().is_dirty());

    // Cmd+3 with two groups splits again; closing its only tab closes the group (no prompt:
    // the buffer is still shown elsewhere).
    app.press(K::Char('3'), cmd());
    assert_eq!(app.groups.len(), 3);
    app.press(K::Char('w'), cmd());
    assert!(app.prompt.is_none());
    assert_eq!(app.groups.len(), 2);

    // Same for the second group.
    app.press(K::Char('2'), cmd());
    assert_eq!(app.groups.active, second);
    app.press(K::Char('w'), cmd());
    assert!(app.prompt.is_none());
    assert_eq!(app.groups.len(), 1);
    assert_eq!(app.groups.active, first);

    // Last tab showing the unsaved buffer: asked. Don't Save drops it; the last group stays.
    app.press(K::Char('w'), cmd());
    assert!(matches!(app.prompt, Some(Prompt::SaveBeforeClose { .. })));
    app.press(K::Char('d'), M::NONE);
    assert_eq!(app.groups.len(), 1);
    assert!(app.group().tabs.is_empty());
    assert!(app.buffers.dirty().is_empty(), "buffer dropped");
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "hello\nworld\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn save_from_close_prompt_in_one_of_two_groups() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    fs::write(root.join("a.txt"), "a\n").unwrap();
    let mut app = open_app(&root, "a.txt");
    app.type_text("1");
    app.press(K::Char('\\'), cmd());
    let second = app.groups.active;

    // Closing one of two tabs on a dirty buffer doesn't ask; closing the last one does.
    app.press(K::Char('w'), cmd());
    assert!(app.prompt.is_none());
    assert_ne!(app.groups.active, second);
    app.press(K::Char('w'), cmd());
    assert!(matches!(app.prompt, Some(Prompt::SaveBeforeClose { .. })));
    app.press(K::Char('s'), M::NONE);
    assert!(app.settle(|a| a.group().tabs.is_empty()).await);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "1a\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn split_down_mouse_focus_and_divider_drag() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    let mut app = open_app(&root, "a.txt");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();

    app.press(K::Char('\\'), cmd() | M::ALT);
    let (top, bottom) = (app.groups.ids()[0], app.groups.ids()[1]);
    assert_eq!(app.groups.active, bottom);
    draw(&mut app, &mut term);

    let editor = |app: &App, g| app.hits.group(g).unwrap().editor;
    let top_rect = editor(&app, top);
    let bottom_rect = editor(&app, bottom);
    assert!(
        top_rect.y + top_rect.height < bottom_rect.y,
        "stacked with a divider between"
    );

    // Clicking in the top group focuses it.
    app.mouse(MouseEventKind::Down(MouseButton::Left), top_rect.x + 8, top_rect.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), top_rect.x + 8, top_rect.y);
    assert_eq!(app.groups.active, top);

    // Dragging the divider up shrinks the top group.
    let div = app.hits.dividers[0].rect;
    app.mouse(MouseEventKind::Down(MouseButton::Left), div.x + 5, div.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), div.x + 5, div.y - 6);
    app.mouse(MouseEventKind::Up(MouseButton::Left), div.x + 5, div.y - 6);
    draw(&mut app, &mut term);
    assert_eq!(editor(&app, top).height, top_rect.height - 6);

    // The screen shows two tab bars and a horizontal divider.
    let b = term.backend().buffer();
    let row = |y: u16| (0..100).map(|x| b[(x, y)].symbol().to_string()).collect::<String>();
    let new_div = app.hits.dividers[0].rect;
    assert!(row(new_div.y).contains("────"), "{}", row(new_div.y));
    assert!(row(0).contains("a.txt") && row(new_div.y + 1).contains("a.txt"));
}

/// Borders of different layouts meet in junction glyphs: a horizontal divider ends on the side
/// panel's edge in `┤`, and a vertical divider in `┬` / `┴` where the panel's rule joins it.
#[tokio::test(flavor = "multi_thread")]
async fn borders_join_with_junction_glyphs() {
    let dir = tempfile::tempdir().unwrap();
    let root = workspace::normalize(dir.path());
    fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    let mut app = open_app(&root, "a.txt");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    app.press(K::Char('\\'), cmd() | M::ALT);
    draw(&mut app, &mut term);
    let side = app.hits.regions.side.expect("the side panel is shown");
    let edge = side.x + side.width - 1;
    let div = app.hits.dividers[0].rect;
    let b = term.backend().buffer();
    assert_eq!(b[(edge, div.y)].symbol(), "├", "side edge meets the horizontal divider");
    assert_eq!(b[(edge, div.y - 1)].symbol(), "│");
    let resolved = app.borders.resolved;
    draw(&mut app, &mut term);
    assert_eq!(
        app.borders.resolved, resolved,
        "an unchanged layout is not resolved again"
    );
}
