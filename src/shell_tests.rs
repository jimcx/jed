//! End-to-end tests for the shell task: shell-a's "Terminal: New Shell" opens the user's shell
//! in a tab of the active group (`app.group().active_shell()`), as a terminal emulator starts it
//! — a PTY in the workspace root at the group's editor size, with `TERM` / `COLORTERM` and
//! `JED_IPC` only when the socket listens — titled per program (`sh`, `sh (2)`, reusing a closed
//! number) and following the shell's OSC codes while a command runs, keeping the `scrollback`
//! setting's lines, never in a session, and with no tab of its own in a split. shell-b: the screen
//! drawn in the editor area at its size, in the terminal's
//! colours with its cursor only when focused and live, keys and pastes through `tasks::keys`
//! (types, Enter, arrows in the shell's cursor mode, Esc, Ctrl+C, bracketed paste; Cmd+W and
//! global keys stay jed's), Shift+PageUp / PageDown / Ctrl+Home / End and the wheel through the
//! scrollback, exit notes kept on screen and in the status bar (success, exit code, failed
//! start), a closed tab killing the shell, and quitting hanging every shell up (SIGKILL when
//! SIGHUP is ignored). shell-c: the real socket end to end — a program started from the shell
//! records `JED_IPC` (the socket path), and the real `jed` binary run from the shell opens its
//! file here (`jed <file>`, and `git commit` through `core.editor = "jed --wait"`; both skip
//! with a printed note when the binary or git is missing) — `stty size` follows the editor
//! area's resize and a split, and a `.workspace` `scrollback = 50` is what a new shell and a
//! new task run keep. That the user's own `.zshrc` / `.bashrc` runs is a hand check: it needs
//! the user's real init and shell, which a test must not depend on. shell_integration-b: the
//! wizard (`Command::InstallShellIntegration`) writes the snippets into a test state dir and
//! shows the detected shell's init file and source line (C copies it, A appends it to a temp
//! file), and the written bash snippet is sourced in a real `bash --norc --noprofile` over a PTY
//! — a command's title, a program's own OSC 2 title until `133;D`, the red idle glyph after a
//! failed command, and the `JED_IPC` guard's no-op when there's no socket.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event as CEvent, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus, Level, Prompt};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::syntax::Language;
use crate::tasks::{SHUTDOWN_TIMEOUT, Source, Status, Task, Template};
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// A command printing 0 to 199, one per line, in the shell (`seq` is missing on macOS).
pub(crate) const COUNT_TO_200: &str = "i=0; while [ $i -lt 200 ]; do echo $i; i=$((i+1)); done";

/// The platform's Cmd (Super on macOS, Ctrl on Linux).
fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

/// A workspace app on `root`, its trust store in `state`.
pub(crate) fn shell_app(root: &Path, state: &Path) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app
}

/// An executable `#!/bin/sh` script at `root/name`, as `tasks.shell` (a program with known
/// signal behaviour: `sleep`, or `trap '' HUP` and a loop).
fn shell_script(root: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A fresh root and state dir under a tempdir.
pub(crate) fn dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let base = crate::workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    (dir, root, state)
}

/// "Terminal: New Shell" opens `tasks.shell` (here a link named `myshell`, so not a hardcoded
/// `/bin/sh`) in the active group; titles number shells of the same program only, a text tab
/// in the group not counting.
#[tokio::test(flavor = "multi_thread")]
async fn shell_new_shell_opens_the_shell_in_a_tab() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.test_open(Buffer::new_file(&root.join("a.txt")));
    let program = root.join("myshell");
    std::os::unix::fs::symlink("/bin/sh", &program).unwrap();
    app.tasks.shell = program;

    app.test_execute(Command::NewShell);
    let tab = app.group().active_shell().expect("a shell tab in the active group");
    assert_eq!(tab.title, "myshell ○", "named after the program, not a hardcoded shell");
    assert_eq!(tab.base, "myshell");
    assert_eq!(tab.number, 1);
    assert!(tab.idle, "the shell waits at its prompt");
    assert_eq!(tab.run.name, "myshell");
    assert_eq!(tab.run.cwd, root, "started in the workspace root");
    assert!(
        matches!(tab.run.status(), Status::Starting | Status::Running),
        "the run is starting or running: {:?}",
        tab.run.status()
    );
    assert!(app.group().active_tab().is_none(), "not a text tab");
    assert!(app.group().active_diff().is_none());
    assert!(app.group().active_hex().is_none());
    assert!(app.active_buffer().is_none(), "editor features find no buffer");
    assert!(app.active_mut().is_none());

    app.test_execute(Command::NewShell);
    assert_eq!(app.group().tabs.len(), 3, "the text tab and two shells");
    assert_eq!(app.group().active_shell().unwrap().title, "myshell (2) ○");
    assert_eq!(app.group().active_shell().unwrap().number, 2);
}

/// Numbers count only shells of the same program, and a closed one's number is used again
/// rather than repeated.
#[tokio::test(flavor = "multi_thread")]
async fn shell_titles_reuse_a_closed_number() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    for _ in 0..3 {
        app.test_execute(Command::NewShell);
    }
    let mut titles: Vec<String> = app.groups.shell_tabs().map(|s| s.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, ["sh (2) ○", "sh (3) ○", "sh ○"]);

    // Close the middle one ("sh (2)").
    app.group_mut().active = 1;
    app.test_execute(Command::CloseTab);
    let mut titles: Vec<String> = app.groups.shell_tabs().map(|s| s.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, ["sh (3) ○", "sh ○"]);

    app.test_execute(Command::NewShell);
    assert_eq!(app.group().active_shell().unwrap().title, "sh (2) ○", "the free number");
    let mut titles: Vec<String> = app.groups.shell_tabs().map(|s| s.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, ["sh (2) ○", "sh (3) ○", "sh ○"], "no repeated title");
}

/// The tab's title follows the shell's OSC codes (what the shell integration and programs send):
/// `sh ○` at the prompt, `133;B` + the snippet's OSC 0 title and the running glyph while a command
/// runs, a program's own OSC 2 title (vim's) until `133;D`, and back to the shell — red at the
/// prompt after a non-zero exit. Without the codes the title stays the shell's name.
#[tokio::test(flavor = "multi_thread")]
async fn shell_title_follows_the_shells_osc_codes() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    let title = |a: &App| a.group().active_shell().map(|s| s.title.clone()).unwrap_or_default();
    assert_eq!(title(&app), "sh ○", "the shell, at its prompt");

    // The integration's preexec (`133;B`) then its OSC 0 with the command's first word.
    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;B\\007\\033]0;sleep\\007'\n");
    assert!(
        app.settle(|a| title(a) == "sleep ●").await,
        "the title follows the command: {:?}; screen:\n{}",
        title(&app),
        shell_text(&app)
    );
    // The running glyph's theme colour.
    let term = draw(&mut app, 80, 24);
    let bar = app.hits.group(app.groups.active).unwrap().tab_bar;
    let drawn: String = (bar.x..bar.x + bar.width)
        .map(|x| term.backend().buffer()[(x, bar.y)].symbol())
        .collect();
    let glyph = drawn.find('●').expect("the running glyph is in the tab bar") as u16;
    assert_eq!(
        term.backend().buffer()[(bar.x + glyph, bar.y)].fg,
        app.theme.get("tab.shellRunning").fg.unwrap()
    );
    assert!(
        !app.group().active_shell().unwrap().error,
        "a running command isn't an error"
    );

    // Precmd with a zero exit (`133;D;0`): the shell is at its prompt again.
    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;D;0\\007'\n");
    assert!(
        app.settle(|a| title(a) == "sh ○").await,
        "back to the shell: {:?}",
        title(&app)
    );
    assert!(!app.group().active_shell().unwrap().error);

    // A program's own OSC 2 title (vim's) while it runs, until `133;D` resets it.
    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;B\\007\\033]2;file.rs \\342\\200\\224 VIM\\007'\n");
    assert!(
        app.settle(|a| title(a) == "file.rs — VIM ●").await,
        "the program's title replaces the first word: {:?}; screen:\n{}",
        title(&app),
        shell_text(&app)
    );
    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;D;0\\007'\n");
    assert!(
        app.settle(|a| title(a) == "sh ○").await,
        "`133;D` resets the program's title: {:?}",
        title(&app)
    );

    // A command that exits non-zero (`133;D;1`): the idle glyph in the error colour.
    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;B\\007\\033]0;false\\007'\n");
    assert!(
        app.settle(|a| title(a) == "false ●").await,
        "the command runs: {:?}",
        title(&app)
    );
    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;D;1\\007'\n");
    assert!(
        app.settle(|a| title(a) == "sh ○" && a.group().active_shell().is_some_and(|s| s.error))
            .await,
        "the failed command's prompt is an error: {:?}",
        title(&app)
    );
    let term = draw(&mut app, 80, 24);
    let bar = app.hits.group(app.groups.active).unwrap().tab_bar;
    let drawn: String = (bar.x..bar.x + bar.width)
        .map(|x| term.backend().buffer()[(x, bar.y)].symbol())
        .collect();
    let glyph = drawn.find('○').expect("the idle glyph is in the tab bar") as u16;
    assert_eq!(
        term.backend().buffer()[(bar.x + glyph, bar.y)].fg,
        app.theme.get("tab.shellError").fg.unwrap(),
        "a failed command's `○` is red"
    );
}

