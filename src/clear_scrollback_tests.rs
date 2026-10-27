//! clear_scrollback-b "Done when": Cmd+K (Ctrl+K on Linux) in a shell tab and in a shown task tab
//! blanks the screen and empties the scrollback (the wheel and Ctrl+Shift+Up have nothing to go
//! back to) without touching the running program (nothing is written to the PTY; later output
//! lands at the top of the cleared screen), drops the mouse selection, and a `.workspace`
//! `scrollback = 50` still caps a run after a Clear. The key resolves to `terminal.clear` /
//! `task.clear` in its contexts and stays unbound in the editor; the cheat sheet and the palette
//! list it. On the alternate screen it blanks that screen and leaves the main one's kept lines.
//!
//! clear_scrollback-c "Done when": with the shell integration (real bash, real zsh) a Clear goes
//! back to the last prompt mark (`133;A`), so a multi-line prompt survives whole; the user's own
//! `PROMPT_COMMAND` (a string, or an array in bash 5.1+) still runs with the real `$?`, and one
//! that rebuilds `PS1` keeps the mark; while a command runs, and with no mark, it is the full clear.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::App;
use crate::command::{Command, ShellCommand, TaskCommand};
use crate::config::keymap::{Context, Keymap, Platform};
use crate::milestone12_tests::open_app;
use crate::shell_tests::{
    COUNT_TO_200, bash_or_skip, dirs, draw, listening_app, run_in_shell, scrollback, shell_app, shell_script,
};
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
    draw(&mut app, 80, 20);
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    app.mouse(crossterm::event::MouseEventKind::ScrollUp, editor.x + 1, editor.y + 1);
    assert_eq!(scrollback(&app), 0, "the wheel has nothing to go back to");
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

/// `zsh`'s path, or a printed skip note and `None`.
fn zsh_or_skip(test: &str) -> Option<std::path::PathBuf> {
    let zsh = [
        "/bin/zsh",
        "/usr/bin/zsh",
        "/usr/local/bin/zsh",
        "/opt/homebrew/bin/zsh",
    ]
    .into_iter()
    .map(std::path::PathBuf::from)
    .find(|p| p.is_file());
    if zsh.is_none() {
        eprintln!("skipped {test}: needs zsh");
    }
    zsh
}

/// The shell tab's screen rows, trailing spaces dropped.
fn rows(app: &App) -> Vec<String> {
    shell_text(app).lines().map(|l| l.trim_end().to_string()).collect()
}

/// The last two lines of the screen are the two-line prompt, i.e. the shell is idle at it.
fn at_prompt(app: &App) -> bool {
    let r = rows(app);
    r.len() >= 2 && r[r.len() - 1] == "P2>" && r[r.len() - 2] == "P1"
}

/// A real `shell` in a shell tab with the jed snippet for `kind` sourced, a two-line prompt
/// (`P1` / `P2> `), and `before` run first (e.g. a `PROMPT_COMMAND`). Waits for the prompt.
async fn integrated_shell(
    root: &std::path::Path,
    state: &std::path::Path,
    sockets: &tempfile::TempDir,
    shell: &std::path::Path,
    args: &str,
    kind: &str,
    before: &str,
) -> App {
    let (mut app, _socket) = listening_app(root, state, sockets).await;
    app.tasks.shell = shell_script(root, "myshell", &format!("exec {} {args}\n", shell.display()));
    app.test_execute(Command::NewShell);
    draw(&mut app, 80, 12);
    crate::shell_integration::write_snippets(state).unwrap();
    let snippet = state.join(format!("shell-integration.{kind}"));
    let ps = if kind == "zsh" { "PROMPT" } else { "PS1" };
    run_in_shell(&mut app, &format!("{ps}=$'P1\\nP2> '; {before}"));
    run_in_shell(&mut app, &format!("source '{}'", snippet.display()));
    assert!(
        app.settle(at_prompt).await,
        "the two-line prompt:\n{}",
        shell_text(&app)
    );
    app
}

