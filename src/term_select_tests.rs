//! Mouse selection in a terminal (`term_select`): over a shell and a task tab whose program
//! hasn't asked for the mouse, a left drag highlights the cells it covers and Cmd/Ctrl+C copies
//! exactly that text (trailing spaces trimmed, soft-wrapped rows joined), also when the view is
//! scrolled back (`term_select_scrollback`: the selection moves with its text through any scroll
//! and copies in full, in view or not); a click clears it; new output clears it (the rows move);
//! a program that asked for the mouse still gets every event and nothing is selected, scrolled
//! back too. The copy's normalisation and windowing are `tasks::select`'s unit tests.

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

/// Waits for the active shell's first prompt (its only output before a command runs), so a command
/// written next echoes after it and its own output starts at column 0 rather than interleaving with
/// a prompt that arrives once the shell finishes starting.
async fn shell_prompt(app: &mut App) {
    assert!(
        app.settle(|a| !screen_text(a).trim().is_empty()).await,
        "the shell printed its prompt:\n{}",
        screen_text(app)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_shell_drag_highlights_and_copy() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    shell_prompt(&mut app).await;
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
            anchor: (alpha as isize, 0),
            head: (beta as isize, 4),
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
            anchor: (alpha as isize, 0),
            head: (beta as isize, 4),
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
            "[tasks.ended]\ncommand = '''printf 'one\\ntwo\\nthree\\nfour\\nfive\\nsix\\nseven\\neight\\nnine\\nten\\n''''\n",
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
            screen.screen().size().1.get(),
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
        Some((top as isize, 0)),
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
    app.new_shell_in_editor();
    shell_prompt(&mut app).await;
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
    app.new_shell_in_editor();
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
    drop(term);

    // A double-click on a path character is the program's too: both presses reach it and
    // nothing is selected.
    let (x, y) = (area.x + 1, area.y + one_ninety_nine);
    click(&mut app, x, y);
    click(&mut app, x, y);
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        None,
        "a double-click over a program that reports the mouse selects nothing"
    );
    let press = format!("^[[<0;2;{}M", one_ninety_nine + 1);
    assert!(
        app.settle(|a| screen_text(a).matches(&press).count() >= 2).await,
        "the program got both presses of the double-click ({press}):\n{}",
        screen_text(&app)
    );

    // Scrolled back (the scroll keys still scroll; the wheel is the program's), a drag still
    // selects nothing and leaves the view where it is.
    app.press(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    app.press(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(scrollback(&app), 2, "Ctrl+Shift+Up scrolled back");
    drag(&mut app, area, (2, 0), (3, 3));
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        None,
        "scrolled back, a program that reports the mouse still selects nothing"
    );
    assert_eq!(scrollback(&app), 2, "the drag left the view scrolled back");
    click(&mut app, area.x + 1, area.y + 2);
    click(&mut app, area.x + 1, area.y + 2);
    assert_eq!(
        app.group().active_shell().unwrap().run.selection(),
        None,
        "scrolled back, a double-click over a program that reports the mouse selects nothing"
    );
}

// FLAKE: 2026-10-26, in a full parallel run the selection was still `Some(..)` after the second
// command's output arrived (the `Output` event that clears it hadn't been processed by the frame
// the settle saw); passes alone and in the term_select module. Dump the selection and the screen
// tail so a recurrence shows whether the output event raced the settle.
#[tokio::test(flavor = "multi_thread")]
async fn term_select_output_clears_and_the_wheel_moves_the_selection() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
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
    let selection = app.group().active_shell().unwrap().run.selection();
    if selection.is_some() {
        let tail = screen_text(&app).lines().rev().take(8).collect::<Vec<_>>().join("\n");
        eprintln!(
            "FLAKE term_select_output_clears_and_the_wheel_moves_the_selection: not cleared \
             ({selection:?}); screen tail:\n{tail}"
        );
    }
    assert_eq!(selection, None, "new output cleared the selection");

    // A wheel scroll does not: the selection moves with its text.
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
        selected,
        "the wheel kept the selection on its text"
    );
}