/// Without the integration's OSC codes nothing renames the tab: a shell's title stays its base
/// name even while a command runs (`sleep` no longer reaches the title — the `/proc` foreground
/// poller it used to come from is gone).
#[tokio::test(flavor = "multi_thread")]
async fn shell_without_osc_codes_keeps_its_base_name() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    let title = |a: &App| a.group().active_shell().map(|s| s.title.clone()).unwrap_or_default();

    app.group().active_shell().unwrap().run.write(b"sleep 30\n");
    // Two of the old watcher's poll intervals: it would have renamed the tab by now.
    let started = Instant::now();
    let pumped = app.settle(|_| started.elapsed() >= Duration::from_secs(2)).await;
    assert!(pumped, "the app pumped while `sleep` ran");
    assert_eq!(title(&app), "sh ○", "no OSC codes, so the title stays the base name");
    app.group().active_shell().unwrap().run.write(b"\x03"); // Ctrl+C: `sleep` ends.
}

/// Two shells of one program keep distinct titles while a command runs: the number is the
/// shell's (stable as its title changes), so the second shell's command is `sleep (2) ●` while the
/// idle first shell stays `sh ○`.
#[tokio::test(flavor = "multi_thread")]
async fn shell_running_titles_keep_the_per_program_number() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    app.test_execute(Command::NewShell);
    let title = |a: &App| a.group().active_shell().map(|s| s.title.clone()).unwrap_or_default();
    assert_eq!(title(&app), "sh (2) ○", "the second shell, at its prompt");

    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;B\\007\\033]0;sleep\\007'\n");
    assert!(
        app.settle(|a| title(a) == "sleep (2) ●").await,
        "the second shell's number stays while its command runs: {:?}",
        title(&app)
    );
    assert_eq!(
        app.groups.shell_tabs().next().map(|s| s.title.clone()).as_deref(),
        Some("sh ○"),
        "the idle first shell keeps its own (un-numbered) title"
    );

    // Precmd ends the command: the second shell is back at its numbered prompt.
    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf '\\033]133;D;0\\007'\n");
    assert!(
        app.settle(|a| title(a) == "sh (2) ○").await,
        "back to the numbered prompt: {:?}",
        title(&app)
    );
}

/// "Terminal: Install Shell Integration" writes the snippets into the state dir and opens the
/// wizard with the detected shell's init file and the line sourcing its snippet: C copies the line
/// (nothing on disk), Esc closes; no state dir, or a shell with no snippet (`sh`), says so.
#[tokio::test(flavor = "multi_thread")]
async fn shell_install_wizard_shows_the_paths_and_copies_the_line() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/zsh".into();

    // No state dir in tests: nothing to write the snippets to.
    app.test_execute(Command::InstallShellIntegration);
    assert!(app.prompt.is_none());
    assert_eq!(message(&app), "No shell integration: there's no state directory");

    app.state_dir = Some(state.clone());
    app.test_execute(Command::InstallShellIntegration);
    let (snippet, init, line) = match app.prompt.as_ref() {
        Some(Prompt::InstallShellIntegration { snippet, init, line }) => (snippet.clone(), init.clone(), line.clone()),
        _ => panic!("the wizard opened"),
    };
    assert_eq!(snippet, state.join("shell-integration.zsh"));
    assert_eq!(
        init,
        crate::paths::home().join(".zshrc"),
        "the detected shell's init file"
    );
    assert_eq!(line, format!("source '{}'", snippet.display()));
    assert!(
        app.settle(|_| state.join("shell-integration.bash").is_file()).await,
        "the snippets were written off-thread"
    );

    // The dialog shows the paths, the line and the buttons.
    let term = draw(&mut app, 100, 30);
    let drawn = rows(&term, ratatui::layout::Rect::new(0, 0, 100, 30)).join("\n");
    let snippet_shown = crate::recent::display(&snippet, &crate::paths::home());
    for needle in [line.as_str(), snippet_shown.as_str(), "[C]opy", "[A]ppend"] {
        assert!(drawn.contains(needle), "the dialog shows {needle:?}:\n{drawn}");
    }
    assert!(
        drawn.contains(&format!(
            "[A]ppend to ~/{}",
            init.strip_prefix(crate::paths::home()).unwrap().display()
        )),
        "the append button names the init file:\n{drawn}"
    );

    // C copies the line and leaves the dialog open.
    app.press(KeyCode::Char('c'), KeyModifiers::NONE);
    assert_eq!(app.clipboard.read().unwrap(), line);
    assert_eq!(message(&app), "Copied the shell integration line");
    assert!(matches!(app.prompt, Some(Prompt::InstallShellIntegration { .. })));

    app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.prompt.is_none());

    // `sh` has no snippet: a message instead of the wizard.
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::InstallShellIntegration);
    assert!(app.prompt.is_none());
    assert_eq!(
        message(&app),
        "No shell integration for 'sh': zsh, bash and fish have one"
    );
}

/// The wizard's A appends the line off-thread (here to a temp init file, never the real `~/.zshrc`)
/// and reports it: one newline-first line, then a new shell tab picks it up.
#[tokio::test(flavor = "multi_thread")]
async fn shell_install_wizard_appends_the_line() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    let init = state.join("zshrc");
    std::fs::write(&init, "export A=1").unwrap();
    let line = "source '/state/shell-integration.zsh'".to_string();
    app.prompt = Some(Prompt::InstallShellIntegration {
        snippet: state.join("shell-integration.zsh"),
        init: init.clone(),
        line: line.clone(),
    });

    app.press(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(app.prompt.is_none(), "appending closes the wizard");
    assert!(
        app.settle(|a| message(a).starts_with("Appended to ")).await,
        "the append was reported: {:?}",
        message(&app)
    );
    assert_eq!(std::fs::read_to_string(&init).unwrap(), format!("export A=1\n{line}\n"));
    assert_eq!(
        message(&app),
        format!(
            "Appended to {} — open a new shell tab",
            crate::recent::display(&init, &crate::paths::home())
        )
    );
}

/// The app's last message, for the tests above.
fn message(app: &App) -> String {
    app.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

/// `bash`'s path, or a printed skip note and `None` (the snippet tests run the real bash).
fn bash_or_skip(test: &str) -> Option<PathBuf> {
    let bash = ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file());
    if bash.is_none() {
        eprintln!("skipped {test}: needs bash");
    }
    bash
}

