//! Markdown preview (markdown_preview-c): the tab and the key. Cmd+Alt+V (Ctrl+Alt+V on Linux)
//! on a Markdown tab opens a read-only preview in the group to the right (a new split when there
//! is none), the source keeps the focus, pressing again focuses the existing preview, other tabs
//! get a notice; the preview draws six distinguishable heading levels, a highlighted code block
//! on its own background, a box-drawn table that follows the width (wide characters too);
//! movement, selection and copy work, every editing command, typing and paste do nothing; splits
//! copy it, closing it never prompts.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::app::{App, Focus};
use crate::command::Command;
use crate::group::Tab;
use crate::markdown_tab::MarkdownTab;
use crate::milestone10_tests::dirs;
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

const README: &str = "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n\n\
Some *emphasis* and `code` text.\n\n```rust\nfn main() {}\n```\n\n    indented block\n\n\
| Left | Right | Wide |\n|:-----|------:|:----:|\n| a | 1 | \u{4e2d}\u{6587} |\n| bb | 22 | x |\n\n\
- [x] done\n- [ ] todo\n";

fn mods() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER | M::ALT
    } else {
        M::CONTROL | M::ALT
    }
}

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

async fn md_app() -> (tempfile::TempDir, PathBuf, App) {
    let (dir, root, state) = dirs();
    fs::write(root.join("README.md"), README).unwrap();
    fs::write(root.join("notes.txt"), "plain\n").unwrap();
    let mut app = App::new(
        Workspace::load(root.clone()),
        TrustStore::load(state.join("trust.toml")),
        None,
        true,
    );
    app.test_start();
    app.ui.side_visible = false;
    app.ui.focus = Focus::Editor;
    (dir, root, app)
}

async fn quick_open(app: &mut App, name: &str) {
    app.press(KeyCode::Char('p'), cmd());
    app.type_text(name);
    let found = app
        .settle(|a| {
            a.palette
                .as_ref()
                .is_some_and(|p| p.rows.first().is_some_and(|r| r.label == name))
        })
        .await;
    assert!(found, "quick open lists {name}");
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.name() == name))
            .await
    );
}

/// Opens README.md and its preview; returns the source's group id.
async fn open_preview(app: &mut App) -> crate::layout::GroupId {
    quick_open(app, "README.md").await;
    let source = app.groups.active;
    app.press(KeyCode::Char('v'), mods());
    source
}

