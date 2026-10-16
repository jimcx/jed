//! Milestone 12 end to end (tasks). 12a, the `tasks` module without UI: templates come from
//! `.workspace` `[tasks]` and the user's `tasks.toml`, the workspace's winning a name both define;
//! an undecided or restricted workspace contributes none (the user's still run), a trusted one
//! all (`milestone_12_templates_follow_workspace_trust`, the real `TrustStore`); `tasks.toml`
//! errors are reported as `main` reports them and bad tasks skipped; a task run from the app
//! runs in a PTY through the user's shell in its resolved cwd with its env, its colours land in
//! the vt100 screen's cells, its exit code is kept, and one that can't start says why
//! (`milestone_12_a_task_runs_in_a_pty`); quitting (and dropping the app) kills running tasks
//! and everything they started (`milestone_12_quit_kills_running_tasks`).

use std::fs;
use std::path::Path;

use crate::app::{App, Level};
use crate::tasks::{self, Source, Status};
use crate::workspace::trust::{Trust, TrustStore};
use crate::workspace::{self, Workspace};

/// The app on `root` with the trust store in `state`, the user's `tasks.toml` loaded (and its
/// errors reported) as `main` does.
fn open_app(root: &Path, state: &Path, tasks_toml: Option<&str>) -> App {
    let config_dir = state.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    let path = config_dir.join(tasks::templates::FILE_NAME);
    if let Some(text) = tasks_toml {
        fs::write(&path, text).unwrap();
    }
    let user = tasks::templates::load(&path);
    let ws = Workspace::load(root.to_path_buf());
    let trust = TrustStore::load(state.join("trust.toml"));
    let mut app = App::new(ws, trust, None, true);
    app.config_dir = config_dir;
    app.tasks.user = user.tasks;
    app.report_errors(user.errors);
    app.test_start();
    app
}

/// `proj/` (the workspace root) and `state/` (trust store, config) in a tempdir.
fn dirs(dir: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let base = workspace::normalize(dir.path());
    let (root, state) = (base.join("proj"), base.join("state"));
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&state).unwrap();
    (root, state)
}

fn templates(app: &App) -> Vec<(String, String, Source)> {
    app.task_templates()
        .into_iter()
        .map(|t| (t.name, t.task.command, t.source))
        .collect()
}

fn t(name: &str, command: &str, source: Source) -> (String, String, Source) {
    (name.into(), command.into(), source)
}

/// Whether any process is in group `pgid`.
fn group_alive(pgid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc::kill(-(pgid as i32), 0) == 0 }
}