/// A shell running the real bash snippet: with `JED_IPC` set (a listening socket) the guard passes,
/// and its hooks title the tab — a command while it runs (`read ●`), a program's own OSC 2 title
/// until `133;D` (`file.rs — VIM ●`), and red at the prompt after a failed one. `read` waits for a
/// line so a command can be ended without a signal racing its start.
#[tokio::test(flavor = "multi_thread")]
async fn shell_bash_snippet_titles_a_real_shell() {
    let Some(bash) = bash_or_skip("shell_bash_snippet_titles_a_real_shell") else {
        return;
    };
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let (mut app, _socket) = listening_app(&root, &state, &sockets).await;
    let wrapper = shell_script(
        &root,
        "myshell",
        &format!("exec {} --norc --noprofile\n", bash.display()),
    );
    app.tasks.shell = wrapper;
    app.test_execute(Command::NewShell);
    let title = |a: &App| a.group().active_shell().map(|s| s.title.clone()).unwrap_or_default();
    assert_eq!(title(&app), "myshell ○");

    crate::shell_integration::write_snippets(&state).unwrap();
    let snippet = state.join("shell-integration.bash");
    run_in_shell(&mut app, &format!("source '{}'", snippet.display()));
    run_in_shell(&mut app, "read x");
    assert!(
        app.settle(|a| title(a) == "read ●").await,
        "preexec and its OSC 0 name the command: {:?}\n{}",
        title(&app),
        shell_text(&app)
    );
    // A line ends `read`: the shell is back at its prompt (a success, so not red).
    app.group().active_shell().unwrap().run.write(b"\n");
    assert!(
        app.settle(|a| title(a) == "myshell ○" && a.group().active_shell().is_some_and(|s| !s.error))
            .await,
        "back at the prompt: {:?}\n{}",
        title(&app),
        shell_text(&app)
    );

    // A program's own title, until its command ends (`133;D` resets it).
    run_in_shell(&mut app, "sh -c 'printf \"\\033]0;file.rs — VIM\\007\"; read x'");
    assert!(
        app.settle(|a| title(a) == "file.rs — VIM ●").await,
        "the program's title replaces the command's: {:?}\n{}",
        title(&app),
        shell_text(&app)
    );
    app.group().active_shell().unwrap().run.write(b"\n");
    assert!(
        app.settle(|a| title(a) == "myshell ○").await,
        "`133;D` resets the program's title: {:?}",
        title(&app)
    );

    // `false`: the idle glyph in the error colour.
    run_in_shell(&mut app, "false");
    assert!(
        app.settle(|a| title(a) == "myshell ○" && a.group().active_shell().is_some_and(|s| s.error))
            .await,
        "a failed command's prompt: {:?}",
        title(&app)
    );
    let term = draw(&mut app, 80, 24);
    let bar = app.hits.group(app.groups.active).unwrap().tab_bar;
    let drawn: String = (bar.x..bar.x + bar.width)
        .map(|x| term.backend().buffer()[(x, bar.y)].symbol())
        .collect();
    let glyph = drawn.find('○').expect("the idle glyph is in the tab bar") as u16;
    assert_eq!(
        term.backend().buffer()[(bar.x + glyph, bar.y)].fg,
        app.theme.get("tab.shellError").fg.unwrap(),
        "the failed command's `○` is red"
    );
}

/// Without `JED_IPC` (no socket) the snippet's guard returns: sourcing it does nothing, so a
/// command doesn't rename the tab and the shell keeps running as if it weren't there.
#[tokio::test(flavor = "multi_thread")]
async fn shell_bash_snippet_does_nothing_without_jed_ipc() {
    let Some(bash) = bash_or_skip("shell_bash_snippet_does_nothing_without_jed_ipc") else {
        return;
    };
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    let wrapper = shell_script(
        &root,
        "myshell",
        &format!("exec {} --norc --noprofile\n", bash.display()),
    );
    app.tasks.shell = wrapper;
    app.test_execute(Command::NewShell);
    let title = |a: &App| a.group().active_shell().map(|s| s.title.clone()).unwrap_or_default();

    crate::shell_integration::write_snippets(&state).unwrap();
    let snippet = state.join("shell-integration.bash");
    run_in_shell(&mut app, &format!("source '{}'", snippet.display()));
    // The guard's `return` ended the sourcing only: the shell still runs commands.
    run_in_shell(&mut app, "echo $((6*7))");
    assert!(
        app.settle(|a| shell_text(a).contains("42")).await,
        "the shell runs on after the guard returned:\n{}",
        shell_text(&app)
    );

    // A command prints a marker (not in the typed line) and then waits, so the title is checked
    // while one really runs.
    run_in_shell(&mut app, "printf 'M%s\\n' ARK; read x");
    assert!(
        app.settle(|a| shell_text(a).contains("MARK")).await,
        "the command runs:\n{}",
        shell_text(&app)
    );
    let started = Instant::now();
    assert!(app.settle(|_| started.elapsed() >= Duration::from_millis(300)).await);
    assert_eq!(title(&app), "myshell ○", "no JED_IPC, so nothing renames the tab");
    assert!(!app.group().active_shell().unwrap().error);
    app.group().active_shell().unwrap().run.write(b"\n");
}

/// The shell gets a terminal (`TERM`, `COLORTERM`); without a listening socket its environment
/// has no `JED_IPC` (tests start no socket).
#[tokio::test(flavor = "multi_thread")]
async fn shell_runs_with_a_terminal_and_no_ipc_without_a_socket() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);

    let run = &app.group().active_shell().unwrap().run;
    run.write(b"printf 'T=%s C=%s I=%s\\n' \"$TERM\" \"$COLORTERM\" \"${JED_IPC-unset}\"\n");
    assert!(
        app.settle(|a| {
            a.group().active_shell().is_some_and(|s| {
                let screen = s.run.screen();
                let text = screen.screen().contents();
                text.contains("T=xterm-256color C=truecolor I=unset")
            })
        })
        .await,
        "the shell's output should show the environment"
    );
}

/// A listening socket (as `App::ipc_started` records it) puts its path in `JED_IPC`.
#[tokio::test(flavor = "multi_thread")]
async fn shell_gets_jed_ipc_when_the_socket_listens() {
    let (_d, root, state) = dirs();
    let ws = Workspace::load(root.clone());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    // A short socket dir, as `git_editor_tests` uses (`sun_path` is ~104 bytes on macOS).
    app.ipc_dir = Some(crate::workspace::normalize(_d.path()).join("s"));
    app.test_start();
    assert!(app.settle(|a| a.tasks.ipc.is_some()).await, "the socket is listening");
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);

    let expected = format!("IPC={}", app.tasks.ipc.as_ref().unwrap().display());
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(b"printf 'IPC=%s\\n' \"$JED_IPC\"\n");
    let seen = app
        .settle(|a| {
            let text = a.group().active_shell().unwrap().run.screen().screen().contents();
            // The prompt is on the line the output starts on ("$ IPC=…").
            text.lines().any(|line| line.contains(&expected))
        })
        .await;
    assert!(
        seen,
        "the shell sees JED_IPC={expected}; screen:\n{}",
        app.group().active_shell().unwrap().run.screen().screen().contents()
    );
}