fn draw(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// The preview's text rect in the group holding it.
fn rows_rect(app: &App) -> (crate::layout::GroupId, Rect) {
    let (g, _) = app.groups.find_markdown(source_id(app)).expect("a preview");
    (g, app.hits.group(g).unwrap().text)
}

fn source_id(app: &App) -> u64 {
    app.groups
        .ids()
        .into_iter()
        .flat_map(|g| app.groups.get(g).unwrap().tabs.iter())
        .find_map(|t| t.markdown().map(|m| m.source))
        .expect("a preview")
}

fn preview(app: &App) -> &MarkdownTab {
    let (g, i) = app.groups.find_markdown(source_id(app)).unwrap();
    app.groups.get(g).unwrap().tabs[i].markdown().unwrap()
}

fn row_text(term: &Terminal<TestBackend>, rect: Rect, i: u16) -> String {
    let buf = term.backend().buffer();
    // A wide character's second cell is blank in the buffer: skip it.
    let mut out = String::new();
    let mut skip = 0;
    for x in rect.x..rect.x + rect.width {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let sym = buf[(x, rect.y + i)].symbol();
        skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
        out.push_str(sym);
    }
    out.trim_end().to_string()
}

fn row_with(term: &Terminal<TestBackend>, rect: Rect, needle: &str) -> Option<u16> {
    (0..rect.height).find(|&i| row_text(term, rect, i).contains(needle))
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_opens_to_the_right_and_focuses_on_the_second_press() {
    let (_d, _root, mut app) = md_app().await;
    let source = open_preview(&mut app).await;
    assert_eq!(app.groups.len(), 2, "split to the right");
    let (g, i) = app.groups.find_markdown(source_id(&app)).unwrap();
    assert_ne!(g, source);
    assert_eq!(app.groups.ids(), vec![source, g], "the preview is after the source");
    assert_eq!(app.groups.get(g).unwrap().active, i);
    assert_eq!(app.groups.active, source, "the source keeps the focus");
    assert_eq!(app.context(), crate::config::keymap::Context::Editor);
    // Pressing again focuses the preview: no second one.
    app.press(KeyCode::Char('v'), mods());
    assert_eq!(app.groups.active, g);
    assert_eq!(app.context(), crate::config::keymap::Context::Markdown);
    assert_eq!(app.groups.len(), 2);
    app.test_execute(Command::FocusGroup(1));
    app.press(KeyCode::Char('v'), mods());
    assert_eq!(app.groups.active, g);
    let n = app
        .groups
        .ids()
        .iter()
        .map(|&g| app.groups.get(g).unwrap().tabs.len())
        .sum::<usize>();
    assert_eq!(n, 2, "one source tab and one preview");
    // Another source group to the right is reused: close the preview, open again.
    app.test_execute(Command::CloseTab);
    app.test_execute(Command::FocusGroup(1));
    app.press(KeyCode::Char('v'), mods());
    assert_eq!(app.groups.len(), 2, "the group on the right is reused");
    assert_eq!(preview(&app).title(), "Preview README.md");
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_needs_a_markdown_tab() {
    let (_d, _root, mut app) = md_app().await;
    quick_open(&mut app, "notes.txt").await;
    app.press(KeyCode::Char('v'), mods());
    assert_eq!(app.groups.len(), 1);
    assert_eq!(app.group().tabs.len(), 1, "no new tab");
    let term = draw(&mut app, 200, 12);
    let area = term.backend().buffer().area;
    let bar: String = (0..area.width)
        .map(|x| term.backend().buffer()[(x, area.height - 1)].symbol())
        .collect();
    assert!(bar.contains("needs a Markdown file"), "{bar}");
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_draws_headings_code_and_table() {
    let (_d, _root, mut app) = md_app().await;
    open_preview(&mut app).await;
    let term = draw(&mut app, 120, 40);
    let (_, rect) = rows_rect(&app);
    let buf = term.backend().buffer();
    // Six heading levels, each in a style of its own.
    let mut styles = Vec::new();
    for word in ["One", "Two", "Three", "Four", "Five", "Six"] {
        let row = row_with(&term, rect, word).unwrap_or_else(|| panic!("{word} drawn"));
        let x = rect.x + row_text(&term, rect, row).find(word).unwrap() as u16;
        let cell = &buf[(x, rect.y + row)];
        styles.push((cell.fg, cell.modifier));
    }
    for i in 0..styles.len() {
        for j in i + 1..styles.len() {
            assert_ne!(styles[i], styles[j], "heading {} vs {}", i + 1, j + 1);
        }
    }
    // The code block on its own background, the text around it not.
    let theme = crate::theme::Theme::dark_plus(true);
    let code_bg = theme.get("markdown.codeblock").bg;
    let row = row_with(&term, rect, "fn main").expect("code drawn");
    assert_eq!(buf[(rect.x, rect.y + row)].bg, code_bg.unwrap());
    assert_eq!(
        buf[(rect.x + rect.width - 1, rect.y + row)].bg,
        code_bg.unwrap(),
        "to the full width"
    );
    let text_row = row_with(&term, rect, "emphasis").unwrap();
    assert_ne!(buf[(rect.x, rect.y + text_row)].bg, code_bg.unwrap());
    // Highlighted: `fn` is not in the plain code colour.
    let x = rect.x + row_text(&term, rect, row).find("fn").unwrap() as u16;
    let plain = buf[(rect.x + rect.width - 2, rect.y + row)].fg;
    let kw = buf[(x, rect.y + row)].fg;
    let main = buf[(x + 3, rect.y + row)].fg;
    assert!(kw != main || kw != plain, "the Rust block is highlighted");
    // The table: a box-drawn grid, every row the same width, also with the wide characters.
    let top = row_with(&term, rect, "┌").expect("table top");
    let bottom = row_with(&term, rect, "└").expect("table bottom");
    let widths: Vec<usize> = (top..=bottom)
        .map(|i| unicode_width::UnicodeWidthStr::width(row_text(&term, rect, i).as_str()))
        .collect();
    assert!(widths.iter().all(|&w| w == widths[0]), "{widths:?}");
    assert!(row_with(&term, rect, "\u{4e2d}\u{6587}").is_some());
    // Narrowing the window keeps the grid aligned.
    let term = draw(&mut app, 50, 60);
    let (_, rect) = rows_rect(&app);
    let top = row_with(&term, rect, "┌").expect("table top");
    let bottom = row_with(&term, rect, "└").expect("table bottom");
    let widths: Vec<usize> = (top..=bottom)
        .map(|i| unicode_width::UnicodeWidthStr::width(row_text(&term, rect, i).as_str()))
        .collect();
    assert!(widths.iter().all(|&w| w == widths[0]), "narrow: {widths:?}");
    assert!(row_with(&term, rect, "[x]").is_some() || row_with(&term, rect, "done").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_moves_selects_and_copies() {
    let (_d, _root, mut app) = md_app().await;
    open_preview(&mut app).await;
    app.press(KeyCode::Char('v'), mods());
    draw(&mut app, 100, 12);
    // Heading one is the first line.
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Right, M::SHIFT);
    app.press(KeyCode::Right, M::SHIFT);
    assert_eq!(preview(&app).copy_text().as_deref(), Some("One"));
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "One");
    app.press(KeyCode::Left, M::NONE);
    assert!(preview(&app).selection().is_none());
    app.press(KeyCode::Char('a'), cmd());
    assert!(preview(&app).copy_text().unwrap().contains("fn main() {}"));
    // Page down / end scroll the view with the caret.
    app.press(KeyCode::Esc, M::NONE);
    app.test_execute(Command::Move {
        motion: crate::command::Motion::DocEnd,
        select: false,
    });
    draw(&mut app, 100, 12);
    let p = preview(&app);
    assert!(p.top > 0 && p.cursor.0 >= p.top && p.cursor.0 < p.top + p.height);
    let top = p.top;
    // The wheel scrolls without moving the caret.
    let (_, rect) = rows_rect(&app);
    let cursor = preview(&app).cursor;
    app.mouse(MouseEventKind::ScrollUp, rect.x + 2, rect.y + 2);
    assert!(preview(&app).top < top);
    assert_eq!(preview(&app).cursor, cursor);
    // A click puts the caret, a drag selects.
    draw(&mut app, 100, 12);
    app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, rect.y);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), rect.x + 4, rect.y + 1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), rect.x + 4, rect.y + 1);
    assert!(preview(&app).selection().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_cannot_be_edited() {
    let (_d, _root, mut app) = md_app().await;
    open_preview(&mut app).await;
    app.press(KeyCode::Char('v'), mods());
    draw(&mut app, 100, 30);
    let source = source_id(&app);
    let before = app.buffers.get(source).unwrap().rope.to_string();
    let skip = |c: &Command| {
        matches!(
            c,
            Command::Quit
                | Command::CloseTab
                | Command::NextTab
                | Command::PrevTab
                | Command::MoveTabToNextGroup
                | Command::MoveTabToPreviousGroup
                | Command::MoveTabLeft
                | Command::MoveTabRight
                | Command::OpenUserSettings
                | Command::OpenLogFile
                | Command::Tools(_)
                | Command::NewShell
        )
    };
    app.log_dir = _root.join("../state/logs");
    app.config_dir = _root.join("../state/config");
    let all = Command::all();
    assert!(all.len() > 130, "every command");
    assert!(all.iter().any(|c| matches!(c, Command::Paste)) && all.iter().any(|c| matches!(c, Command::Cut)));
    for c in all.into_iter().filter(|c| !skip(c)) {
        app.test_execute(c);
        app.prompt = None;
        app.palette = None;
        // Back to the preview, focused.
        let (g, i) = app.groups.find_markdown(source).expect("still there");
        app.groups.get_mut(g).unwrap().active = i;
        app.groups.active = g;
        app.ui.focus = Focus::Editor;
        draw(&mut app, 100, 30);
        draw(&mut app, 40, 30);
        assert_eq!(
            app.buffers.get(source).unwrap().rope.to_string(),
            before,
            "{}",
            c.name()
        );
    }
    app.type_text("abc");
    app.press(KeyCode::Enter, M::NONE);
    app.press(KeyCode::Backspace, M::NONE);
    app.handle_test_event(Event::Paste("pasted".into()));
    assert_eq!(app.buffers.get(source).unwrap().rope.to_string(), before);
    assert!(!app.buffers.get(source).unwrap().is_dirty());
    // The previews themselves (splits made copies): the same text, laid out as it renders.
    draw(&mut app, 100, 30);
    for g in app.groups.ids() {
        for m in app.groups.get(g).unwrap().tabs.iter().filter_map(Tab::markdown) {
            assert_eq!(&*m.text, README);
            assert_eq!(
                m.doc,
                crate::markdown::render(README, m.width),
                "the rendered text is unchanged"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_split_copy_closes_without_prompt_and_quit_is_clean() {
    let (_d, _root, mut app) = md_app().await;
    open_preview(&mut app).await;
    app.press(KeyCode::Char('v'), mods());
    app.test_execute(Command::SplitRight);
    assert!(app.group().active_markdown().is_some(), "the split holds a copy");
    let ids: Vec<u64> = app
        .groups
        .ids()
        .into_iter()
        .flat_map(|g| {
            app.groups
                .get(g)
                .unwrap()
                .tabs
                .iter()
                .filter_map(Tab::markdown)
                .map(|m| m.id)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    draw(&mut app, 120, 30);
    app.test_execute(Command::CloseTab);
    assert!(app.prompt.is_none(), "closing a preview never asks");
    assert!(
        app.groups.find_markdown(source_id(&app)).is_some(),
        "the other copy stays"
    );
    draw(&mut app, 120, 30);
    app.test_execute(Command::Quit);
    assert!(
        app.prompt.is_none() && app.quitting(),
        "a preview never holds up a quit"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_wide_table_scrolls_sideways() {
    let (_d, root, mut app) = md_app().await;
    let header = (0..10).map(|i| format!("| c{i} ")).collect::<String>() + "|\n";
    let rule = "|---".repeat(10) + "|\n";
    let row = (0..10).map(|i| format!("| {i}{i}{i}{i}{i} ")).collect::<String>() + "|\n";
    fs::write(root.join("wide.md"), format!("Intro\n\n{header}{rule}{row}")).unwrap();
    quick_open(&mut app, "wide.md").await;
    app.press(KeyCode::Char('v'), mods());
    app.press(KeyCode::Char('v'), mods());
    draw(&mut app, 100, 20);
    draw(&mut app, 100, 20);
    let (g, rect) = rows_rect(&app);
    let p = preview(&app);
    assert!(p.doc.overflows(p.view_width), "the grid is wider than the view");
    let bar = app.hits.group(g).unwrap().hscrollbar.expect("a horizontal bar");
    assert_eq!(bar.track.y, rect.y + rect.height, "under the rows");
    // Shift+wheel scrolls sideways, the plain wheel doesn't.
    app.mouse(MouseEventKind::ScrollDown, rect.x + 1, rect.y + 1);
    assert_eq!(preview(&app).left, 0);
    app.mouse_with(MouseEventKind::ScrollDown, rect.x + 1, rect.y + 1, M::SHIFT);
    assert!(preview(&app).left > 0);
    app.mouse_with(MouseEventKind::ScrollUp, rect.x + 1, rect.y + 1, M::SHIFT);
    assert_eq!(preview(&app).left, 0);
    // A press at the bar's right end scrolls to the grid's end (one blank column after it).
    let end = bar.track.x + bar.track.width - 1;
    app.mouse(MouseEventKind::Down(MouseButton::Left), end, bar.track.y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), end, bar.track.y);
    let term = draw(&mut app, 100, 20);
    let p = preview(&app);
    assert_eq!(p.left, p.doc.max_width() + 1 - p.view_width);
    let top = row_with(&term, rect, "┐").expect("the grid's right edge in view");
    assert!(row_text(&term, rect, top).ends_with('┐'));
    // The caret's line start brings it back.
    app.test_execute(Command::Move {
        motion: crate::command::Motion::DocStart,
        select: false,
    });
    draw(&mut app, 100, 20);
    assert_eq!(preview(&app).left, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn markdown_preview_status_bar_names_the_source() {
    let (_d, _root, mut app) = md_app().await;
    open_preview(&mut app).await;
    app.press(KeyCode::Char('v'), mods());
    let term = draw(&mut app, 120, 20);
    let area = term.backend().buffer().area;
    let bar: String = (0..area.width)
        .map(|x| term.backend().buffer()[(x, area.height - 1)].symbol())
        .collect();
    assert!(bar.contains("Markdown preview of README.md"), "{bar}");
    assert!(bar.contains("Ln 1, Col 1"), "{bar}");
}
