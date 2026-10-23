//! End-to-end tests for the shell task: shell-a's "Terminal: New Shell" opens the user's shell
//! in a tab of the active group (`app.group().active_shell()`), as a terminal emulator starts it
//! — a PTY in the workspace root at the group's editor size, with `TERM` / `COLORTERM` and
//! `JED_IPC` only when the socket listens — titled per program (`sh`, `sh (2)`, reusing a closed
//! number), keeping the `scrollback` setting's lines, never in a session, and with no tab of its
//! own in a split. shell-b: the screen drawn in the editor area at its size, in the terminal's
//! colours with its cursor only when focused and live, keys and pastes through `tasks::keys`
//! (types, Enter, arrows in the shell's cursor mode, Esc, Ctrl+C, bracketed paste; Cmd+W and
//! global keys stay jed's), Shift+PageUp / PageDown / Ctrl+Home / End and the wheel through the
//! scrollback, exit notes kept on screen and in the status bar (success, exit code, failed
//! start), a closed tab killing the shell, and quitting hanging every shell up (SIGKILL when
//! SIGHUP is ignored).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event as CEvent, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{App, Focus, Level};
use crate::buffer::Buffer;
use crate::command::Command;
use crate::tasks::{SHUTDOWN_TIMEOUT, Source, Status, Task, Template};
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// A command printing 0 to 199, one per line, in the shell (`seq` is missing on macOS).
const COUNT_TO_200: &str = "i=0; while [ $i -lt 200 ]; do echo $i; i=$((i+1)); done";

/// The platform's Cmd (Super on macOS, Ctrl on Linux).
fn cmd() -> KeyModifiers {
    if cfg!(target_os = "macos") {
        KeyModifiers::SUPER
    } else {
        KeyModifiers::CONTROL
    }
}

/// A workspace app on `root`, its trust store in `state`.
fn shell_app(root: &Path, state: &Path) -> App {
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
fn dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
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
    assert_eq!(tab.title, "myshell", "named after the program, not a hardcoded shell");
    assert_eq!(tab.base, "myshell");
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
    assert_eq!(app.group().active_shell().unwrap().title, "myshell (2)");
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
    assert_eq!(titles, ["sh", "sh (2)", "sh (3)"]);

    // Close the middle one ("sh (2)").
    app.group_mut().active = 1;
    app.test_execute(Command::CloseTab);
    let mut titles: Vec<String> = app.groups.shell_tabs().map(|s| s.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, ["sh", "sh (3)"]);

    app.test_execute(Command::NewShell);
    assert_eq!(app.group().active_shell().unwrap().title, "sh (2)", "the free number");
    let mut titles: Vec<String> = app.groups.shell_tabs().map(|s| s.title.clone()).collect();
    titles.sort();
    assert_eq!(titles, ["sh", "sh (2)", "sh (3)"], "no repeated title");
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
    assert_eq!(app.groups.get(first).unwrap().active_shell().unwrap().title, "sh");
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
fn draw(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
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
fn screen_text(app: &App) -> String {
    app.group().active_shell().unwrap().run.screen().screen().contents()
}

/// The active shell's scrollback position (0 = the live screen).
fn scrollback(app: &App) -> usize {
    app.group().active_shell().unwrap().run.screen().screen().scrollback()
}

/// The shell process group is gone (`kill(-pgid, 0)` fails).
fn group_gone(pid: u32) -> bool {
    // SAFETY: signal 0 checks for the group's existence and sends nothing.
    unsafe { libc::kill(-(pid as i32), 0) != 0 }
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