/// A shell starts at the active group's editor size, not the first group's.
#[tokio::test(flavor = "multi_thread")]
async fn shell_size_is_the_active_groups_editor() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    // No side panel, odd width: the editor area's halves differ (49 / 48).
    app.ui.side_visible = false;
    let mut term = Terminal::new(TestBackend::new(101, 30)).unwrap();
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();
    app.test_execute(Command::SplitRight);
    term.draw(|f| crate::ui::render(f, &mut app)).unwrap();

    let active = app.groups.active;
    let editor = app.hits.group(active).expect("the active group's editor").editor;
    let first = app.groups.ids()[0];
    assert_ne!(first, active);
    let first_editor = app.hits.group(first).unwrap().editor;
    assert_ne!(editor.width, first_editor.width, "the split's groups differ in width");

    app.test_execute(Command::NewShell);
    let size = app.group().active_shell().unwrap().run.screen().screen().size();
    assert_eq!(
        size,
        (editor.height, editor.width),
        "the shell starts at its group's editor size"
    );
}

/// A shell keeps the `scrollback` setting's lines above the screen (read when it opens).
#[tokio::test(flavor = "multi_thread")]
async fn shell_scrollback_comes_from_the_setting() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.settings.scrollback = 2;
    app.test_execute(Command::NewShell);

    let run = &app.group().active_shell().unwrap().run;
    run.write(format!("{COUNT_TO_200}\n").as_bytes());
    assert!(
        app.settle(|a| {
            let text = a.group().active_shell().unwrap().run.screen().screen().contents();
            text.contains("199")
        })
        .await,
        "the command's output arrived"
    );
    let mut screen = app.group().active_shell().unwrap().run.screen();
    screen.screen_mut().set_scrollback(usize::MAX);
    assert_eq!(screen.screen().scrollback(), 2, "only the setting's lines are kept");
}

/// Task runs read the same setting when they start.
#[tokio::test(flavor = "multi_thread")]
async fn task_scrollback_comes_from_the_setting() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.settings.scrollback = 2;
    app.tasks.size = (2, 20);
    let template = Template {
        name: "count".into(),
        task: Task {
            command: COUNT_TO_200.into(),
            cwd: None,
            env: BTreeMap::new(),
        },
        source: Source::User,
    };
    let id = app.start_task(&template);
    // The end note replaces what the two-row screen showed, so wait for the exit itself.
    let ended = app
        .settle(|a| a.tasks.get(id).is_some_and(|r| !r.status().is_running()))
        .await;
    assert!(ended, "the task ended");
    let mut screen = app.tasks.get(id).unwrap().screen();
    screen.screen_mut().set_scrollback(usize::MAX);
    assert_eq!(screen.screen().scrollback(), 2, "only the setting's lines are kept");
}

/// Splitting on a shell leaves the new group empty: a shell can't be in two groups.
#[tokio::test(flavor = "multi_thread")]
async fn shell_split_leaves_the_new_group_empty() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);

    app.test_execute(Command::SplitRight);
    assert_eq!(app.groups.len(), 2);
    assert!(app.group().tabs.is_empty(), "a shell is not copied into the new group");
    let first = app.groups.ids()[0];
    assert_eq!(app.groups.get(first).unwrap().active_shell().unwrap().title, "sh ○");
}

/// A shell closes without asking (it's never dirty).
#[tokio::test(flavor = "multi_thread")]
async fn shell_closes_without_a_prompt() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);

    app.test_execute(Command::CloseTab);
    assert!(app.prompt.is_none(), "no save prompt for a shell");
    assert!(app.group().tabs.is_empty());
    assert!(app.group().active_shell().is_none());
}

/// A draw at `w` × `h`, as `run` does.
pub(crate) fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

/// The drawn rows of `area`, trailing blanks trimmed.
fn rows(term: &Terminal<TestBackend>, area: ratatui::layout::Rect) -> Vec<String> {
    let buf = term.backend().buffer();
    (area.y..area.y + area.height)
        .map(|y| {
            let row: String = (area.x..area.x + area.width).map(|x| buf[(x, y)].symbol()).collect();
            row.trim_end().to_string()
        })
        .collect()
}

/// The active shell's screen text.
pub(crate) fn screen_text(app: &App) -> String {
    app.group().active_shell().unwrap().run.screen().screen().contents()
}

/// The active shell's scrollback position (0 = the live screen).
pub(crate) fn scrollback(app: &App) -> usize {
    app.group().active_shell().unwrap().run.screen().screen().scrollback()
}

/// The shell process group is gone (`kill(-pgid, 0)` fails).
fn group_gone(pid: u32) -> bool {
    // SAFETY: signal 0 checks for the group's existence and sends nothing.
    unsafe { libc::kill(-(pid as i32), 0) != 0 }
}

/// An app on `root` with a listening socket in a short temp subdir (`sun_path` is ~104 bytes on
/// macOS), its `$SHELL` for "Terminal: New Shell" set to `/bin/sh`; with the socket path.
async fn listening_app(root: &Path, state: &Path, sockets: &tempfile::TempDir) -> (App, PathBuf) {
    let dir = crate::workspace::normalize(sockets.path()).join("s");
    let socket = crate::ipc::socket_path(&dir, std::process::id());
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.ipc_dir = Some(dir);
    let config_dir = state.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    app.config_dir = config_dir;
    app.test_start();
    assert!(app.settle(|a| a.tasks.ipc.is_some()).await, "the socket is listening");
    assert_eq!(app.tasks.ipc.as_deref(), Some(socket.as_path()));
    app.tasks.shell = "/bin/sh".into();
    (app, socket)
}

/// `text` into the focused shell as a paste (a long command line arrives at once).
fn paste(app: &mut App, text: &str) {
    app.handle_test_event(CEvent::Paste(text.into()));
}

/// A command line in the focused shell: pasted, then Enter.
fn run_in_shell(app: &mut App, line: &str) {
    paste(app, line);
    app.press(KeyCode::Enter, KeyModifiers::NONE);
}

/// Whether any shell tab's screen text contains `needle` (the shell is not the active tab after
/// an IPC-opened file takes it).
fn screen_has(app: &App, needle: &str) -> bool {
    app.groups
        .shell_tabs()
        .any(|s| s.run.screen().screen().contents().contains(needle))
}