/// Types `line` and waits for the prompt after its output, whose last line is `last_output`.
async fn run_and_wait(app: &mut App, line: &str, last_output: &str) {
    run_in_shell(app, line);
    assert!(
        app.settle(|a| {
            let r = rows(a);
            at_prompt(a) && r.len() >= 3 && r[r.len() - 3] == last_output
        })
        .await,
        "`{line}` finished:\n{}",
        shell_text(app)
    );
}

fn assert_whole_prompt_kept(app: &App) {
    let r = rows(app);
    assert_eq!(
        r,
        ["P1", "P2>"],
        "the whole prompt is all that is left:\n{}",
        shell_text(app)
    );
    assert_eq!(shell_extent(app).1, 0, "nothing to scroll back to");
}

/// Bash with a two-line prompt and a lot of output: Cmd+K keeps both prompt lines at the top and
/// empties the scrollback; the shell works on (a command's output follows the prompt).
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_bash_keeps_the_whole_prompt() {
    let Some(bash) = bash_or_skip("clear_scrollback_bash_keeps_the_whole_prompt") else {
        return;
    };
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let mut app = integrated_shell(&root, &state, &sockets, &bash, "--norc --noprofile", "bash", "").await;
    run_and_wait(&mut app, COUNT_TO_200, "199").await;
    assert!(shell_extent(&app).1 > 0, "output went into the scrollback");

    app.press(KeyCode::Char('k'), cmd());
    assert_whole_prompt_kept(&app);

    run_in_shell(&mut app, "echo after");
    assert!(
        app.settle(|a| rows(a).iter().any(|l| l == "after") && at_prompt(a))
            .await,
        "the shell works on:\n{}",
        shell_text(&app)
    );
    let r = rows(&app);
    assert_eq!(&r[..3], ["P1", "P2> echo after", "after"], "{r:?}");
}

/// The user's own `PROMPT_COMMAND` (a string) still runs after the snippet's, with the real `$?`.
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_bash_keeps_the_users_prompt_command() {
    let Some(bash) = bash_or_skip("clear_scrollback_bash_keeps_the_users_prompt_command") else {
        return;
    };
    let (_d, root, state) = dirs();
    let log = root.join("pc.log");
    let sockets = tempfile::tempdir().unwrap();
    let before = format!("PROMPT_COMMAND='echo \"st=$?\" >> {}'", log.display());
    let mut app = integrated_shell(&root, &state, &sockets, &bash, "--norc --noprofile", "bash", &before).await;
    run_in_shell(&mut app, "false");
    assert!(
        app.settle(|_| std::fs::read_to_string(&log).is_ok_and(|s| s.lines().last() == Some("st=1")))
            .await,
        "the user's hook saw the failed command's status: {:?}",
        std::fs::read_to_string(&log)
    );
    assert!(app.settle(at_prompt).await);
    app.press(KeyCode::Char('k'), cmd());
    assert_whole_prompt_kept(&app);
}

/// Zsh with a two-line prompt (when zsh is installed).
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_zsh_keeps_the_whole_prompt() {
    let Some(zsh) = zsh_or_skip("clear_scrollback_zsh_keeps_the_whole_prompt") else {
        return;
    };
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let mut app = integrated_shell(&root, &state, &sockets, &zsh, "-f", "zsh", "").await;
    run_and_wait(&mut app, "seq 1 200", "200").await;
    assert!(shell_extent(&app).1 > 0);
    app.press(KeyCode::Char('k'), cmd());
    assert_whole_prompt_kept(&app);
}