fn run_of(app: &App, id: Option<RunId>) -> &crate::tasks::Run {
    match id {
        Some(id) => app.tasks.get(id).unwrap(),
        None => &app.group().active_shell().unwrap().run,
    }
}

fn visible_lines(app: &App, id: Option<RunId>) -> Vec<String> {
    let contents = run_of(app, id).screen().screen().contents();
    contents.lines().map(String::from).collect()
}

/// Scrolled back over `COUNT_TO_200`-style numbered output: a drag over two older rows
/// highlights and copies them, the highlight follows the text through the wheel, and the copy
/// stays whole once it is out of view and back on the live screen.
fn scrolled_back_scenario(app: &mut App, id: Option<RunId>, area: Rect) {
    let (x, y) = (area.x + 1, area.y + 1);
    for _ in 0..4 {
        app.mouse(MouseEventKind::ScrollUp, x, y);
    }
    drop(draw(app, 100, 30));
    let back = run_of(app, id).screen().screen().scrollback();
    assert_eq!(back, 12, "the wheel scrolled back");
    let lines = visible_lines(app, id);
    let (l2, l3) = (lines[2].trim_end().to_string(), lines[3].trim_end().to_string());
    assert!(
        l2.parse::<u32>().is_ok() && l3.parse::<u32>().is_ok(),
        "numbered rows: {lines:?}"
    );
    let expected = format!("{l2}\n{}", &l3[..1]);

    drag(app, area, (2, 0), (3, 1));
    let selection = run_of(app, id).selection().unwrap();
    assert_eq!(
        selection.anchor,
        (2 - back as isize, 0),
        "the cells are scroll-independent"
    );
    let selected = app.theme.get("editor.selection").bg.unwrap();
    let term = draw(app, 100, 30);
    assert_eq!(bg(&term, area.x, area.y + 2), selected, "the older row is highlighted");
    assert_eq!(bg(&term, area.x, area.y + 3), selected);
    assert_ne!(bg(&term, area.x, area.y + 4), selected);
    drop(term);
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(
        app.clipboard.read().unwrap(),
        expected,
        "copies exactly the older lines"
    );

    // One wheel notch (3 rows) up: the highlight is 3 rows lower, the selection unchanged.
    app.mouse(MouseEventKind::ScrollUp, x, y);
    assert_eq!(
        run_of(app, id).selection(),
        Some(selection),
        "the wheel kept the selection"
    );
    let term = draw(app, 100, 30);
    assert_eq!(
        bg(&term, area.x, area.y + 5),
        selected,
        "the highlight moved with its text"
    );
    assert_ne!(bg(&term, area.x, area.y + 2), selected);
    drop(term);

    // Back to the live screen: far out of view, the copy is the same whole text.
    for _ in 0..8 {
        app.mouse(MouseEventKind::ScrollDown, x, y);
    }
    assert_eq!(run_of(app, id).screen().screen().scrollback(), 0);
    assert_eq!(run_of(app, id).selection(), Some(selection));
    drop(draw(app, 100, 30));
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(
        app.clipboard.read().unwrap(),
        expected,
        "copies the selection scrolled out of view"
    );
    assert_eq!(
        run_of(app, id).screen().screen().scrollback(),
        0,
        "the copy restores the scroll"
    );

    // A click scrolled back clears it and copies nothing.
    app.mouse(MouseEventKind::ScrollUp, x, y);
    click(app, x, y);
    assert_eq!(run_of(app, id).selection(), None);
    app.clipboard.write(Copied {
        text: "keep".into(),
        whole_line: false,
        pieces: Vec::new(),
    });
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "keep", "no selection: nothing to copy");

    // The wheel mid-drag: the anchor stays on its text, the head is where the pointer is in
    // the view scrolled further back, and the copy runs between them.
    let before = run_of(app, id).screen().screen().scrollback();
    let at = |(row, col): (u16, u16)| (area.x + col, area.y + row);
    let (px, py) = at((2, 0));
    app.mouse(MouseEventKind::Down(MouseButton::Left), px, py);
    app.mouse(MouseEventKind::ScrollUp, px, py);
    let now = run_of(app, id).screen().screen().scrollback();
    assert_eq!(now, before + 3, "the wheel scrolled mid-drag");
    let (hx, hy) = at((3, 1));
    app.mouse(MouseEventKind::Drag(MouseButton::Left), hx, hy);
    app.mouse(MouseEventKind::Up(MouseButton::Left), hx, hy);
    let selection = run_of(app, id).selection().unwrap();
    assert_eq!(
        selection.anchor,
        (2 - before as isize, 0),
        "the anchor stayed on its text"
    );
    assert_eq!(selection.head, (3 - now as isize, 1));
    // In the current view the anchor's line is row 5: rows 3 (from column 1) to 5 (none of it).
    let lines = visible_lines(app, id);
    let expected = format!("{}\n{}\n", &lines[3].trim_end()[1..], lines[4].trim_end());
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(
        app.clipboard.read().unwrap(),
        expected,
        "copies across the mid-drag scroll"
    );

    // The scroll keys (Ctrl+Shift+Up / Down, the tab having the keys after the drag) scroll the
    // scrollback too, and leave the selection on its text.
    app.press(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(
        run_of(app, id).screen().screen().scrollback(),
        now + 1,
        "Ctrl+Shift+Up scrolled the scrollback"
    );
    app.press(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    app.press(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(
        run_of(app, id).screen().screen().scrollback(),
        now - 1,
        "Ctrl+Shift+Down"
    );
    assert_eq!(run_of(app, id).selection(), Some(selection));
    let term = draw(app, 100, 30);
    let selected = app.theme.get("editor.selection").bg.unwrap();
    // One row further down than before the keys: rows 2 (from column 1) to 4 (none of it).
    assert_ne!(bg(&term, area.x, area.y + 2), selected, "column 0 of the head's row");
    assert_eq!(
        bg(&term, area.x + 1, area.y + 2),
        selected,
        "the head's row from column 1"
    );
    assert_eq!(bg(&term, area.x, area.y + 3), selected);
    assert_ne!(
        bg(&term, area.x, area.y + 4),
        selected,
        "the anchor's column is excluded"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_scrollback_shell() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    drop(draw(&mut app, 100, 30));
    shell_prompt(&mut app).await;
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
        "output arrived:\n{}",
        screen_text(&app)
    );
    drop(draw(&mut app, 100, 30));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    scrolled_back_scenario(&mut app, None, area);
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_scrollback_task() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = crate::milestone12_tests::dirs(&dir);
    let mut app = crate::milestone12_tests::open_app(
        &root,
        &state,
        Some(&format!("[tasks.count]\ncommand = '''{COUNT_TO_200}; sleep 60'''\n")),
    );
    drop(draw(&mut app, 100, 30));
    let id = app.run_task("count").unwrap();
    assert!(
        app.settle(|a| task_text(a, id).lines().any(|l| l.trim_end() == "199"))
            .await,
        "output arrived:\n{}",
        task_text(&app, id)
    );
    drop(draw(&mut app, 100, 30));
    let area = layout::task_area(app.hits.regions.panel.unwrap());
    scrolled_back_scenario(&mut app, Some(id), area);
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

/// Double-clicks `(x, y)` (two quick clicks on one cell).
fn double_click(app: &mut App, x: u16, y: u16) {
    click(app, x, y);
    click(app, x, y);
}

type Sel = Option<crate::tasks::select::Selection>;

/// Checks double-clicks over `row`, which reads `~/src/jed-0.1 ok`: the run `~/src/jed-0.1`
/// (cols 0..13) is selected, highlighted and copied; a blank selects nothing; a third click and a
/// single click clear; a drag from a double-click's press extends from the pressed cell; a drag
/// still selects. No sleeps: a click on another cell restarts the count (`ClickTracker`), so
/// each double-click starts on a cell other than the last click's.
fn double_click_checks(app: &mut App, area: Rect, row: u16, selection: impl Fn(&App) -> Sel) {
    let at = |col: u16| (area.x + col, area.y + row);
    let path = Some(crate::tasks::select::Selection {
        anchor: (row as isize, 0),
        head: (row as isize, 13),
    });
    let (x, y) = at(5);
    double_click(app, x, y);
    assert_eq!(selection(app), path, "a double-click selects the path run");
    let selected = app.theme.get("editor.selection").bg.unwrap();
    let term = draw(app, 100, 30);
    for col in [0, 12] {
        assert_eq!(bg(&term, at(col).0, at(col).1), selected, "col {col} highlighted");
    }
    assert_ne!(bg(&term, at(13).0, at(13).1), selected, "nor the blank");
    drop(term);
    app.press(KeyCode::Char('c'), cmd());
    assert_eq!(app.clipboard.read().unwrap(), "~/src/jed-0.1");

    // A third click on the cell is a plain click: it clears.
    click(app, x, y);
    assert_eq!(selection(app), None, "a triple click clears");

    // A double-click on a non-path character selects nothing.
    let (x, y) = at(13);
    double_click(app, x, y);
    assert_eq!(selection(app), None, "a blank selects nothing");

    // A single click on a path character clears the double-click's selection.
    let (x, y) = at(5);
    double_click(app, x, y);
    assert_eq!(selection(app), path);
    let (x, y) = at(6);
    click(app, x, y);
    assert_eq!(selection(app), None, "a single click clears");

    // The second press of a double-click seeds the run; a drag from it extends from the
    // pressed cell, not from the run's ends.
    let (x, y) = at(9);
    click(app, x, y);
    app.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    assert_eq!(selection(app), path, "the second press selects the run");
    let (x, y) = at(11);
    app.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
    app.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(
        selection(app).map(|s| (s.anchor, s.head)),
        Some(((row as isize, 9), (row as isize, 11))),
        "a drag from a double-click extends from the clicked cell"
    );

    drag(app, area, (row, 1), (row, 5));
    assert_eq!(
        selection(app).map(|s| (s.anchor.1, s.head.1)),
        Some((1, 5)),
        "a drag still selects"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_double_click_shell() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    shell_prompt(&mut app).await;
    assert!(
        app.group()
            .active_shell()
            .unwrap()
            .run
            .write(b"printf '~/src/jed-0.1 ok\\n'\n")
    );
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "~/src/jed-0.1 ok"))
            .await,
        "{}",
        screen_text(&app)
    );
    let row = row_of(&screen_text(&app), "~/src/jed-0.1 ok");
    drop(draw(&mut app, 100, 30));
    let area = app.hits.group(app.groups.active).unwrap().editor;
    double_click_checks(&mut app, area, row, |a| {
        a.group().active_shell().unwrap().run.selection()
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn term_select_double_click_task() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = crate::milestone12_tests::dirs(&dir);
    let mut app = crate::milestone12_tests::open_app(
        &root,
        &state,
        Some("[tasks.select]\ncommand = '''printf '~/src/jed-0.1 ok\\n'; sleep 60'''\n"),
    );
    drop(draw(&mut app, 100, 30));
    let id = app.run_task("select").unwrap();
    assert!(
        app.settle(|a| task_text(a, id).lines().any(|l| l.trim_end() == "~/src/jed-0.1 ok"))
            .await,
        "{}",
        task_text(&app, id)
    );
    let row = row_of(&task_text(&app, id), "~/src/jed-0.1 ok");
    drop(draw(&mut app, 100, 30));
    let area = layout::task_area(app.hits.regions.panel.unwrap());
    app.ui.focus = Focus::Editor;
    double_click_checks(&mut app, area, row, |a| a.tasks.get(id).unwrap().selection());
}