/// Every shell tab's screen text (for a failing test's message).
fn shell_text(app: &App) -> String {
    app.groups
        .shell_tabs()
        .map(|s| s.run.screen().screen().contents())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The real `jed` executable: unit tests get no `CARGO_BIN_EXE_jed`, so fall back to the target
/// dir's binary (`<target>/<profile>/deps/jed-<hash>` → `<target>/<profile>/jed`).
fn jed_binary() -> Option<PathBuf> {
    if let Some(path) = option_env!("CARGO_BIN_EXE_jed")
        && Path::new(path).is_file()
    {
        return Some(path.into());
    }
    let path = std::env::current_exe().ok()?.parent()?.parent()?.join("jed");
    path.is_file().then_some(path)
}

/// `jed_binary`, or a printed skip note and `None`: a test needing the real binary passes
/// without it (as other environment-dependent tests do).
fn jed_or_skip(test: &str) -> Option<PathBuf> {
    let binary = jed_binary();
    if binary.is_none() {
        eprintln!("skipped {test}: needs the jed binary");
    }
    binary
}

/// A git command run outside the shell (`init`, `add`, `log`), its trimmed stdout when it worked.
fn git_in(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Quits the app (as `run` does) and checks every shell's process group is gone.
async fn quit_shells(app: &mut App) {
    let pids: Vec<u32> = app.groups.shell_tabs().filter_map(|s| s.run.pid()).collect();
    app.test_execute(Command::Quit);
    app.shutdown().await;
    assert!(
        app.settle(|_| pids.iter().all(|&pid| group_gone(pid))).await,
        "no shell process is left after quit"
    );
}

/// Every key bound to nothing goes to the shell as a terminal sends it (typed chars, Up, Esc,
/// Ctrl+C), as a paste does; the program (here `cat`) echoes those bytes.
#[tokio::test(flavor = "multi_thread")]
async fn shell_keys_and_pastes_go_to_the_shell() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/cat".into();
    app.test_execute(Command::NewShell);
    assert!(
        app.group().active_shell().is_some(),
        "the shell tab is open and focused"
    );

    app.type_text("abc");
    app.press(KeyCode::Up, KeyModifiers::NONE);
    app.press(KeyCode::Esc, KeyModifiers::NONE);
    app.handle_test_event(CEvent::Paste("xy".into()));
    assert!(
        app.settle(|a| screen_text(a).contains("abc^[[A^[xy")).await,
        "the cat echoes the keys and the paste as terminal bytes:\n{}",
        screen_text(&app)
    );

    app.press(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(
        app.settle(|a| !a.group().active_shell().unwrap().run.status().is_running())
            .await,
        "Ctrl+C reached the terminal (SIGINT ended the cat):\n{}",
        screen_text(&app)
    );

    // jed's global keys still work from a shell tab: the palette opens.
    app.press(KeyCode::Char('p'), cmd() | KeyModifiers::SHIFT);
    assert!(app.palette.is_some(), "the palette opens from a shell tab");
}

/// `shell_key` / `shell_paste` are gated by the shell's focus: with the focus on the side panel,
/// keys and pastes no longer reach the shell (`cat` would echo them).
#[tokio::test(flavor = "multi_thread")]
async fn shell_keys_and_pastes_stop_when_the_shell_is_unfocused() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/cat".into();
    app.test_execute(Command::NewShell);
    assert!(
        app.group().active_shell().is_some(),
        "the shell tab is open and focused"
    );

    app.ui.focus = Focus::SidePanel;
    app.type_text("nope");
    app.handle_test_event(CEvent::Paste("nada".into()));
    assert!(
        app.settle(|a| !screen_text(a).contains("nope") && !screen_text(a).contains("nada"))
            .await,
        "nothing reached the shell while it was unfocused:\n{}",
        screen_text(&app)
    );
}

/// Keys and pastes follow the modes the shell's program turned on: `SS3` arrows in application
/// cursor mode and the paste wrapped for bracketed paste (both read from the emulated screen).
#[tokio::test(flavor = "multi_thread")]
async fn shell_keys_follow_the_shells_terminal_modes() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    // sh prints the mode-setting sequences (DECCKM, bracketed paste), then cat echoes keys.
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(b"printf '\\033[?1h\\033[?2004h'; cat\n");
    assert!(
        app.settle(|a| {
            let run = &a.group().active_shell().unwrap().run;
            let screen = run.screen();
            screen.screen().application_cursor() && screen.screen().bracketed_paste()
        })
        .await,
        "the shell turned both modes on:\n{}",
        screen_text(&app)
    );

    app.press(KeyCode::Up, KeyModifiers::NONE);
    app.handle_test_event(CEvent::Paste("xy".into()));
    assert!(
        app.settle(|a| {
            let text = screen_text(a);
            // ECHOCTL writes ESC as `^[`, then the rest of the sequence literally.
            text.contains("^[OA") && text.contains("^[[200~xy^[[201~")
        })
        .await,
        "the arrow is SS3 and the paste bracketed (cat echoed them):\n{}",
        screen_text(&app)
    );
}

/// Typing a command and Enter runs it in the shell.
#[tokio::test(flavor = "multi_thread")]
async fn shell_typing_and_enter_run_a_command() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);

    app.type_text("echo $((6*7))");
    app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        app.settle(|a| screen_text(a).contains("42")).await,
        "the shell ran the typed command:\n{}",
        screen_text(&app)
    );
    assert!(app.group().active_shell().unwrap().run.status().is_running());
}

/// The scrollback bindings and the wheel scroll the shell's output; Ctrl+End returns to the
/// live screen and Ctrl+Home goes to the oldest output kept.
#[tokio::test(flavor = "multi_thread")]
async fn shell_scrollback_keys_and_wheel_scroll() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(format!("{COUNT_TO_200}\n").as_bytes());
    assert!(
        app.settle(|a| { screen_text(a).lines().any(|l| l.trim_end().ends_with("199")) })
            .await,
        "the command's output arrived:\n{}",
        screen_text(&app)
    );

    app.press(KeyCode::PageUp, KeyModifiers::SHIFT);
    assert!(scrollback(&app) > 0, "Shift+PageUp goes back");
    app.press(KeyCode::PageDown, KeyModifiers::SHIFT);
    assert_eq!(scrollback(&app), 0, "Shift+PageDown is back on the live screen");
    app.press(KeyCode::PageUp, KeyModifiers::SHIFT);
    assert!(scrollback(&app) > 0);
    app.press(KeyCode::End, KeyModifiers::CONTROL);
    assert_eq!(scrollback(&app), 0, "Ctrl+End is the live screen");
    app.press(KeyCode::Home, KeyModifiers::CONTROL);
    let top = scrollback(&app);
    assert!(top > 100, "Ctrl+Home is the oldest output kept");
    app.press(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(
        scrollback(&app),
        top - 1,
        "Ctrl+Shift+Down is a line towards the live screen"
    );
    app.press(KeyCode::End, KeyModifiers::CONTROL);
    app.press(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert!(scrollback(&app) > 0, "Ctrl+Shift+Up goes back a line");
    app.press(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(scrollback(&app), 0);

    // The wheel over the editor area scrolls it too.
    app.press(KeyCode::End, KeyModifiers::CONTROL);
    draw(&mut app, 100, 30);
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    app.mouse(MouseEventKind::ScrollUp, editor.x + 1, editor.y + 1);
    assert!(scrollback(&app) > 0, "the wheel scrolls the shell's scrollback");
}

/// The wheel scrolls the shell of the group it is over, without focusing it: over an inactive
/// group's editor, only that group's shell scrolls back.
#[tokio::test(flavor = "multi_thread")]
async fn shell_wheel_scrolls_a_non_focused_groups_shell() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    app.test_execute(Command::SplitRight);
    app.test_execute(Command::NewShell);
    assert_eq!(app.groups.shell_tabs().count(), 2, "a shell in each group");
    for shell in app.groups.shell_tabs() {
        shell.run.write(format!("{COUNT_TO_200}\n").as_bytes());
    }
    assert!(
        app.settle(|a| {
            a.groups.shell_tabs().all(|s| {
                s.run
                    .screen()
                    .screen()
                    .contents()
                    .lines()
                    .any(|l| l.trim_end() == "199")
            })
        })
        .await,
        "both shells printed the output:\n{}",
        shell_text(&app)
    );

    let active = app.groups.active;
    let inactive = *app.groups.ids().iter().find(|&&g| g != active).expect("two groups");
    draw(&mut app, 100, 30);
    let editor = app.hits.group(inactive).unwrap().editor;
    app.mouse(MouseEventKind::ScrollUp, editor.x + 1, editor.y + 1);
    let scrollback_of = |a: &App, g| {
        a.groups
            .get(g)
            .unwrap()
            .active_shell()
            .unwrap()
            .run
            .screen()
            .screen()
            .scrollback()
    };
    assert!(scrollback_of(&app, inactive) > 0, "the inactive group's shell scrolled");
    assert_eq!(scrollback_of(&app, active), 0, "the focused group's shell did not");
}

/// The screen is drawn in the editor area, at its size; resizing the frame (a split included)
/// resizes the PTY (and so the emulated screen).
#[tokio::test(flavor = "multi_thread")]
async fn shell_draws_in_the_editor_area_and_resizes_with_it() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    // A script (not echoed by the tty) that prints one line and then stops writing: what the
    // frame draws can't change under the comparison below.
    let marker = "jed-shell-drawn";
    app.tasks.shell = shell_script(&root, "drawn", &format!("echo {marker}\nexec sleep 30"));
    app.test_execute(Command::NewShell);
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.contains(marker))).await,
        "the shell printed the marker:\n{}",
        screen_text(&app)
    );

    let term = draw(&mut app, 100, 30);
    let active = app.groups.active;
    let editor = app.hits.group(active).unwrap().editor;
    let size = app.group().active_shell().unwrap().run.screen().screen().size();
    assert_eq!(size, (editor.height, editor.width), "the PTY is the editor area's size");
    let drawn = rows(&term, editor);
    assert!(
        drawn.iter().any(|r| r.contains(marker)),
        "the shell's text is drawn:\n{drawn:#?}"
    );
    // The shell may keep printing after the frame; the rows it had then are the drawn ones.
    let contents = screen_text(&app);
    assert_eq!(drawn.len(), editor.height as usize);
    for (i, (row, line)) in drawn.iter().zip(contents.lines()).enumerate() {
        assert_eq!(*row, line.trim_end(), "drawn row {i} is the screen's row");
    }

    let term = draw(&mut app, 90, 24);
    let editor = app.hits.group(active).unwrap().editor;
    let size = app.group().active_shell().unwrap().run.screen().screen().size();
    assert_eq!(size, (editor.height, editor.width), "a new frame resizes the PTY");
    assert!(rows(&term, editor).iter().any(|r| r.contains(marker)));

    let wide = editor.width;
    app.test_execute(Command::SplitRight);
    let shell_group = app.groups.ids()[0];
    let term = draw(&mut app, 90, 24);
    let editor = app.hits.group(shell_group).unwrap().editor;
    assert!(editor.width < wide, "the split narrowed the shell's group");
    let size = app
        .groups
        .get(shell_group)
        .unwrap()
        .active_shell()
        .unwrap()
        .run
        .screen()
        .screen()
        .size();
    assert_eq!(size, (editor.height, editor.width), "a split resizes the shell's group");
    assert!(
        !term.backend().cursor_visible(),
        "the shell of an inactive group shows no cursor"
    );
}

