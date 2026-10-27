//! clear_scrollback-b "Done when": Cmd+K (Ctrl+K on Linux) in a shell tab and in a shown task tab
//! blanks the screen and empties the scrollback (the wheel and Ctrl+Shift+Up have nothing to go
//! back to) without touching the running program (nothing is written to the PTY; later output
//! lands at the top of the cleared screen), drops the mouse selection, and a `.workspace`
//! `scrollback = 50` still caps a run after a Clear. The key resolves to `terminal.clear` /
//! `task.clear` in its contexts and stays unbound in the editor; the cheat sheet and the palette
//! list it. On the alternate screen it blanks that screen and leaves the main one's kept lines.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::App;
use crate::command::{Command, ShellCommand, TaskCommand};
use crate::config::keymap::{Context, Keymap, Platform};
use crate::milestone12_tests::open_app;
use crate::shell_tests::{COUNT_TO_200, dirs, draw, scrollback, shell_app};
use crate::tasks::RunId;
use crate::tasks::select::Selection;

fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

fn shell_text(app: &App) -> String {
    app.group().active_shell().unwrap().run.screen().screen().contents()
}

/// A shell tab (`/bin/sh`) on a short screen after `COUNT_TO_200`, back at its prompt (the line
/// after `199` printed, so no late prompt lands on a cleared screen).
async fn shell_with_output(root: &std::path::Path, state: &std::path::Path) -> App {
    let mut app = shell_app(root, state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    draw(&mut app, 80, 20);
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(format!("{COUNT_TO_200}\n").as_bytes());
    assert!(
        app.settle(|a| shell_text(a)
            .lines()
            .skip_while(|l| l.trim_end() != "199")
            .nth(1)
            .is_some_and(|l| !l.trim().is_empty()))
            .await,
        "the count and the next prompt arrived:\n{}",
        shell_text(&app)
    );
    app
}

fn shell_extent(app: &App) -> (usize, usize, usize) {
    app.group().active_shell().unwrap().run.scrollbar_extent()
}

/// A task that counts to 200 and then `cat`s, so it stays running and echoes what it is sent.
async fn task_with_output(root: &std::path::Path, state: &std::path::Path) -> (App, RunId) {
    let toml = format!("[tasks.count]\ncommand = \"{COUNT_TO_200}; exec cat\"\n");
    let mut app = open_app(root, state, Some(&toml));
    app.tasks.size = (5, 20);
    let id = app.run_task("count").unwrap();
    assert!(
        app.settle(|a| a
            .tasks
            .get(id)
            .is_some_and(|r| r.screen().screen().contents().contains("199")))
            .await
    );
    (app, id)
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_shell_blanks_screen_and_scrollback() {
    let (_d, root, state) = dirs();
    let mut app = shell_with_output(&root, &state).await;
    assert!(shell_extent(&app).1 > 100, "output scrolled off the top");
    // A selection and a view scrolled back, both gone after the Clear.
    app.press(KeyCode::PageUp, KeyModifiers::SHIFT);
    assert!(scrollback(&app) > 0);
    app.group().active_shell().unwrap().run.set_selection(Selection {
        anchor: (0, 0),
        head: (2, 3),
    });

    app.press(KeyCode::Char('k'), cmd());

    let run = &app.group().active_shell().unwrap().run;
    assert_eq!(run.scrollbar_extent().0, 0);
    assert_eq!(run.scrollbar_extent().1, 0, "no kept lines");
    assert!(run.selection().is_none(), "the selection is dropped");
    assert_eq!(run.screen().screen().contents().trim(), "", "a blank screen");
    assert_eq!(run.screen().screen().cursor_position(), (0, 0));
    for (key, mods) in [
        (KeyCode::PageUp, KeyModifiers::SHIFT),
        (KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        (KeyCode::Home, KeyModifiers::CONTROL),
    ] {
        app.press(key, mods);
        assert_eq!(scrollback(&app), 0, "nothing to go back to");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_shell_program_is_untouched() {
    let (_d, root, state) = dirs();
    let mut app = shell_with_output(&root, &state).await;
    // `cat -v` in the foreground shows any byte it is sent: a forwarded ^K (0x0b) would show as
    // `^K` (readline at a prompt would swallow it silently).
    let lines_equal = |a: &App, want: &str| shell_text(a).lines().filter(|l| l.trim_end() == want).count();
    {
        let run = &app.group().active_shell().unwrap().run;
        run.write(b"cat -v\n");
        run.write(b"ready\n");
    }
    assert!(
        app.settle(|a| lines_equal(a, "ready") >= 2).await,
        "cat echoed its input:\n{}",
        shell_text(&app)
    );

    app.press(KeyCode::Char('k'), cmd());
    // Nothing was sent to the program: it still runs and its next output lands at the top.
    app.group().active_shell().unwrap().run.write(b"after\n");
    assert!(
        app.settle(|a| lines_equal(a, "after") >= 2).await,
        "cat got exactly `after`, no ^K before it:\n{}",
        shell_text(&app)
    );
    let run = &app.group().active_shell().unwrap().run;
    assert!(run.status().is_running());
    let text = shell_text(&app);
    assert!(
        text.lines().next().is_some_and(|l| l.trim_end() == "after"),
        "output restarts at the top: {text:?}"
    );
    assert!(
        !text.contains("199") && !text.contains("ready"),
        "the old output is gone: {text:?}"
    );
    assert!(
        !text.contains("^K") && !text.contains('\x0b'),
        "the key was not forwarded: {text:?}"
    );
    assert!(shell_extent(&app).1 < 5, "only the new lines are kept");
    run.write(b"\x04");
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_task_blanks_screen_and_scrollback() {
    let (_d, root, state) = dirs();
    let (mut app, id) = task_with_output(&root, &state).await;
    assert!(app.tasks.get(id).unwrap().scrollbar_extent().1 > 100);
    app.tasks.get(id).unwrap().set_selection(Selection {
        anchor: (0, 0),
        head: (1, 3),
    });
    app.press(KeyCode::PageUp, KeyModifiers::SHIFT);

    app.press(KeyCode::Char('k'), cmd());

    let run = app.tasks.get(id).unwrap();
    assert_eq!(run.scrollbar_extent().0, 0);
    assert_eq!(run.scrollbar_extent().1, 0);
    assert!(run.selection().is_none());
    assert_eq!(run.screen().screen().contents().trim(), "");
    assert_eq!(run.screen().screen().cursor_position(), (0, 0));
    app.press(KeyCode::Home, KeyModifiers::CONTROL);
    assert_eq!(app.tasks.get(id).unwrap().scrollbar_extent().0, 0);

    // The program is untouched: cat still runs and echoes what it is sent, from the top, and the
    // key itself was not written to it.
    assert!(app.tasks.get(id).unwrap().status().is_running());
    app.tasks.get(id).unwrap().write(b"hello\n");
    assert!(
        app.settle(|a| a.tasks.get(id).unwrap().screen().screen().contents().contains("hello"))
            .await
    );
    let text = app.tasks.get(id).unwrap().screen().screen().contents();
    assert!(text.starts_with("hello"), "{text:?}");
    assert!(!text.contains("^K") && !text.contains('\x0b'), "{text:?}");
    app.tasks.get(id).unwrap().kill();
}

/// The setting is the capacity: a `.workspace` `scrollback = 50` run still keeps 50 lines of the
/// output that follows a Clear.
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_setting_still_caps_after_a_clear() {
    let (_d, root, state) = dirs();
    std::fs::write(root.join(".workspace"), "[settings]\nscrollback = 50\n").unwrap();
    let mut app = shell_with_output(&root, &state).await;
    assert_eq!(shell_extent(&app).1, 50);
    app.press(KeyCode::Char('k'), cmd());
    assert_eq!(shell_extent(&app).1, 0);
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(format!("{COUNT_TO_200}; echo done2\n").as_bytes());
    assert!(app.settle(|a| shell_text(a).contains("done2")).await);
    assert_eq!(shell_extent(&app).1, 50, "capped at the setting, not grown or shrunk");
}

#[test]
fn clear_scrollback_key_resolves_per_context() {
    let map = Keymap::default_for(Platform::MacOs);
    let key = crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::SUPER);
    assert_eq!(
        map.lookup(&key, Context::Shell),
        Some(Command::Shell(ShellCommand::Clear))
    );
    assert_eq!(map.lookup(&key, Context::Task), Some(Command::Task(TaskCommand::Clear)));
    assert_eq!(
        map.lookup(&key, Context::Editor),
        None,
        "an editor tab's Cmd+K is unbound"
    );
    let linux = Keymap::default_for(Platform::Linux);
    let ctrl_k = crossterm::event::KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
    assert_eq!(
        linux.lookup(&ctrl_k, Context::Shell),
        Some(Command::Shell(ShellCommand::Clear))
    );
    assert_eq!(
        linux.lookup(&ctrl_k, Context::Task),
        Some(Command::Task(TaskCommand::Clear))
    );
}

/// A full-screen program (the alternate screen): Clear blanks that screen (the program redraws
/// on its own next output) and empties only the alternate screen's kept lines (it has none); the
/// main screen and its scrollback come back unchanged when the program leaves.
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_on_the_alternate_screen_leaves_the_main_one() {
    let (_d, root, state) = dirs();
    let mut app = shell_with_output(&root, &state).await;
    let kept = shell_extent(&app).1;
    assert!(kept > 100);
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(b"printf '\\033[?1049hALT'; read x; printf '\\033[?1049l'\n");
    let alt = |a: &App| {
        a.group()
            .active_shell()
            .unwrap()
            .run
            .screen()
            .screen()
            .alternate_screen()
    };
    assert!(app.settle(|a| alt(a) && shell_text(a).contains("ALT")).await);

    app.press(KeyCode::Char('k'), cmd());
    assert!(alt(&app), "the program is still on its screen");
    assert_eq!(shell_text(&app).trim(), "", "its screen is blank");
    assert_eq!(shell_extent(&app).0, 0);

    app.group().active_shell().unwrap().run.write(b"\n");
    assert!(app.settle(|a| !alt(a)).await);
    assert!(
        shell_extent(&app).1 >= kept,
        "the main screen's kept lines are untouched"
    );
}

/// The cheat sheet's first section in a shell tab ("Terminal") and a task tab lists the key, and
/// the palette lists both commands under their names.
#[test]
fn clear_scrollback_cheat_sheet_and_palette_list_it() {
    for (platform, key) in [(Platform::MacOs, "Cmd+K"), (Platform::Linux, "Ctrl+K")] {
        let km = Keymap::default_for(platform);
        for (context, title) in [(Context::Shell, "Terminal: Clear"), (Context::Task, "Tasks: Clear")] {
            let s = crate::cheatsheet::sections(&km, context);
            assert!(
                s[0].rows.iter().any(|r| r.key == key && r.title == title),
                "{platform:?} {context:?}: {:?}",
                s[0]
            );
        }
    }
    let all = Command::all();
    for c in [Command::Shell(ShellCommand::Clear), Command::Task(TaskCommand::Clear)] {
        assert!(all.contains(&c) && c.in_palette(), "{c:?}");
    }
    assert_eq!(
        Command::from_name("terminal.clear"),
        Some(Command::Shell(ShellCommand::Clear))
    );
    assert_eq!(
        Command::from_name("task.clear"),
        Some(Command::Task(TaskCommand::Clear))
    );
}