async fn wait_gone(pgid: u32) -> bool {
    for _ in 0..500 {
        if !group_alive(pgid) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}

const WORKSPACE: &str = "[tasks.build]\ncommand = \"echo workspace build\"\n\n[tasks.lint]\ncommand = \"echo lint\"\n";
const USER: &str = "[tasks.build]\ncommand = \"echo user build\"\n\n[tasks.clean]\ncommand = \"echo clean\"\n";

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_templates_follow_workspace_trust() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join(".workspace"), WORKSPACE).unwrap();

    // Undecided (the trust dialog is up): the user's tasks only.
    let mut app = open_app(&root, &state, Some(USER));
    assert_eq!(app.trust, Trust::Undecided);
    let user_only = vec![
        t("build", "echo user build", Source::User),
        t("clean", "echo clean", Source::User),
    ];
    assert_eq!(templates(&app), user_only);
    assert_eq!(app.run_task("lint"), None);
    assert_eq!(app.message.as_ref().unwrap().text, "No task named 'lint'");
    assert!(app.tasks.runs().is_empty(), "nothing started");

    // Trusted: both, the workspace's build winning.
    app.press(
        crossterm::event::KeyCode::Char('t'),
        crossterm::event::KeyModifiers::NONE,
    );
    assert_eq!(app.trust, Trust::Trusted);
    assert_eq!(
        templates(&app),
        vec![
            t("build", "echo workspace build", Source::Workspace),
            t("clean", "echo clean", Source::User),
            t("lint", "echo lint", Source::Workspace),
        ]
    );
    let id = app.run_task("build").unwrap();
    assert!(app.settle(|a| !a.tasks.get(id).unwrap().status().is_running()).await);
    assert_eq!(
        app.tasks.get(id).unwrap().screen().screen().contents(),
        "workspace build"
    );
    assert!(app.settle(|_| state.join("trust.toml").exists()).await);
    drop(app);

    // Restricted (stored as such by the real trust store): the user's only; their build runs.
    let mut store = TrustStore::load(state.join("trust.toml"));
    store.record(&root, &Workspace::load(root.clone()).config, false);
    store.write().unwrap();
    let mut app = open_app(&root, &state, Some(USER));
    assert_eq!(app.trust, Trust::Restricted);
    assert!(app.prompt.is_none());
    assert_eq!(templates(&app), user_only);
    assert_eq!(app.run_task("lint"), None);
    let id = app.run_task("build").unwrap();
    assert!(app.settle(|a| !a.tasks.get(id).unwrap().status().is_running()).await);
    assert_eq!(app.tasks.get(id).unwrap().screen().screen().contents(), "user build");

    // No `.workspace` commands at all: nothing to trust, the user's tasks.
    let dir2 = tempfile::tempdir().unwrap();
    let (root2, state2) = dirs(&dir2);
    let app = open_app(&root2, &state2, Some(USER));
    assert_eq!(app.trust, Trust::NotNeeded);
    assert_eq!(templates(&app), user_only);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_tasks_toml_errors_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let app = open_app(
        &root,
        &state,
        Some("[tasks.ok]\ncommand = \"true\"\n\n[tasks.bad]\ncomand = \"x\"\n\n[tasks.empty]\ncommand = \"\"\n"),
    );
    let message = app.message.as_ref().unwrap();
    assert_eq!(message.level, Level::Error);
    assert!(
        message
            .text
            .starts_with("tasks.toml: [tasks.bad]: unknown field `comand`"),
        "{}",
        message.text
    );
    assert!(message.text.ends_with(" (and 1 more)"), "{}", message.text);
    assert_eq!(templates(&app), vec![t("ok", "true", Source::User)]);

    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let app = open_app(&root, &state, Some("[tasks.a\n"));
    assert!(app.message.as_ref().unwrap().text.starts_with("tasks.toml: line 1:"));
    assert!(app.task_templates().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_a_task_runs_in_a_pty() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::create_dir(root.join("sub")).unwrap();
    let mut app = open_app(
        &root,
        &state,
        Some(
            r#"
[tasks.colours]
command = '''printf '\033[1;32mok\033[0m %s %s\n' "$PWD" "$TERM"; printf '\033[38;2;255;128;0mrgb\033[0m %s\n' "$NAME"; exit 2'''
cwd = "sub"
env = { NAME = "tasks" }

[tasks.nowhere]
command = "true"
cwd = "missing"
"#,
        ),
    );
    assert!(app.message.is_none(), "{:?}", app.message.as_ref().map(|m| &m.text));
    let id = app.run_task("colours").unwrap();
    assert_eq!(
        app.tasks.get(id).unwrap().status(),
        Status::Starting,
        "spawned off the UI thread"
    );
    assert!(app.settle(|a| !a.tasks.get(id).unwrap().status().is_running()).await);
    let run = app.tasks.get(id).unwrap();
    assert!(
        run.name == "colours" && run.command.starts_with("printf '\\033[1;32m"),
        "{}",
        run.command
    );
    match run.status() {
        Status::Exited(e) => assert!(e.code == 2 && e.signal.is_none() && !e.killed, "{e:?}"),
        s => panic!("{s:?}"),
    }
    {
        let parser = run.screen();
        let screen = parser.screen();
        assert_eq!(
            screen.contents(),
            format!("ok {} xterm-256color\nrgb tasks", root.join("sub").display())
        );
        let ok = screen.cell(0, 0).unwrap();
        assert_eq!(ok.fgcolor(), vt100::Color::Idx(2));
        assert!(ok.bold());
        let plain = screen.cell(0, 3).unwrap();
        assert_eq!(plain.fgcolor(), vt100::Color::Default);
        assert!(!plain.bold());
        assert_eq!(screen.cell(1, 0).unwrap().fgcolor(), vt100::Color::Rgb(255, 128, 0));
    }

    let id = app.run_task("nowhere").unwrap();
    assert!(
        app.settle(|a| a
            .message
            .as_ref()
            .is_some_and(|m| m.text.starts_with("Task 'nowhere' failed to start:")))
            .await
    );
    assert!(matches!(app.tasks.get(id).unwrap().status(), Status::Failed(m) if m.contains("missing")));
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_quit_kills_running_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(
        &root,
        &state,
        Some(
            "[tasks.serve]\ncommand = \"sleep 30 & sleep 30\"\n\n[tasks.stubborn]\ncommand = \"trap '' HUP; echo ready; sleep 30\"\n",
        ),
    );
    let serve = app.run_task("serve").unwrap();
    let stubborn = app.run_task("stubborn").unwrap();
    assert!(
        app.settle(|a| a.tasks.runs().iter().all(|r| r.status() == Status::Running)
            && a.tasks.get(stubborn).unwrap().screen().screen().contents() == "ready")
            .await
    );
    let pids: Vec<u32> = [serve, stubborn]
        .iter()
        .map(|&id| app.tasks.get(id).unwrap().pid().unwrap())
        .collect();
    app.test_execute(crate::command::Command::Quit);
    assert!(app.quitting());
    // What `run` does on quit: SIGHUP, then SIGKILL for the one ignoring it.
    let at = std::time::Instant::now();
    app.shutdown().await;
    let took = at.elapsed();
    assert!(
        took >= tasks::SHUTDOWN_TIMEOUT && took < tasks::SHUTDOWN_TIMEOUT * 2,
        "{took:?}"
    );
    for pid in &pids {
        assert!(wait_gone(*pid).await, "group {pid} still alive");
    }
    assert!(app.settle(|a| a.tasks.running() == 0).await);
    for id in [serve, stubborn] {
        assert!(matches!(app.tasks.get(id).unwrap().status(), Status::Exited(e) if e.killed));
    }

    // Dropping the app (a panic, the terminal going away) kills them too.
    let mut app = open_app(
        &root,
        &state,
        Some("[tasks.serve]\ncommand = \"sleep 30 & sleep 30\"\n"),
    );
    let id = app.run_task("serve").unwrap();
    assert!(app.settle(|a| a.tasks.get(id).unwrap().pid().is_some()).await);
    let pid = app.tasks.get(id).unwrap().pid().unwrap();
    drop(app);
    assert!(wait_gone(pid).await);
}