/// The screen fills the editor area with the terminal's colours (no scroll-bar column), and the
/// terminal cursor is only on the live screen of the focused group while the shell runs.
#[tokio::test(flavor = "multi_thread")]
async fn shell_draws_colours_and_the_cursor() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    // The command line echoes, so wait for output the typed line can't contain.
    app.group().active_shell().unwrap().run.write(b"echo RE''ADY\n");
    assert!(
        app.settle(|a| screen_text(a).contains("READY")).await,
        "the shell ran the command:\n{}",
        screen_text(&app)
    );

    let active = app.groups.active;
    let term = draw(&mut app, 100, 30);
    let hit = app.hits.group(active).unwrap();
    let editor = hit.editor;
    assert_eq!(hit.text, editor, "the shell uses the whole editor area");
    assert!(hit.scrollbar.is_none(), "no scroll-bar column for a shell");
    assert_eq!(
        editor.width, hit.tab_bar.width,
        "the shell is as wide as its group: no column is reserved for a bar"
    );
    let bg = app.theme.get("terminal.background").bg.unwrap();
    let corner = term.backend().buffer()[(editor.x + editor.width - 1, editor.y + editor.height - 1)].bg;
    assert_eq!(corner, bg, "blank cells take the terminal's background");
    assert!(
        term.backend().cursor_visible(),
        "the focused live shell shows the cursor"
    );
    let (row, col) = {
        let run = &app.group().active_shell().unwrap().run;
        let screen = run.screen();
        screen.screen().cursor_position()
    };
    assert_eq!(
        term.backend().cursor_position(),
        (editor.x + col, editor.y + row).into(),
        "the cursor is at the shell's cursor"
    );

    // Not focused: no cursor.
    app.ui.focus = Focus::SidePanel;
    let term = draw(&mut app, 100, 30);
    assert!(!term.backend().cursor_visible(), "an unfocused shell shows no cursor");
    app.ui.focus = Focus::Editor;

    // Scrolled back: no cursor; on the live screen again: the cursor.
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(format!("{COUNT_TO_200}\n").as_bytes());
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.trim_end() == "199"))
            .await,
        "the command's output arrived:\n{}",
        screen_text(&app)
    );
    app.press(KeyCode::Home, KeyModifiers::CONTROL);
    assert!(scrollback(&app) > 0);
    let term = draw(&mut app, 100, 30);
    assert!(!term.backend().cursor_visible(), "no cursor over the scrollback");
    app.press(KeyCode::End, KeyModifiers::CONTROL);
    let term = draw(&mut app, 100, 30);
    assert!(
        term.backend().cursor_visible(),
        "back on the live screen the cursor returns"
    );

    // An ended shell keeps its output but has no cursor.
    app.group().active_shell().unwrap().run.write(b"exit\n");
    assert!(
        app.settle(|a| !a.group().active_shell().unwrap().run.status().is_running())
            .await,
        "the shell ended"
    );
    let term = draw(&mut app, 100, 30);
    assert!(!term.backend().cursor_visible(), "an ended shell shows no cursor");
}

/// A shell that exits keeps its tab, its output and a note; the note is the status-bar message
/// too.
#[tokio::test(flavor = "multi_thread")]
async fn shell_exit_keeps_its_tab_and_the_note() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    app.group().active_shell().unwrap().run.write(b"echo leaving; exit 3\n");
    let note = "Shell 'sh' exited with code 3";
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.contains(note))).await,
        "the exit note is written below the output:\n{}",
        screen_text(&app)
    );
    let tab = app.group().active_shell().expect("the tab stays until closed");
    assert!(matches!(tab.run.status(), Status::Exited(_)));
    assert!(screen_text(&app).contains("leaving"), "the output stays too");
    assert_eq!(
        app.message.as_ref().map(|m| m.text.as_str()),
        Some(note),
        "the note is the status-bar message"
    );
    assert_eq!(
        app.message.as_ref().map(|m| m.level),
        Some(Level::Warning),
        "a failed exit is a warning"
    );
    // The note is drawn, still in the editor area.
    let active = app.groups.active;
    let term = draw(&mut app, 100, 30);
    let editor = app.hits.group(active).unwrap().editor;
    assert!(rows(&term, editor).iter().any(|r| r.contains(note)));
}

/// Success is an Info note; a shell that never starts says so at Error, and its tab stays too.
#[tokio::test(flavor = "multi_thread")]
async fn shell_exit_notes_cover_success_and_failure_to_start() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    app.group().active_shell().unwrap().run.write(b"exit\n");
    let note = "Shell 'sh' exited with code 0";
    assert!(
        app.settle(|a| screen_text(a).lines().any(|l| l.contains(note))).await,
        "a clean exit says so:\n{}",
        screen_text(&app)
    );
    assert_eq!(
        app.message.as_ref().map(|m| m.text.as_str()),
        Some(note),
        "the note is the status-bar message"
    );
    assert_eq!(app.message.as_ref().map(|m| m.level), Some(Level::Info));
    assert!(matches!(
        app.group().active_shell().unwrap().run.status(),
        Status::Exited(_)
    ));

    // A program that can't start: its tab stays with the reason below it.
    app.tasks.shell = root.join("missing-shell");
    app.test_execute(Command::NewShell);
    let failed = app
        .settle(|a| {
            a.group()
                .active_shell()
                .is_some_and(|s| matches!(s.run.status(), Status::Failed(_)))
        })
        .await;
    assert!(failed, "the spawn failed");
    assert!(
        app.settle(|a| screen_text(a).contains("failed to start")).await,
        "the failure note is below the output:\n{}",
        screen_text(&app)
    );
    let text = app.message.as_ref().map(|m| m.text.as_str()).unwrap_or_default();
    assert!(
        text.starts_with("Shell 'missing-shell' failed to start:"),
        "the reason is in the message: {text:?}"
    );
    assert_eq!(app.message.as_ref().map(|m| m.level), Some(Level::Error));
    assert!(
        app.group().active_shell().is_some(),
        "the failed shell's tab stays until closed"
    );
}

