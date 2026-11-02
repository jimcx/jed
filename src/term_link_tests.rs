//! Cmd/Ctrl+click on a `path[:line[:col]]` in a shell tab's output (`app/term_link.rs`): a path
//! that names one file opens it in the editor area at the line; anything else goes into quick
//! open's input; a plain click and a word that is no path do nothing.

use crossterm::event::KeyModifiers;
use crossterm::event::{MouseButton, MouseEventKind};

use crate::app::App;
use crate::buffer::pos;
use crate::command::Command;
use crate::shell_tests::{dirs, draw, screen_text, shell_app};

fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

/// Prints `line` in a fresh shell and returns the screen cell `(x, y)` of the first character
/// of `needle` in it.
async fn show(app: &mut App, line: &str, needle: &str) -> (u16, u16) {
    app.tasks.shell = "/bin/sh".into();
    app.new_shell_in_editor();
    let run = &app.group().active_shell().unwrap().run;
    assert!(run.write(format!("printf '{line}\\n'\n").as_bytes()));
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end().ends_with(line)))
            .await,
        "{}",
        screen_text(app)
    );
    drop(draw(app, 100, 30));
    // The output shares the prompt's line ("$ line"), so find the line's column on screen.
    let text = screen_text(app);
    let row = text.lines().position(|l| l.trim_end().ends_with(line)).unwrap();
    let col = text.lines().nth(row).unwrap().find(line).unwrap() + line.find(needle).unwrap();
    let area = app.hits.group(app.groups.active).unwrap().editor;
    (area.x + col as u16, area.y + row as u16)
}

fn link_click(app: &mut App, (x, y): (u16, u16)) {
    app.mouse_with(MouseEventKind::Down(MouseButton::Left), x, y, cmd());
    app.mouse_with(MouseEventKind::Up(MouseButton::Left), x, y, cmd());
}

#[tokio::test(flavor = "multi_thread")]
async fn term_link_opens_the_file_at_the_line() {
    let (_d, root, state) = dirs();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "a\nb\nc\nd\n").unwrap();
    let mut app = shell_app(&root, &state);
    let at = show(&mut app, "error at src/lib.rs:3:1 here", "lib").await;
    // A plain click opens nothing.
    app.mouse(MouseEventKind::Down(MouseButton::Left), at.0, at.1);
    app.mouse(MouseEventKind::Up(MouseButton::Left), at.0, at.1);
    assert!(app.active_buffer().is_none());
    link_click(&mut app, at);
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.path.ends_with("src/lib.rs")))
            .await
    );
    let tab = app.group().active_tab().unwrap();
    let buf = app.active_buffer().unwrap();
    assert_eq!(pos::line_of(&buf.rope, tab.view.selections.primary().head), 2);
    assert!(app.palette.is_none());
    // The shell stays on screen in its own group, so a second link click still reaches it.
    assert_eq!(app.groups.ids().len(), 2);
    assert!(
        app.groups
            .ids()
            .iter()
            .any(|g| app.groups.get(*g).unwrap().active_shell().is_some())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn term_link_unresolved_goes_to_quick_open() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    let at = show(&mut app, "see nope/x.rs:4 now", "x.rs").await;
    link_click(&mut app, at);
    assert!(app.settle(|a| a.palette.is_some()).await);
    assert_eq!(app.palette.as_ref().unwrap().query(), "nope/x.rs:4:1");
    assert!(app.active_buffer().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn term_link_ignores_plain_words() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    let at = show(&mut app, "hello world", "world").await;
    link_click(&mut app, at);
    assert!(!app.settle(|a| a.palette.is_some() || a.active_buffer().is_some()).await);
}

/// The shell is in the terminal area and the file is open in two editor groups, active in the
/// second while the first is the active group: the click goes to the group showing it.
#[tokio::test(flavor = "multi_thread")]
async fn term_link_from_the_terminal_area_goes_to_the_group_showing_the_file() {
    let (_d, root, state) = dirs();
    std::fs::write(root.join("lib.rs"), "a\nb\nc\nd\n").unwrap();
    std::fs::write(root.join("other.rs"), "x\ny\n").unwrap();
    let mut app = shell_app(&root, &state);
    app.test_open_file(root.join("lib.rs"));
    assert!(app.settle(|a| a.active_buffer().is_some()).await);
    app.test_execute(Command::SplitRight);
    let (g2, g1) = (app.groups.active, app.groups.ids()[0]);
    assert_ne!(g1, g2);
    app.test_execute(Command::FocusGroup(1));
    app.test_open_file(root.join("other.rs"));
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| b.path.ends_with("other.rs")))
            .await
    );
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    let run = &app.terminals.active_group().active_shell().unwrap().run;
    assert!(run.write(b"printf 'at lib.rs:3\\n'\n"));
    assert!(
        app.settle(|a| {
            let run = &a.terminals.active_group().active_shell().unwrap().run;
            let text = run.screen().screen().contents();
            text.lines().any(|l| l.trim_end().ends_with("at lib.rs:3"))
        })
        .await
    );
    drop(draw(&mut app, 100, 30));
    let run = &app.terminals.active_group().active_shell().unwrap().run;
    let text = run.screen().screen().contents();
    let row = text
        .lines()
        .position(|l| l.trim_end().ends_with("at lib.rs:3"))
        .unwrap();
    let col = text.lines().nth(row).unwrap().find("lib").unwrap();
    let area = app.hits.group(app.terminals.active).unwrap().editor;
    link_click(&mut app, (area.x + col as u16, area.y + row as u16));
    assert!(
        app.settle(|a| a.groups.active == g2).await,
        "focus went to the group showing lib.rs"
    );
    assert!(app.active_buffer().unwrap().path.ends_with("lib.rs"));
    let tab = app.group().active_tab().unwrap();
    assert_eq!(
        pos::line_of(&app.active_buffer().unwrap().rope, tab.view.selections.primary().head),
        2
    );
    assert_eq!(
        app.terminals.active_group().active,
        0,
        "the terminal area's tabs are untouched"
    );
}
