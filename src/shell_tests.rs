//! End-to-end tests for shell-a: "Terminal: New Shell" opens the user's shell in a tab of the
//! active group (`app.group().active_shell()`), as a terminal emulator starts it — a PTY in the
//! workspace root at the group's editor size, with `TERM` / `COLORTERM` and `JED_IPC` only when
//! the socket listens — titled per program (`sh`, `sh (2)`, reusing a closed number), keeping
//! the `scrollback` setting's lines, never in a session, and with no tab of its own in a split.

use std::collections::BTreeMap;
use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::App;
use crate::buffer::Buffer;
use crate::command::Command;
use crate::tasks::{Source, Status, Task, Template};
use crate::workspace::Workspace;
use crate::workspace::trust::TrustStore;

/// A command printing 0 to 199, one per line, in the shell (`seq` is missing on macOS).
const COUNT_TO_200: &str = "i=0; while [ $i -lt 200 ]; do echo $i; i=$((i+1)); done";

/// A workspace app on `root`, its trust store in `state`.
fn shell_app(root: &Path, state: &Path) -> App {
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.test_start();
    app
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