/// A shell ended by a signal (not a `kill` by jed, and not an exit code) says so in its note,
/// at `Warning` — the branch `tasks::end_note`'s "ended by signal" and `end_run_note`'s warning
/// for it, covered end to end rather than only by the unit test.
#[tokio::test(flavor = "multi_thread")]
async fn shell_a_signal_ended_shell_says_so_as_a_warning() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = shell_script(&root, "die", "kill -TERM $$");
    app.test_execute(Command::NewShell);
    // The note is appended (and the warning shown) when the app handles the run's `Ended` event,
    // a beat after the waiter thread marks it `Exited` — settle on the note, not the raw status.
    assert!(
        app.settle(|a| screen_text(a).contains("ended by signal")).await,
        "the shell ended by SIGTERM:\n{}",
        screen_text(&app)
    );
    assert_eq!(
        app.message.as_ref().map(|m| m.level),
        Some(Level::Warning),
        "a signal-ended shell is a warning"
    );
}

/// Closing a shell tab kills its shell (the run is dropped); Cmd+W works in the shell context.
#[tokio::test(flavor = "multi_thread")]
async fn shell_close_kills_the_shell() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    assert!(
        app.settle(|a| a.group().active_shell().and_then(|s| s.run.pid()).is_some())
            .await,
        "the shell started"
    );
    let pid = app.group().active_shell().unwrap().run.pid().unwrap();

    app.press(KeyCode::Char('w'), cmd());
    assert!(app.prompt.is_none(), "no prompt for a shell");
    assert!(app.group().active_shell().is_none(), "the tab closed");
    assert!(app.settle(|_| group_gone(pid)).await, "the shell process group is gone");

    // Its run's late events (the Ended) name a tab nothing holds: no note, no panic.
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        app.test_turn(Duration::from_millis(50)).await;
    }
    assert!(app.message.is_none(), "no exit note for a closed shell");
}

/// Quitting hangs every shell up (one per group) with SIGHUP, before `shutdown` returns.
#[tokio::test(flavor = "multi_thread")]
async fn shell_quit_hangs_the_shell_up() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = shell_script(&root, "well-behaved", "sleep 30");
    app.test_execute(Command::NewShell);
    app.test_execute(Command::SplitRight);
    app.test_execute(Command::NewShell);
    assert!(
        app.settle(|a| a.groups.shell_tabs().filter(|s| s.run.pid().is_some()).count() == 2)
            .await,
        "a shell runs in each group"
    );
    let pids: Vec<u32> = app.groups.shell_tabs().filter_map(|s| s.run.pid()).collect();

    app.test_execute(Command::Quit);
    app.shutdown().await;
    assert!(
        app.settle(|_| pids.iter().all(|&pid| group_gone(pid))).await,
        "no shell process is left after quit"
    );
    for shell in app.groups.shell_tabs() {
        match shell.run.status() {
            Status::Exited(e) => assert!(
                e.signal.as_deref().is_some_and(|s| s.contains("Hangup")),
                "{} ended by SIGHUP, not escalated: {e:?}",
                shell.title
            ),
            other => panic!("{} still {other:?}", shell.title),
        }
    }
}

/// Quitting a shell that ignores SIGHUP waits `SHUTDOWN_TIMEOUT` and then SIGKILLs it: bounded,
/// and gone at once rather than left to `Run::kill`'s later grace.
#[tokio::test(flavor = "multi_thread")]
async fn shell_quit_kills_a_shell_that_ignores_hangup() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = shell_script(
        &root,
        "ignores-hup",
        "trap '' HUP\necho READY\nwhile :; do sleep 0.05; done",
    );
    app.test_execute(Command::NewShell);
    assert!(
        app.settle(|a| screen_text(a).contains("READY")).await,
        "the shell is running its loop:\n{}",
        screen_text(&app)
    );
    let pid = app.group().active_shell().unwrap().run.pid().unwrap();

    app.test_execute(Command::Quit);
    let at = Instant::now();
    app.shutdown().await;
    let took = at.elapsed();
    assert!(took >= SHUTDOWN_TIMEOUT, "it gave SIGHUP time to work: {took:?}");
    assert!(took < SHUTDOWN_TIMEOUT * 2, "it stayed bounded: {took:?}");
    // SIGKILL was sent before `shutdown` returned: the shell ends now, not at `KILL_GRACE` (1 s).
    let until = Instant::now() + Duration::from_millis(400);
    while Instant::now() < until {
        if !app.groups.shell_tabs().next().unwrap().run.status().is_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    match app.groups.shell_tabs().next().unwrap().run.status() {
        Status::Exited(e) => assert!(
            e.signal.as_deref().is_some_and(|s| s.contains("Kill")),
            "SIGKILL, not SIGHUP: {e:?}"
        ),
        other => panic!("still running after the timeout's SIGKILL: {other:?}"),
    }
    assert!(app.settle(|_| group_gone(pid)).await, "the whole process group is gone");
}

/// Quitting while a shell is still `Starting` (its PTY spawn is off-thread) hangs it up through
/// the `kill_when_started` path: the quit stays bounded and the shell is killed once it starts,
/// not left running.
#[tokio::test(flavor = "multi_thread")]
async fn shell_quit_while_the_shell_is_still_starting() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = shell_script(&root, "slow", "sleep 30");
    app.test_execute(Command::NewShell);
    assert!(
        matches!(app.group().active_shell().unwrap().run.status(), Status::Starting),
        "the shell is still starting (the spawn is off-thread)"
    );

    let at = Instant::now();
    app.test_execute(Command::Quit);
    app.shutdown().await;
    let took = at.elapsed();
    assert!(took < SHUTDOWN_TIMEOUT * 2, "quit stays bounded: {took:?}");
    let ended = app
        .settle(|a| {
            a.groups
                .shell_tabs()
                .all(|s| matches!(s.run.status(), Status::Exited(_)))
        })
        .await;
    assert!(ended, "the shell was killed once it started:\n{}", shell_text(&app));
    match app.groups.shell_tabs().next().unwrap().run.status() {
        Status::Exited(e) => assert!(e.killed, "the late-starting shell was killed: {e:?}"),
        other => panic!("still {other:?} after quit"),
    }
}

/// `JED_IPC` is in the shell program's environment (the listening socket's path), so tools it
/// starts (`git commit`, `jed <file>`) reach this jed.
#[tokio::test(flavor = "multi_thread")]
async fn shell_jed_ipc_is_exported_to_its_program() {
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let record = root.join("ipc.txt");
    let (mut app, socket) = listening_app(&root, &state, &sockets).await;
    app.tasks.shell = shell_script(
        &root,
        "record-ipc",
        &format!("printf '%s' \"$JED_IPC\" > '{}'\nexec /bin/sh", record.display()),
    );
    app.test_execute(Command::NewShell);

    let written = app
        .settle(|_| std::fs::read_to_string(&record).is_ok_and(|s| !s.is_empty()))
        .await;
    assert!(
        written,
        "the program wrote JED_IPC to {}:\n{}",
        record.display(),
        shell_text(&app)
    );
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        socket.to_string_lossy(),
        "the shell's JED_IPC is the socket path, exactly"
    );
    assert_eq!(app.tasks.ipc.as_deref(), Some(socket.as_path()));
    quit_shells(&mut app).await;
}