/// Cmd+K while a command runs (`tail -f`, a build) is the full clear: going back to its prompt
/// would keep all of its output. The command runs on and its next output lands at the top.
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_running_command_is_the_full_clear() {
    let Some(bash) = bash_or_skip("clear_scrollback_running_command_is_the_full_clear") else {
        return;
    };
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let mut app = integrated_shell(&root, &state, &sockets, &bash, "--norc --noprofile", "bash", "").await;
    run_in_shell(&mut app, "seq 1 200; cat");
    assert!(
        app.settle(|a| rows(a).last().is_some_and(|l| l == "200")).await,
        "the output ran past the prompt:\n{}",
        shell_text(&app)
    );
    app.press(KeyCode::Char('k'), cmd());
    assert_eq!(shell_extent(&app).1, 0, "nothing to scroll back to");
    assert_eq!(shell_text(&app).trim(), "", "a blank screen");
    let run = &app.group().active_shell().unwrap().run;
    assert!(run.status().is_running());
    run.write(b"more\n");
    assert!(
        app.settle(|a| rows(a).iter().filter(|l| *l == "more").count() == 2)
            .await,
        "cat runs on (the echo and its copy):\n{}",
        shell_text(&app)
    );
    assert_eq!(rows(&app)[..2], ["more", "more"]);
    app.group().active_shell().unwrap().run.write(b"\x04");
    // Back at the prompt, a Clear keeps it again.
    assert!(app.settle(at_prompt).await, "{}", shell_text(&app));
    app.press(KeyCode::Char('k'), cmd());
    assert_whole_prompt_kept(&app);
}

/// A `PROMPT_COMMAND` of the user's that rebuilds `PS1` at every prompt (`__git_ps1`, themes)
/// and ends in `;`: the mark is added after it, so the whole prompt still survives a Clear.
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_bash_prompt_command_rebuilding_ps1() {
    let Some(bash) = bash_or_skip("clear_scrollback_bash_prompt_command_rebuilding_ps1") else {
        return;
    };
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let before = "myps() { PS1=$'P1\\nP2> '; }; PROMPT_COMMAND='myps;'";
    let mut app = integrated_shell(&root, &state, &sockets, &bash, "--norc --noprofile", "bash", before).await;
    run_and_wait(&mut app, COUNT_TO_200, "199").await;
    app.press(KeyCode::Char('k'), cmd());
    assert_whole_prompt_kept(&app);
}

/// Bash 5.1+'s array `PROMPT_COMMAND`: the user's element still runs with the real `$?` and the
/// prompt survives a Clear (skipped on older bash, e.g. macOS's 3.2).
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_bash_array_prompt_command() {
    let Some(bash) = bash_or_skip("clear_scrollback_bash_array_prompt_command") else {
        return;
    };
    let version = std::process::Command::new(&bash)
        .args(["-c", "echo $((BASH_VERSINFO[0] * 100 + BASH_VERSINFO[1]))"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
        .unwrap_or(0);
    if version < 501 {
        eprintln!("skipped clear_scrollback_bash_array_prompt_command: needs bash 5.1+");
        return;
    }
    let (_d, root, state) = dirs();
    let log = root.join("pc.log");
    let sockets = tempfile::tempdir().unwrap();
    let before = format!("PROMPT_COMMAND=('echo \"st=$?\" >> {}')", log.display());
    let mut app = integrated_shell(&root, &state, &sockets, &bash, "--norc --noprofile", "bash", &before).await;
    run_in_shell(&mut app, "false");
    assert!(
        app.settle(|_| std::fs::read_to_string(&log).is_ok_and(|s| s.lines().last() == Some("st=1")))
            .await,
        "the user's hook saw the failed command's status: {:?}",
        std::fs::read_to_string(&log)
    );
    assert!(app.settle(at_prompt).await);
    app.press(KeyCode::Char('k'), cmd());
    assert_whole_prompt_kept(&app);
}

/// With no integration there is no mark: the full clear.
#[tokio::test(flavor = "multi_thread")]
async fn clear_scrollback_without_a_mark_is_the_full_clear() {
    let (_d, root, state) = dirs();
    let mut app = shell_with_output(&root, &state).await;
    app.press(KeyCode::Char('k'), cmd());
    assert_eq!(shell_extent(&app).1, 0);
    assert!(
        shell_text(&app).lines().filter(|l| !l.trim().is_empty()).count() <= 1,
        "blank but for a late prompt:\n{}",
        shell_text(&app)
    );
}
