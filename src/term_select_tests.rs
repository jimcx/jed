//! Mouse selection in a terminal (`term_select`): over a shell and a task tab whose program
//! hasn't asked for the mouse, a left drag highlights the cells it covers and Cmd/Ctrl+C copies
//! exactly that text (trailing spaces trimmed, soft-wrapped rows joined); a click clears it; new
//! output and the wheel clear it (the rows move); a program that asked for the mouse still gets
//! every event and nothing is selected. The copy's normalisation is `tasks::select`'s unit tests.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;
use vt100::MouseProtocolMode;

use crate::app::{App, Focus};
use crate::clipboard::Copied;
use crate::command::Command;
use crate::layout;
use crate::shell_tests::{COUNT_TO_200, dirs, draw, screen_text, scrollback, shell_app};
use crate::tasks::RunId;

/// The platform's Cmd (Super on macOS, Ctrl on Linux): Cmd+C / Ctrl+C copies.
fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

/// The visible row whose text is exactly `line` (trailing blanks trimmed).
fn row_of(text: &str, line: &str) -> u16 {
    text.lines()
        .position(|l| l.trim_end() == line)
        .unwrap_or_else(|| panic!("no {line:?} row in:\n{text}")) as u16
}

/// A left drag from cell `from` to cell `to` of a terminal drawn in `area` (a cell is
/// `(row, col)`, with the run at the live screen: no `top_row` shift).
fn drag(app: &mut App, area: Rect, from: (u16, u16), to: (u16, u16)) {
    let at = |(row, col): (u16, u16)| (area.x + col, area.y + row);
    let (x, y) = at(from);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    let (x, y) = at(to);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// A click (press and release on one cell).
fn click(app: &mut App, x: u16, y: u16) {
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// The drawn background of the cell at `x`, `y`.
fn bg(term: &Terminal<TestBackend>, x: u16, y: u16) -> Color {
    term.backend().buffer()[(x, y)].bg
}

/// `command` (else nothing), then the shell's `printf` turning the mouse on in `modes`
/// (`CSI ? n h`) and `cat`, which echoes every byte the mouse sends at it (ESC as `^[`).
fn echo_mouse(command: Option<&str>, modes: &[u16]) -> Vec<u8> {
    let modes: String = modes.iter().map(|m| format!("\\033[?{m}h")).collect();
    let command = command.map(|c| format!("{c}; ")).unwrap_or_default();
    format!("{command}printf '{modes}'; cat\n").into_bytes()
}

/// The task run's screen text.
fn task_text(app: &App, id: RunId) -> String {
    app.tasks.get(id).unwrap().screen().screen().contents()
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_shell_drag_highlights_and_copy() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    assert!(
        app.group()
            .active_shell()
            .unwrap()
            .run
            .write(b"printf 'alpha   \\nbeta\\n'\n")
    );
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "beta"))
            .await,
        "the command printed both lines:\n{}",
        screen_text(&app)
    );
    let text = screen_text(&app);
    let (alpha, beta) = (row_of(&text, "alpha"), row_of(&text, "beta"));

    drop(draw(&mut app, 100, 30));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    drag(&mut app, area, (alpha, 0), (beta, 4));
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        Some(crate::tasks::select::Selection {
            anchor: (alpha, 0),
            head: (beta, 4),
        }),
        "the drag selected the cells it covered"
    );

    // The cells are painted in the editor's selection background over the terminal's colours.
    let selected = app.theme.get("editor.selection").bg.unwrap();
    let term = draw(&mut app, 100, 30);
    for (row, col) in [(alpha, 0), (alpha, 6), (beta, 0), (beta, 3)] {
        assert_eq!(
            bg(&term, area.x + col, area.y + row),
            selected,
            "the selected cell ({row}, {col}) is highlighted"
        );
    }
    assert_ne!(
        bg(&term, area.x, area.y + alpha.saturating_sub(1)),
        selected,
        "the row above the selection isn't painted"
    );
    drop(term);

    // Cmd/Ctrl+C copies exactly the covered text: trailing spaces trimmed per line.
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "alpha\nbeta");
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_task_drag_highlights_and_copy() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = crate::milestone12_tests::dirs(&dir);
    let mut app = crate::milestone12_tests::open_app(
        &root,
        &state,
        Some("[tasks.select]\ncommand = '''printf 'alpha   \\nbeta\\n'; sleep 60'''\n"),
    );
    // A frame first, as the app always has: the run starts at its tab's size.
    drop(draw(&mut app, 100, 30));
    let id = app.run_task("select").unwrap();
    assert!(
        app.settle(|a| task_text(a, id).lines().any(|l| l.trim_end() == "beta"))
            .await,
        "the task printed both lines:\n{}",
        task_text(&app, id)
    );
    let text = task_text(&app, id);
    let (alpha, beta) = (row_of(&text, "alpha"), row_of(&text, "beta"));

    drop(draw(&mut app, 100, 30));
    let area = layout::task_area(app.hits.regions.panel.unwrap());
    app.ui.focus = Focus::Editor;
    drag(&mut app, area, (alpha, 0), (beta, 4));
    assert_eq!(
        app.tasks.get(id).unwrap().selection(),
        Some(crate::tasks::select::Selection {
            anchor: (alpha, 0),
            head: (beta, 4),
        }),
        "the drag selected the cells it covered"
    );
    assert_eq!(app.ui.focus, Focus::Panel, "the press focused the task tab");

    let selected = app.theme.get("editor.selection").bg.unwrap();
    let term = draw(&mut app, 100, 30);
    for (row, col) in [(alpha, 0), (beta, 0), (beta, 3)] {
        assert_eq!(
            bg(&term, area.x + col, area.y + row),
            selected,
            "the selected cell ({row}, {col}) is highlighted"
        );
    }
    drop(term);

    // No shell tab has the editor's keys: the shown task is the terminal copied.
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "alpha\nbeta");
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_ended_task_drag_uses_the_drawn_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = crate::milestone12_tests::dirs(&dir);
    let mut app = crate::milestone12_tests::open_app(
        &root,
        &state,
        Some(
            "[tasks.ended]\ncommand = '''printf 'one\\ntwo\\nthree\\nfour\\nfive\\nsix\\nseven\\neight\\nnine\\nten\\n'''\n",
        ),
    );
    drop(draw(&mut app, 100, 30));
    let id = app.run_task("ended").unwrap();
    assert!(
        crate::milestone12_tests::settle_ended(&mut app, id).await,
        "the task ended and its note was written:\n{}",
        task_text(&app, id)
    );

    // A panel shorter than the ended run's screen: it is drawn from `select::top_row` down, and a
    // drag's cells must be the screen rows it shows, not the rows from 0.
    app.ui.panel_height = layout::MIN_PANEL_HEIGHT;
    drop(draw(&mut app, 100, 30));
    let area = layout::task_area(app.hits.regions.panel.unwrap());
    let (top, width) = {
        let run = app.tasks.get(id).unwrap();
        let screen = run.screen();
        (
            crate::tasks::select::top_row(screen.screen(), area.height),
            screen.screen().size().1,
        )
    };
    assert!(top > 0, "the ended screen is taller than its area");
    let screen_row = |row: u16| -> String {
        let run = app.tasks.get(id).unwrap();
        let screen = run.screen();
        (0..width)
            .map(|col| {
                screen
                    .screen()
                    .cell(row, col)
                    .map_or(String::from(" "), |cell| cell.contents().to_string())
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    };

    let expected = format!(
        "{}\n{}",
        screen_row(top),
        screen_row(top + 1).chars().take(4).collect::<String>().trim_end()
    );

    drag(&mut app, area, (0, 0), (1, 4));
    assert_eq!(
        app.tasks.get(id).unwrap().selection().map(|s| s.anchor),
        Some((top, 0)),
        "the first drawn row is screen row `top`"
    );

    // The highlight is painted at the top of the drawn area, not shifted down by `top`.
    let selected = app.theme.get("editor.selection").bg.unwrap();
    let term = draw(&mut app, 100, 30);
    assert_eq!(
        bg(&term, area.x, area.y),
        selected,
        "the first drawn row (screen row `top`) is highlighted at the top of the area"
    );
    drop(term);

    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_click_clears_and_copy_leaves_the_clipboard_alone() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    assert!(
        app.group()
            .active_shell()
            .unwrap()
            .run
            .write(b"printf 'alpha   \\nbeta\\n'\n")
    );
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "beta"))
            .await,
        "the command printed both lines:\n{}",
        screen_text(&app)
    );
    let text = screen_text(&app);
    let (alpha, beta) = (row_of(&text, "alpha"), row_of(&text, "beta"));
    drop(draw(&mut app, 100, 30));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    drag(&mut app, area, (alpha, 0), (beta, 4));
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "alpha\nbeta");

    // A click without a drag clears the selection: nothing is highlighted and Cmd/Ctrl+C finds
    // nothing to copy, so the clipboard stays as it was.
    click(&mut app, area.x + 1, area.y + alpha);
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        None,
        "a click leaves no selection"
    );
    let term = draw(&mut app, 100, 30);
    assert_ne!(
        bg(&term, area.x, area.y + alpha),
        app.theme.get("editor.selection").bg.unwrap(),
        "the highlight is gone"
    );
    drop(term);
    app.clipboard.write(Copied {
        text: "keep".into(),
        whole_line: false,
        pieces: Vec::new(),
    });
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "keep", "no selection: nothing to copy");
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_mouse_reporting_keeps_every_event() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    assert!(
        app.group()
            .active_shell()
            .unwrap()
            .run
            .write(&echo_mouse(Some(COUNT_TO_200), &[1000, 1002, 1006]))
    );
    assert!(
        app.settle(|a| {
            let run = &a.group().active_shell().unwrap().run;
            run.screen().screen().mouse_protocol_mode() == MouseProtocolMode::ButtonMotion
        })
        .await,
        "the program asked for the mouse:\n{}",
        screen_text(&app)
    );
    let text = screen_text(&app);
    let one_ninety_nine = row_of(&text, "199");

    drop(draw(&mut app, 100, 30));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    drag(&mut app, area, (one_ninety_nine, 0), (one_ninety_nine, 3));
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        None,
        "a program that reports the mouse selects nothing"
    );
    assert!(
        app.settle(|a| {
            let text = screen_text(a);
            text.contains("^[[<0;1;") && text.contains("^[[<32;4;")
        })
        .await,
        "the program got the press and the drag:\n{}",
        screen_text(&app)
    );
    let term = draw(&mut app, 100, 30);
    let selected = app.theme.get("editor.selection").bg.unwrap();
    for (row, col) in [(one_ninety_nine, 0), (one_ninety_nine, 2)] {
        assert_ne!(
            bg(&term, area.x + col, area.y + row),
            selected,
            "nothing is highlighted while the program reports the mouse"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_output_and_the_wheel_clear_the_selection() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    assert!(
        app.group()
            .active_shell()
            .unwrap()
            .run
            .write(format!("{COUNT_TO_200}\n").as_bytes())
    );
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "199"))
            .await,
        "the command's output arrived:\n{}",
        screen_text(&app)
    );
    let text = screen_text(&app);
    let one_ninety_nine = row_of(&text, "199");
    drop(draw(&mut app, 100, 30));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    drag(&mut app, area, (one_ninety_nine, 0), (one_ninety_nine, 3));
    assert!(
        app.group().active_shell().unwrap().run.selection().is_some(),
        "the drag selected the row"
    );

    // New output moves the rows: the stored cells would point at other text, so it is dropped.
    assert!(app.group().active_shell().unwrap().run.write(b"printf 'after\\n'\n"));
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "after"))
            .await,
        "the second command's output arrived:\n{}",
        screen_text(&app)
    );
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        None,
        "new output cleared the selection"
    );

    // So does a wheel scroll (through the scrollback, which the wheel still does).
    let text = screen_text(&app);
    let after = row_of(&text, "after");
    let term = draw(&mut app, 100, 30);
    let area = app.hits.group(app.groups.active).unwrap().editor;
    drop(term);
    drag(&mut app, area, (after, 0), (after, 5));
    let selected = app.group().active_shell().unwrap().run.selection();
    assert!(
        selected.is_some(),
        "the second drag selected the row (it stays until an event pumps)"
    );
    app.mouse(MouseEventKind::ScrollUp, area.x, area.y + after);
    assert_eq!(scrollback(&app), 3, "the wheel scrolled the scrollback");
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        None,
        "the wheel cleared the selection"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_copy_elsewhere_is_unaffected_by_a_lingering_selection() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = crate::milestone12_tests::dirs(&dir);
    let mut app = crate::milestone12_tests::open_app(
        &root,
        &state,
        Some("[tasks.select]\ncommand = '''printf 'alpha   \\nbeta\\n'; sleep 60'''\n"),
    );
    // An editor with its own text, so Cmd/Ctrl+C there must not copy the task's selection.
    app.test_open(crate::buffer::Buffer::new_file(&root.join("a.txt")));
    app.type_text("hello");
    drop(draw(&mut app, 100, 30));
    let id = app.run_task("select").unwrap();
    assert!(
        app.settle(|a| task_text(a, id).lines().any(|l| l.trim_end() == "beta"))
            .await,
        "the task printed both lines:\n{}",
        task_text(&app, id)
    );
    let text = task_text(&app, id);
    let (alpha, beta) = (row_of(&text, "alpha"), row_of(&text, "beta"));

    // A drag over the task selects its cells; the selection lingers once the editor takes focus.
    drop(draw(&mut app, 100, 30));
    let area = layout::task_area(app.hits.regions.panel.unwrap());
    drag(&mut app, area, (alpha, 0), (beta, 4));
    assert!(
        app.tasks.get(id).unwrap().selection().is_some(),
        "the drag selected the task"
    );

    // Back in the editor, its own copy wins: the task's selection must not hijack it.
    app.ui.focus = Focus::Editor;
    app.test_execute(Command::SelectAll);
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(
        app.clipboard.read().unwrap(),
        "hello",
        "copy elsewhere is unaffected by a lingering terminal selection"
    );
}