/// `jed <file>` typed in the shell opens the file in this jed through the socket (the command
/// returns 0 at once), not a second instance; one group, the shell still there.
#[tokio::test(flavor = "multi_thread")]
async fn shell_jed_command_opens_the_file_in_this_jed() {
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let Some(jed) = jed_or_skip("shell_jed_command_opens_the_file_in_this_jed") else {
        return;
    };
    let other = root.join("other.txt");
    std::fs::write(&other, "from the shell\n").unwrap();
    let (mut app, _socket) = listening_app(&root, &state, &sockets).await;
    app.test_execute(Command::NewShell);

    run_in_shell(
        &mut app,
        &format!("'{}' '{}'; echo JED_OPENED=$?", jed.display(), other.display()),
    );
    let opened = app
        .settle(|a| a.active_buffer().is_some_and(|b| b.name() == "other.txt"))
        .await;
    assert!(
        opened,
        "the file opened in this jed, not another:\n{}",
        shell_text(&app)
    );
    assert_eq!(app.active_buffer().unwrap().path, other);
    assert_eq!(app.groups.len(), 1, "the same window: no second group");
    let finished = app.settle(|a| screen_has(a, "JED_OPENED=0")).await;
    assert!(finished, "the command came back from this jed:\n{}", shell_text(&app));
    assert_eq!(app.groups.shell_tabs().count(), 1, "the shell tab stays");
    quit_shells(&mut app).await;
}

/// `git commit` typed in the shell: git's `core.editor` is `jed --wait`, so the message opens in
/// this jed through the socket; saving and closing it lets git finish, and the commit is made.
#[tokio::test(flavor = "multi_thread")]
async fn shell_git_commit_opens_its_message_here() {
    let (_d, root, state) = dirs();
    let sockets = tempfile::tempdir().unwrap();
    let Some(jed) = jed_or_skip("shell_git_commit_opens_its_message_here") else {
        return;
    };
    if git_in(&root, &["--version"]).is_none() {
        eprintln!("skipped shell_git_commit_opens_its_message_here: needs git");
        return;
    }
    assert!(git_in(&root, &["init", "-q"]).is_some(), "git init");
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    assert!(git_in(&root, &["add", "a.txt"]).is_some(), "git add");
    let (mut app, _socket) = listening_app(&root, &state, &sockets).await;
    app.test_execute(Command::NewShell);

    let message = "Made from the shell";
    // `GIT_EDITOR` beats `core.editor`, so unset it; the flags and the config isolation keep the
    // user's git config (templates, hooks, signing) out of the commit.
    let line = format!(
        "unset GIT_EDITOR VISUAL; GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null git \
         -c user.name=t -c user.email=t@t.t -c commit.gpgsign=false \
         -c \"core.editor='{}' --wait\" commit; echo COMMITTED=$?",
        jed.display()
    );
    run_in_shell(&mut app, &line);
    let opened = app
        .settle(|a| a.active_buffer().is_some_and(|b| b.name() == "COMMIT_EDITMSG"))
        .await;
    assert!(
        opened,
        "git's editor opened COMMIT_EDITMSG in this jed:\n{}",
        shell_text(&app)
    );
    assert_eq!(app.active_buffer().unwrap().syntax.language(), Language::GitCommit);

    app.type_text(message);
    app.press(KeyCode::Char('s'), cmd());
    assert!(
        app.settle(|a| a.active_buffer().is_some_and(|b| !b.is_dirty())).await,
        "the message saved"
    );
    app.press(KeyCode::Char('w'), cmd());
    let committed = app.settle(|a| screen_has(a, "COMMITTED=0")).await;
    assert!(committed, "the waiting editor let git commit:\n{}", shell_text(&app));
    assert_eq!(
        git_in(&root, &["log", "-1", "--format=%s"]).as_deref(),
        Some(message),
        "the commit holds the message typed in the tab"
    );
    quit_shells(&mut app).await;
}

/// The shell's PTY follows the editor area (`stty size`): a narrower frame and a split resize the
/// shell's group, and the shell's program sees each size.
#[tokio::test(flavor = "multi_thread")]
async fn shell_stty_size_follows_the_editor_area() {
    let (_d, root, state) = dirs();
    let mut app = shell_app(&root, &state);
    app.tasks.shell = "/bin/sh".into();
    draw(&mut app, 100, 24);
    app.test_execute(Command::NewShell);
    draw(&mut app, 100, 24);
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    let expected = format!("SIZE={} {}", editor.height, editor.width);
    run_in_shell(&mut app, "printf 'SIZE='; stty size");
    assert!(
        app.settle(|a| screen_has(a, &expected)).await,
        "stty size says {expected}:\n{}",
        shell_text(&app)
    );

    // A narrower frame.
    let wide = editor.width;
    draw(&mut app, 90, 24);
    let editor = app.hits.group(app.groups.active).unwrap().editor;
    assert!(editor.width < wide, "the narrower frame shrank the editor");
    let expected = format!("SIZE={} {}", editor.height, editor.width);
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(b"printf 'SIZE='; stty size\n");
    assert!(
        app.settle(|a| screen_has(a, &expected)).await,
        "after a resize stty size says {expected}:\n{}",
        shell_text(&app)
    );

    // A split narrows the shell's group (which is not the active group after it).
    app.test_execute(Command::SplitRight);
    let shell_group = app.groups.ids()[0];
    draw(&mut app, 90, 24);
    let editor = app.hits.group(shell_group).unwrap().editor;
    assert!(editor.width < wide, "the split narrowed the shell's group");
    let expected = format!("SIZE={} {}", editor.height, editor.width);
    app.groups
        .get(shell_group)
        .unwrap()
        .active_shell()
        .unwrap()
        .run
        .write(b"printf 'SIZE='; stty size\n");
    assert!(
        app.settle(|a| screen_has(a, &expected)).await,
        "after a split stty size says {expected}:\n{}",
        shell_text(&app)
    );
    quit_shells(&mut app).await;
}

/// A `.workspace` `scrollback = 50` reaches the runs: a shell opened from it and a task run from
/// it each keep exactly the newest 50 lines above the screen (not just `app.settings` set by
/// hand).
#[tokio::test(flavor = "multi_thread")]
async fn shell_workspace_scrollback_setting() {
    let (_d, root, state) = dirs();
    std::fs::write(root.join(".workspace"), "[settings]\nscrollback = 50\n").unwrap();
    let mut app = shell_app(&root, &state);
    assert_eq!(app.settings.scrollback, 50, "the .workspace's setting is read");
    app.tasks.shell = "/bin/sh".into();
    app.test_execute(Command::NewShell);
    app.group()
        .active_shell()
        .unwrap()
        .run
        .write(format!("{COUNT_TO_200}\n").as_bytes());
    assert!(
        app.settle(|a| screen_has(a, "199")).await,
        "the shell ran the command:\n{}",
        shell_text(&app)
    );
    {
        let mut screen = app.groups.shell_tabs().next().unwrap().run.screen();
        screen.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(screen.screen().scrollback(), 50, "a shell keeps the setting's lines");
        // 200 lines were printed: a window of the newest 50 starts well past line 100, the oldest
        // 50 at line 0 (the line numbers include the echoed command line).
        let first: usize = screen.screen().contents().lines().next().unwrap().parse().unwrap();
        assert!(first >= 100, "the newest 50 lines, not the oldest (first line {first})");
    }

    // The same setting for a task run started after the workspace was loaded.
    app.tasks.size = (2, 20);
    let template = Template {
        name: "count".into(),
        task: Task {
            command: COUNT_TO_200.into(),
            cwd: None,
            env: BTreeMap::new(),
        },
        source: Source::User,
    };
    let id = app.start_task(&template);
    let ended = app
        .settle(|a| a.tasks.get(id).is_some_and(|r| !r.status().is_running()))
        .await;
    assert!(ended, "the task ended");
    {
        let mut screen = app.tasks.get(id).unwrap().screen();
        screen.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(screen.screen().scrollback(), 50, "a task run keeps the setting's lines");
        let first: usize = screen.screen().contents().lines().next().unwrap().parse().unwrap();
        assert!(first >= 100, "the newest 50 lines, not the oldest (first line {first})");
    }
    quit_shells(&mut app).await;
}
