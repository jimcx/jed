//! Milestone 12 end to end (tasks). 12a, the `tasks` module without UI: templates come from
//! `.workspace` `[tasks]` and the user's `tasks.toml`, the workspace's winning a name both define;
//! an undecided or restricted workspace contributes none (the user's still run), a trusted one
//! all (`milestone_12_templates_follow_workspace_trust`, the real `TrustStore`); `tasks.toml`
//! errors are reported as `main` reports them and bad tasks skipped; a task run from the app
//! runs in a PTY through the user's shell in its resolved cwd with its env, its colours land in
//! the vt100 screen's cells, its exit code is kept, and one that can't start says why
//! (`milestone_12_a_task_runs_in_a_pty`); quitting (and dropping the app) kills running tasks
//! and everything they started (`milestone_12_quit_kills_running_tasks`).
//!
//! 12b, tasks in the UI: "Tasks: Run Task" opens a picker of templates (name, command, where it's
//! defined) and runs the pick in a new, focused panel tab drawn in Dark+'s terminal colours, its
//! tab showing its state and exit code, the end note kept below the output
//! (`milestone_12_run_task_shows_a_coloured_tab_and_the_exit_code`); no templates → a message
//! saying where to define them, restricted mode offering and running no `.workspace` task
//! (`milestone_12_run_task_without_templates_says_where_to_define_them`); unbound keys and pastes
//! go to the focused task, global keys and Esc stay red's
//! (`milestone_12_keys_and_pastes_go_to_the_focused_task`), encoded for the task's terminal modes
//! (application cursor keys, bracketed paste), bringing the live screen back, never sent without
//! the keys (`milestone_12_keys_follow_the_task_terminal_modes`); the PTY takes the panel's size
//! (`milestone_12_the_pty_follows_the_panel_size`), resized only when it changes
//! (`milestone_12_frames_resize_the_pty_only_when_the_size_changes`); Shift+PageUp, Ctrl+Home / End and the wheel
//! scroll the output (`milestone_12_scrolling_through_a_task_output`); a tab per run, clicks on
//! tabs, kill, and closing (Cmd+W, `×`, a middle click, the palette) kills a running task
//! (`milestone_12_kill_and_close_task_tabs`); keys typed while a run is still starting are
//! queued and reach it (`milestone_12_keys_typed_while_a_task_starts_reach_it`).
//!
//! 12c, the done-when: "a colourful build (e.g. `cargo build`) renders as in a real terminal" —
//! a real `cargo build --color=always` of a small crate with a warning and a slow build script
//! (cargo's progress bar), run from "Run Task", draws the same text and colours as an
//! independent model of a terminal (`reference`) fed cargo's coloured output from another build,
//! the progress bar gone, "Compiling" / "Finished" bright green, "warning" bold yellow drawn
//! bright, the exit code below (`milestone_12_done_when_a_colourful_build_renders_as_in_a_terminal`;
//! skipped with a note without `cargo`); a script's `\r`-overwritten progress line, `ESC[K` and
//! cursor moves end as a terminal shows them, checked against the same model and spelled out
//! (`milestone_12_done_when_progress_output_renders_as_in_a_terminal`). "Restricted mode runs no
//! `.workspace` task": undecided, restricted from the dialog and restricted as stored, the
//! picker doesn't list it, `run_task` refuses, no run starts and the marker file it would write
//! never appears, while the user's `tasks.toml` tasks run and a name both define runs the
//! user's; once trusted the workspace's runs
//! (`milestone_12_done_when_restricted_mode_runs_no_workspace_task`).

use std::fs;
use std::path::Path;

use crate::app::{App, Level};
use crate::tasks::{self, RunId, Source, Status};
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

/// Settles until run `id` ended and the app wrote its end note below its output (it handled
/// the run's `Ended`).
async fn settle_ended(app: &mut App, id: RunId) -> bool {
    app.settle(|a| {
        let run = a.tasks.get(id).unwrap();
        let status = run.status();
        tasks::end_note(&run.name, &status).is_some_and(|n| run.screen().screen().contents().ends_with(&n))
    })
    .await
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
    assert!(settle_ended(&mut app, id).await);
    assert_eq!(
        app.tasks.get(id).unwrap().screen().screen().contents(),
        "workspace build\nTask 'build' exited with code 0"
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
    assert!(settle_ended(&mut app, id).await);
    assert_eq!(
        app.tasks.get(id).unwrap().screen().screen().contents(),
        "user build\nTask 'build' exited with code 0"
    );

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
    assert!(settle_ended(&mut app, id).await);
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
            format!(
                "ok {} xterm-256color\nrgb tasks\nTask 'colours' exited with code 2",
                root.join("sub").display()
            )
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

// 12b: the "Run Task" picker, task tabs in the bottom panel.

use crossterm::event::{Event, KeyCode, KeyModifiers as M, MouseButton, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::app::Focus;
use crate::command::Command;
use crate::layout::PanelTab;

fn cmd() -> M {
    if cfg!(target_os = "macos") {
        M::SUPER
    } else {
        M::CONTROL
    }
}

fn draw_sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::render(f, app)).unwrap();
    term
}

fn lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// Where `needle` starts on screen (the first place), as cell coordinates.
fn find(term: &Terminal<TestBackend>, needle: &str) -> Option<(u16, u16)> {
    let n: Vec<char> = needle.chars().collect();
    lines(term).iter().enumerate().find_map(|(y, row)| {
        let chars: Vec<char> = row.chars().collect();
        (0..chars.len().saturating_sub(n.len() - 1))
            .find(|&x| chars[x..x + n.len()] == n[..])
            .map(|x| (x as u16, y as u16))
    })
}

fn fg(term: &Terminal<TestBackend>, (x, y): (u16, u16)) -> Color {
    term.backend().buffer()[(x, y)].fg
}

fn bg(term: &Terminal<TestBackend>, (x, y): (u16, u16)) -> Color {
    term.backend().buffer()[(x, y)].bg
}

/// The task tabs drawn in the last frame, in order.
fn task_tabs(app: &App) -> Vec<RunId> {
    app.hits
        .panel_tabs
        .iter()
        .filter_map(|(_, t)| match t {
            PanelTab::Task(id) => Some(*id),
            _ => None,
        })
        .collect()
}

/// "Tasks: Run Task" through the command palette.
fn open_run_task(app: &mut App) {
    app.press(KeyCode::Char('p'), cmd() | M::SHIFT);
    app.type_text("Tasks: Run Task");
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, "Tasks: Run Task");
    app.press(KeyCode::Enter, M::NONE);
}

/// Runs the task named `name` from the "Run Task" picker.
fn pick_task(app: &mut App, name: &str) -> RunId {
    open_run_task(app);
    app.type_text(name);
    let p = app.palette.as_ref().unwrap();
    assert_eq!(p.rows[p.selected].label, name);
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.palette.is_none());
    app.tasks.runs().last().unwrap().id
}

fn screen_text(app: &App, id: RunId) -> String {
    app.tasks.get(id).unwrap().screen().screen().contents()
}

const TASKS: &str = r#"
[tasks.colours]
command = '''printf '\033[1;32mok\033[0m plain \033[31mred\033[0m\n'; printf '\033[38;2;255;128;0mrgb\033[0m\n'; exit 2'''

[tasks.ask]
command = '''printf 'continue? '; read a; echo "got $a"; sleep 30'''

[tasks.size]
command = '''echo ready; read x; stty size'''

[tasks.serve]
command = "echo serving; sleep 30"

[tasks.count]
command = "seq 1 100"

[tasks.modes]
command = '''seq 1 50; printf '\033[?1h\033[?2004h'; echo ready; read -r a; read -r b; printf '%s|' "$a" "$b" | od -An -tx1'''

[tasks.winch]
# zsh runs no WINCH trap: counted in sh.
command = '''exec /bin/sh -c 'n=0; trap "n=\$((n+1))" WINCH; echo ready; while ! read -r x; do :; done; echo "winch $n"''''
"#;

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_run_task_shows_a_coloured_tab_and_the_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join(".workspace"), "[tasks.build]\ncommand = \"true\"\n").unwrap();
    let mut app = open_app(&root, &state, Some(TASKS));
    // Trust it: the picker lists both files' tasks, each with where it's defined.
    app.press(KeyCode::Char('t'), M::NONE);
    assert_eq!(app.trust, Trust::Trusted);

    open_run_task(&mut app);
    let p = app.palette.as_ref().unwrap();
    let rows: Vec<(&str, &str, Option<&str>)> = p
        .rows
        .iter()
        .map(|r| (r.label.as_str(), r.detail.as_str(), r.keys.as_deref()))
        .collect();
    assert_eq!(rows.len(), 8, "{rows:?}");
    assert_eq!(
        rows[0],
        (
            "ask",
            r#"printf 'continue? '; read a; echo "got $a"; sleep 30"#,
            Some("tasks.toml")
        )
    );
    assert_eq!(rows[1], ("build", "true", Some(".workspace")));
    assert_eq!(p.hint, None);
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.palette.is_none() && app.tasks.runs().is_empty());

    let id = pick_task(&mut app, "colours");
    assert!(app.ui.panel_visible);
    assert_eq!((app.ui.panel_tab, app.ui.focus), (PanelTab::Task(id), Focus::Panel));
    assert_eq!(
        app.last_command,
        Some(Command::Palette(crate::command::PaletteCommand::Accept))
    );
    assert!(settle_ended(&mut app, id).await);
    let message = app.message.as_ref().unwrap();
    assert_eq!(
        (message.level, message.text.as_str()),
        (Level::Warning, "Task 'colours' exited with code 2")
    );

    let term = draw_sized(&mut app, 100, 30);
    // The tab: its name and exit code, a close button.
    let tab = find(&term, "colours ✗ 2 ×").expect("tab title");
    assert_eq!(task_tabs(&app), vec![id]);
    assert_eq!(fg(&term, (tab.0 + 8, tab.1)), app.theme.get("task.failed").fg.unwrap());
    // The output in Dark+'s terminal colours: bold green drawn bright, red, an RGB colour.
    let ok = find(&term, "ok plain red").expect("output");
    assert!(ok.1 > tab.1);
    let key_fg = |k: &str| app.theme.get(k).fg.unwrap();
    assert_eq!(fg(&term, ok), key_fg("terminal.ansiBrightGreen"));
    assert_eq!(fg(&term, (ok.0 + 3, ok.1)), key_fg("terminal.foreground"));
    assert_eq!(fg(&term, (ok.0 + 9, ok.1)), key_fg("terminal.ansiRed"));
    let mut rgb = ratatui::buffer::Cell::default();
    rgb.fg = Color::Rgb(255, 128, 0);
    app.theme.terminal_cell(&mut rgb);
    assert_eq!(fg(&term, (ok.0, ok.1 + 1)), rgb.fg);
    let below: String = lines(&term)[ok.1 as usize + 1].chars().skip(ok.0 as usize).collect();
    assert_eq!(below.trim_end(), "rgb");
    let terminal_bg = app.theme.get("terminal.background").bg.unwrap();
    assert_eq!(bg(&term, (ok.0 + 20, ok.1)), terminal_bg);
    // The end note below the output; the command at the right of the tab strip.
    assert_eq!(
        find(&term, "Task 'colours' exited with code 2").map(|p| p.1),
        Some(ok.1 + 2)
    );
    assert!(find(&term, "printf ").is_some_and(|p| p.1 == tab.1));

    // A successful run's tab: ✓.
    let id2 = pick_task(&mut app, "build");
    assert!(settle_ended(&mut app, id2).await);
    let term = draw_sized(&mut app, 100, 30);
    assert!(find(&term, "build ✓ ×").is_some());
    assert_eq!(task_tabs(&app), vec![id, id2]);
    assert_eq!(app.message.as_ref().unwrap().level, Level::Info);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_run_task_without_templates_says_where_to_define_them() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, None);
    app.test_execute(Command::RunTask);
    assert!(app.palette.is_none());
    let text = &app.message.as_ref().unwrap().text;
    assert!(text.starts_with("No tasks defined: add [tasks.<name>]"), "{text}");
    assert!(
        text.contains(".workspace") && text.contains(&*state.join("config/tasks.toml").to_string_lossy()),
        "{text}"
    );

    // Restricted mode: `.workspace`'s tasks aren't offered, and the message says why.
    fs::write(root.join(".workspace"), WORKSPACE).unwrap();
    let mut store = TrustStore::load(state.join("trust.toml"));
    store.record(&root, &Workspace::load(root.clone()).config, false);
    store.write().unwrap();
    let mut app = open_app(&root, &state, None);
    assert_eq!(app.trust, Trust::Restricted);
    app.test_execute(Command::RunTask);
    assert!(app.palette.is_none());
    let text = &app.message.as_ref().unwrap().text;
    assert!(
        text.contains("2 tasks in .workspace not available in restricted mode (Manage Workspace Trust)"),
        "{text}"
    );
    assert!(app.tasks.runs().is_empty());

    // With the user's tasks: the picker lists only those, its hint saying why.
    let mut app = open_app(&root, &state, Some(USER));
    open_run_task(&mut app);
    let p = app.palette.as_ref().unwrap();
    let labels: Vec<&str> = p.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["build", "clean"]);
    assert!(p.rows.iter().all(|r| r.keys.as_deref() == Some("tasks.toml")));
    assert!(
        p.hint.as_deref().is_some_and(|h| h.contains("restricted mode")),
        "{:?}",
        p.hint
    );
    app.type_text("lint");
    assert!(app.palette.as_ref().unwrap().rows.is_empty());
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.tasks.runs().is_empty(), "restricted mode runs no .workspace task");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_keys_and_pastes_go_to_the_focused_task() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, Some(TASKS));
    let id = pick_task(&mut app, "ask");
    assert!(app.settle(|a| screen_text(a, id).contains("continue?")).await);
    // Plain keys and Enter reach the task (not the editor or the palette).
    app.type_text("y");
    app.press(KeyCode::Enter, M::NONE);
    assert!(app.settle(|a| screen_text(a, id).contains("got y")).await);
    let term = draw_sized(&mut app, 100, 30);
    assert!(find(&term, "ask ● ×").is_some(), "running");
    // Ctrl+C goes to the task (an interrupt), not to red's copy.
    app.press(KeyCode::Char('c'), M::CONTROL);
    assert!(settle_ended(&mut app, id).await);
    match app.tasks.get(id).unwrap().status() {
        Status::Exited(e) => assert!(!e.killed && !e.success(), "{e:?}"),
        s => panic!("{s:?}"),
    }
    // Keys to an ended task do nothing.
    app.type_text("z");
    assert!(!screen_text(&app, id).contains('z'));

    // A paste, bracketed or not, as typed input.
    let id = pick_task(&mut app, "size");
    assert!(app.settle(|a| screen_text(a, id).contains("ready")).await);
    app.handle_test_event(Event::Paste("\n".into()));
    assert!(settle_ended(&mut app, id).await);

    // Global keys still work in a task tab; Esc gives the keys back.
    app.press(KeyCode::Char('p'), cmd() | M::SHIFT);
    assert!(app.palette.is_some());
    app.press(KeyCode::Esc, M::NONE);
    assert!(app.palette.is_none());
    assert_eq!(app.ui.focus, Focus::Panel);
    app.press(KeyCode::Esc, M::NONE);
    assert_ne!(app.ui.focus, Focus::Panel);
    assert!(app.ui.panel_visible, "the panel stays");
}

/// A task that asks for application cursor keys and bracketed paste gets them: Up as `ESC O A`,
/// a paste wrapped in `ESC [200~` … `ESC [201~` (its line break sent as Enter).
#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_keys_follow_the_task_terminal_modes() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, Some(TASKS));
    let id = pick_task(&mut app, "modes");
    assert!(app.settle(|a| screen_text(a, id).contains("ready")).await);
    // Scrolled back, a key brings the live screen back.
    app.press(KeyCode::PageUp, M::SHIFT);
    let back = |app: &App| app.tasks.get(id).unwrap().screen().screen().scrollback();
    assert!(back(&app) > 0);
    app.press(KeyCode::Up, M::NONE);
    assert_eq!(back(&app), 0);
    app.press(KeyCode::Enter, M::NONE);
    // Without the keys (Esc), a paste isn't the task's; a click in its terminal gives them back.
    app.press(KeyCode::Esc, M::NONE);
    app.handle_test_event(Event::Paste("zz".into()));
    draw_sized(&mut app, 100, 30);
    let panel = app.hits.regions.panel.unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), panel.x + 5, panel.y + 4);
    assert_eq!(app.ui.focus, Focus::Panel);
    app.press(KeyCode::PageUp, M::SHIFT);
    app.handle_test_event(Event::Paste("hi\n".into()));
    assert_eq!(back(&app), 0, "a paste brings the live screen back");
    assert!(settle_ended(&mut app, id).await);
    let text = screen_text(&app, id);
    assert!(text.contains("1b 4f 41 7c 1b 5b 32 30 30 7e 68 69 7c"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_the_pty_follows_the_panel_size() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, Some(TASKS));
    let id = pick_task(&mut app, "size");
    assert!(app.settle(|a| screen_text(a, id).contains("ready")).await);
    draw_sized(&mut app, 100, 30);
    let list = app.hits.regions.panel.unwrap();
    // Below the border and the tab strip, one column of padding.
    let (rows, cols) = (list.height - 2, list.width - 1);
    assert_eq!(app.tasks.size, (rows, cols));
    assert_eq!(app.tasks.get(id).unwrap().screen().screen().size(), (rows, cols));
    // A taller panel, a narrower terminal: the process sees the new size.
    app.ui.panel_height += 4;
    draw_sized(&mut app, 90, 30);
    let list = app.hits.regions.panel.unwrap();
    let size = (list.height - 2, list.width - 1);
    assert_eq!(size, (rows + 4, cols - 10));
    app.press(KeyCode::Enter, M::NONE);
    assert!(settle_ended(&mut app, id).await);
    assert!(
        screen_text(&app, id).contains(&format!("\n{} {}\n", size.0, size.1)),
        "{}",
        screen_text(&app, id)
    );
    // New runs start at the panel's size.
    let id = pick_task(&mut app, "serve");
    assert_eq!(app.tasks.get(id).unwrap().screen().screen().size(), size);
}

/// Frames at an unchanged size don't resize the PTY again: the task sees one SIGWINCH per
/// change, not one per frame.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_frames_resize_the_pty_only_when_the_size_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, Some(TASKS));
    draw_sized(&mut app, 100, 30);
    let id = pick_task(&mut app, "winch");
    assert!(app.settle(|a| screen_text(a, id).contains("ready")).await);
    for _ in 0..5 {
        draw_sized(&mut app, 100, 30);
    }
    draw_sized(&mut app, 90, 30);
    draw_sized(&mut app, 90, 30);
    app.press(KeyCode::Enter, M::NONE);
    assert!(settle_ended(&mut app, id).await);
    let text = screen_text(&app, id);
    assert!(text.contains("winch 1\n"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_scrolling_through_a_task_output() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, Some(TASKS));
    // A frame first, as the app always has: the run starts at its tab's size.
    draw_sized(&mut app, 100, 30);
    let id = pick_task(&mut app, "count");
    assert!(settle_ended(&mut app, id).await);
    let term = draw_sized(&mut app, 100, 30);
    let rows = app.tasks.size.0 as usize;
    // The live screen: the last lines and the end note.
    assert!(find(&term, "Task 'count' exited with code 0").is_some());
    let first = |app: &App| screen_text(app, id).lines().next().unwrap().to_string();
    assert_eq!(first(&app), (100 - rows + 2).to_string());

    app.press(KeyCode::PageUp, M::SHIFT);
    let back = app.tasks.get(id).unwrap().screen().screen().scrollback();
    assert_eq!(back, rows - 1);
    let term = draw_sized(&mut app, 100, 30);
    assert!(find(&term, &format!("{back} lines back")).is_some());
    assert_eq!(first(&app), (100 - rows + 2 - back).to_string());

    // The wheel over the panel scrolls it too.
    let panel = app.hits.regions.panel.unwrap();
    app.mouse(MouseEventKind::ScrollUp, panel.x + 5, panel.y + 4);
    assert_eq!(app.tasks.get(id).unwrap().screen().screen().scrollback(), back + 3);
    app.press(KeyCode::Home, M::CONTROL);
    assert_eq!(first(&app), "1");
    app.mouse(MouseEventKind::ScrollDown, panel.x + 5, panel.y + 4);
    assert_eq!(first(&app), "4");
    app.press(KeyCode::End, M::CONTROL);
    assert_eq!(app.tasks.get(id).unwrap().screen().screen().scrollback(), 0);
    // From the palette, on the shown tab.
    app.test_execute(Command::Task(crate::command::TaskCommand::ScrollUp));
    assert_eq!(app.tasks.get(id).unwrap().screen().screen().scrollback(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_kill_and_close_task_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, Some(TASKS));
    let a = pick_task(&mut app, "serve");
    let b = pick_task(&mut app, "serve");
    let c = pick_task(&mut app, "serve");
    assert!(
        app.settle(|a| a
            .tasks
            .runs()
            .iter()
            .all(|r| r.pid().is_some() && r.screen().screen().contents() == "serving"))
            .await
    );
    let pid = |app: &App, id| app.tasks.get(id).unwrap().pid().unwrap();
    let (pa, pb, pc) = (pid(&app, a), pid(&app, b), pid(&app, c));
    let term = draw_sized(&mut app, 120, 30);
    assert_eq!(task_tabs(&app), vec![a, b, c], "a tab per run");
    assert_eq!(app.ui.panel_tab, PanelTab::Task(c), "the newest shown");

    // A click on a task tab shows it; Kill Task kills the shown one.
    let (rect, _) = *app
        .hits
        .panel_tabs
        .iter()
        .find(|(_, t)| *t == PanelTab::Task(b))
        .unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x, rect.y);
    assert_eq!((app.ui.panel_tab, app.ui.focus), (PanelTab::Task(b), Focus::Panel));
    drop(term);
    app.test_execute(Command::KillTask);
    assert!(settle_ended(&mut app, b).await);
    assert!(matches!(app.tasks.get(b).unwrap().status(), Status::Exited(e) if e.killed));
    assert!(wait_gone(pb).await);
    assert_eq!(app.message.as_ref().unwrap().text, "Task 'serve' was killed");
    let term = draw_sized(&mut app, 120, 30);
    assert!(find(&term, "serve ✗ killed ×").is_some());
    assert!(group_alive(pa) && group_alive(pc), "only the shown one");
    // Killing an ended one says so.
    app.test_execute(Command::KillTask);
    assert_eq!(app.message.as_ref().unwrap().text, "Task 'serve' isn't running");

    // Cmd+W in the focused task tab closes it (it had ended), showing the tab before it.
    app.press(KeyCode::Char('w'), cmd());
    assert!(app.tasks.get(b).is_none());
    assert_eq!((app.ui.panel_tab, app.ui.focus), (PanelTab::Task(a), Focus::Panel));
    // Its × closes a running one, killing it.
    draw_sized(&mut app, 120, 30);
    let (rect, _) = *app.hits.panel_closes.iter().find(|(_, id)| *id == c).unwrap();
    app.mouse(MouseEventKind::Down(MouseButton::Left), rect.x, rect.y);
    assert!(app.tasks.get(c).is_none());
    assert!(wait_gone(pc).await, "closing kills the task");
    assert_eq!(app.ui.panel_tab, PanelTab::Task(a));
    // The last one: Close Task Tab (palette) on the shown tab, the Problems list comes back.
    app.test_execute(Command::CloseTask);
    assert!(app.tasks.runs().is_empty());
    assert!(wait_gone(pa).await);
    assert_eq!(app.ui.panel_tab, PanelTab::Problems);
    assert_ne!(app.ui.focus, Focus::Panel);
    let term = draw_sized(&mut app, 120, 30);
    assert!(task_tabs(&app).is_empty());
    assert!(find(&term, "PROBLEMS").is_some());
    app.test_execute(Command::CloseTask);
    assert_eq!(app.message.as_ref().unwrap().text, "No task tab is shown");
    app.test_execute(Command::KillTask);
    assert_eq!(app.message.as_ref().unwrap().text, "No task is running");

    // A middle click on a tab that isn't shown closes it (killing it), the shown one stays.
    let d = pick_task(&mut app, "serve");
    let e = pick_task(&mut app, "serve");
    assert!(app.settle(|a| a.tasks.runs().iter().all(|r| r.pid().is_some())).await);
    let pd = pid(&app, d);
    draw_sized(&mut app, 120, 30);
    let tab_rect = |app: &App, id| {
        app.hits
            .panel_tabs
            .iter()
            .find(|(_, t)| *t == PanelTab::Task(id))
            .unwrap()
            .0
    };
    let rect = tab_rect(&app, d);
    app.mouse(MouseEventKind::Down(MouseButton::Middle), rect.x + 1, rect.y);
    assert!(app.tasks.get(d).is_none());
    assert!(wait_gone(pd).await);
    assert_eq!((app.ui.panel_tab, app.ui.focus), (PanelTab::Task(e), Focus::Panel));
    // The last one: back to Problems, the keys back to the editor.
    draw_sized(&mut app, 120, 30);
    let rect = tab_rect(&app, e);
    app.mouse(MouseEventKind::Down(MouseButton::Middle), rect.x, rect.y);
    assert!(app.tasks.runs().is_empty());
    assert_eq!(app.ui.panel_tab, PanelTab::Problems);
    assert_ne!(app.ui.focus, Focus::Panel);
}

// 12c: the done-when tests.

use ratatui::style::Modifier;

/// A cell of `reference`'s screen: the char, its foreground (`Reset`, `Indexed`, `Rgb`, as
/// `tui-term` gives it before the theme) and bold.
type RefCell = (char, Color, bool);

/// What a terminal shows for `text`, modelled independently of `vt100` and only as far as the
/// output these tests check needs: printable chars, CR, LF (to the next line's start, as a PTY's
/// `onlcr` makes it), `CSI K` (erase to the line's end), `CSI n A / B / C / D` (cursor moves),
/// SGR (bold, 16 colours, `38;5;n`, `38;2;r;g;b`, resets) and OSC (cargo's hyperlinks: nothing
/// shown). Anything else panics: the model would be wrong. No wrapping or scrolling (the tests
/// keep the output within the screen). Rows of cells, erased cells dropped from row ends, trailing
/// empty rows dropped.
fn reference(text: &str) -> Vec<Vec<RefCell>> {
    let mut rows: Vec<Vec<RefCell>> = Vec::new();
    let (mut row, mut col) = (0usize, 0usize);
    let (mut fg, mut bold) = (Color::Reset, false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\r' => col = 0,
            '\n' => (row, col) = (row + 1, 0),
            '\x1b' => {
                let next = chars.next();
                if next == Some(']') {
                    // OSC (cargo's hyperlinks, a title): shows nothing; ends at BEL or ESC \.
                    loop {
                        match chars.next().expect("a whole OSC") {
                            '\x07' => break,
                            '\x1b' => {
                                assert_eq!(chars.next(), Some('\\'), "OSC ends with ST");
                                break;
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                assert_eq!(next, Some('['), "only CSI and OSC sequences are modelled");
                let mut params = String::new();
                let end = loop {
                    match chars.next().expect("a whole sequence") {
                        c if c.is_ascii_digit() || c == ';' => params.push(c),
                        c => break c,
                    }
                };
                let n = params.parse::<usize>().unwrap_or(1).max(1);
                match end {
                    'K' => {
                        assert!(params.is_empty() || params == "0", "CSI {params} K");
                        if let Some(line) = rows.get_mut(row) {
                            line.truncate(col);
                        }
                    }
                    'A' => row = row.saturating_sub(n),
                    'B' => row += n,
                    'C' => col += n,
                    'D' => col = col.saturating_sub(n),
                    'm' => {
                        let codes: Vec<u8> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
                        let mut codes = codes.into_iter();
                        while let Some(code) = codes.next() {
                            match code {
                                0 => (fg, bold) = (Color::Reset, false),
                                1 => bold = true,
                                22 => bold = false,
                                // Italic, underline: not compared.
                                3 | 4 | 23 | 24 => {}
                                30..=37 => fg = Color::Indexed(code - 30),
                                39 => fg = Color::Reset,
                                90..=97 => fg = Color::Indexed(code - 90 + 8),
                                38 => match codes.next() {
                                    Some(5) => fg = Color::Indexed(codes.next().unwrap()),
                                    Some(2) => {
                                        let mut c = || codes.next().unwrap();
                                        fg = Color::Rgb(c(), c(), c());
                                    }
                                    other => panic!("SGR 38;{other:?}"),
                                },
                                other => panic!("SGR {other} isn't modelled"),
                            }
                        }
                    }
                    other => panic!("CSI {params} {other} isn't modelled"),
                }
            }
            c if c.is_control() => panic!("{c:?} isn't modelled"),
            c => {
                if rows.len() <= row {
                    rows.resize(row + 1, Vec::new());
                }
                let line = &mut rows[row];
                if line.len() < col {
                    line.resize(col, (' ', Color::Reset, false));
                }
                if col < line.len() {
                    line[col] = (c, fg, bold);
                } else {
                    line.push((c, fg, bold));
                }
                col += 1;
            }
        }
    }
    while rows.last().is_some_and(|r| r.iter().all(|c| c.0 == ' ')) {
        rows.pop();
    }
    rows
}

fn reference_text(rows: &[Vec<RefCell>]) -> String {
    let lines: Vec<String> = rows
        .iter()
        .map(|r| r.iter().map(|c| c.0).collect::<String>().trim_end().to_string())
        .collect();
    lines.join("\n")
}

/// Checks the task tab drawn in `term` shows the `expected` rows from its top: the same text,
/// and each visible char in the colour the theme gives the reference cell's colour (bold
/// included), as a terminal with Dark+'s palette would show it. A row's comparison stops where
/// `cut` says (a build time that differs between runs). Returns the drawn rows below them.
fn assert_drawn_as(
    app: &App,
    term: &Terminal<TestBackend>,
    expected: &[Vec<RefCell>],
    cut: impl Fn(&str) -> Option<usize>,
) -> Vec<String> {
    let area = crate::layout::task_area(app.hits.regions.panel.unwrap());
    let buf = term.backend().buffer();
    let drawn = |row: u16, col: u16| &buf[(area.x + col, area.y + row)];
    let drawn_row = |row: u16| -> String { (0..area.width).map(|col| drawn(row, col).symbol()).collect() };
    let themed = |&(_, fg, bold): &RefCell| {
        let mut cell = ratatui::buffer::Cell::default();
        cell.fg = fg;
        if bold {
            cell.modifier = Modifier::BOLD;
        }
        app.theme.terminal_cell(&mut cell);
        cell.fg
    };
    assert!(expected.len() < area.height as usize, "the output fits");
    for (row, cells) in expected.iter().enumerate() {
        assert!(cells.len() < area.width as usize, "no line wraps");
        let text: String = cells.iter().map(|c| c.0).collect();
        let n = cut(&text).unwrap_or(cells.len());
        let got = drawn_row(row as u16);
        assert_eq!(
            got.chars().take(n).collect::<String>(),
            text.chars().take(n).collect::<String>(),
            "row {row}"
        );
        if n == cells.len() {
            assert_eq!(got.trim_end(), text.trim_end(), "row {row}");
        }
        for (col, cell) in cells.iter().enumerate().take(n) {
            if cell.0 != ' ' {
                assert_eq!(
                    drawn(row as u16, col as u16).fg,
                    themed(cell),
                    "row {row} col {col} ({:?} in {text:?})",
                    cell.0
                );
            }
        }
    }
    (expected.len() as u16..area.height).map(drawn_row).collect()
}

/// Settles until run `id` ended (`settle_ended`), for up to `limit` (a real build under a loaded
/// test run takes longer than `settle`'s 5 s). `each` sees the app at every step.
async fn settle_ended_within(app: &mut App, id: RunId, limit: std::time::Duration, each: impl Fn(&App)) -> bool {
    let until = std::time::Instant::now() + limit;
    while std::time::Instant::now() < until {
        let ended = app
            .settle(|a| {
                each(a);
                let run = a.tasks.get(id).unwrap();
                tasks::end_note(&run.name, &run.status())
                    .is_some_and(|n| run.screen().screen().contents().ends_with(&n))
            })
            .await;
        if ended {
            return true;
        }
    }
    false
}

/// A script's progress line (`\r`-overwritten frames, `ESC[K`), cursor movement up and right
/// and colours end up on screen as a terminal shows them: the same text and colours as an
/// independent model of a terminal (`reference`) fed the script's output, and as written out.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_done_when_progress_output_renders_as_in_a_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let script = r#"printf 'header\n'
printf '\033[1;32mBuilding\033[0m [=  ] 1/3\r'; sleep 0.05
printf '\033[1;32mBuilding\033[0m [== ] 2/3\r'; sleep 0.05
printf '\033[1;32mBuilding\033[0m [===] 3/3\r'; sleep 0.05
printf '\033[K'
printf 'line a\nline b\n'
printf '\033[2A\033[33mLINE\033[0m\r\n\n'
printf 'progress 10%%\rprogress 100%%\n'
printf 'abcdef\r\033[3C\033[K!\n'
printf '\033[1;32mFinished\033[0m\n'"#;
    let mut app = open_app(
        &root,
        &state,
        Some(&format!("[tasks.progress]\ncommand = '''{script}'''\n")),
    );
    draw_sized(&mut app, 100, 30);
    let id = pick_task(&mut app, "progress");
    assert!(settle_ended(&mut app, id).await);
    match app.tasks.get(id).unwrap().status() {
        Status::Exited(e) => assert!(e.success(), "{e:?}"),
        s => panic!("{s:?}"),
    }
    assert_eq!(
        screen_text(&app, id),
        "header\nLINE a\nline b\nprogress 100%\nabc!\nFinished\nTask 'progress' exited with code 0"
    );

    // The same script's bytes through the reference model.
    let output = std::process::Command::new(&app.tasks.shell)
        .args(["-c", script])
        .output()
        .unwrap();
    assert!(output.status.success());
    let expected = reference(&String::from_utf8(output.stdout).unwrap());
    assert_eq!(
        reference_text(&expected),
        "header\nLINE a\nline b\nprogress 100%\nabc!\nFinished"
    );
    let term = draw_sized(&mut app, 100, 30);
    let rest = assert_drawn_as(&app, &term, &expected, |_| None);
    assert_eq!(rest[0].trim_end(), "Task 'progress' exited with code 0");
    assert!(rest[1..].iter().all(|r| r.trim().is_empty()));
    // Spelled out: bold green drawn bright, plain yellow, the default colour.
    let key_fg = |k: &str| app.theme.get(k).fg.unwrap();
    let finished = find(&term, "Finished").unwrap();
    assert_eq!(fg(&term, finished), key_fg("terminal.ansiBrightGreen"));
    let line = find(&term, "LINE a").unwrap();
    assert_eq!(fg(&term, line), key_fg("terminal.ansiYellow"));
    assert_eq!(fg(&term, (line.0 + 5, line.1)), key_fg("terminal.foreground"));
}

/// The done-when: `cargo build` of a small crate with a warning (and a build script slow enough
/// for cargo's progress bar), run from "Run Task", shows on screen what cargo prints — the same
/// text and colours as an independent model of a terminal fed cargo's coloured output from a
/// build run just before (piped: no progress bar), the progress bar gone — with "Compiling" /
/// "Finished" in bold bright green, "warning" in bold yellow drawn bright, and the exit code.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_done_when_a_colourful_build_renders_as_in_a_terminal() {
    let has_cargo = std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_cargo {
        eprintln!("note: no cargo on PATH, skipping the colourful build");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let demo = root.join("demo");
    fs::create_dir_all(demo.join("src")).unwrap();
    fs::write(
        demo.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(demo.join("src/main.rs"), "fn main() {\n    let unused = 1;\n}\n").unwrap();
    // Cargo draws its progress bar once a build has taken 0.5 s.
    fs::write(
        demo.join("build.rs"),
        "fn main() {\n    std::thread::sleep(std::time::Duration::from_millis(1200));\n}\n",
    )
    .unwrap();
    let build = "cargo build --color=always --offline";
    // Each build with its own target folder and cargo home: no lock shared with the other build
    // or any other cargo running ("Blocking waiting for file lock …" would change the output).
    // Cargo's settings that change what it prints, as they'd be without the environment's (a
    // CI's `RUSTFLAGS=-Dwarnings` would fail the build, `CARGO_TERM_*` drop the progress bar or
    // add lines); `CARGO_HOME` keeps `~/.cargo/config.toml` out. The terminal's own variables
    // (`TERM_PROGRAM` turning on cargo's hyperlinks) are left alone, as a real run has them.
    let pinned = [
        ("RUSTFLAGS", ""),
        ("CARGO_ENCODED_RUSTFLAGS", ""),
        ("CARGO_TERM_PROGRESS_WHEN", "auto"),
        ("CARGO_TERM_VERBOSE", "false"),
        ("CARGO_TERM_QUIET", "false"),
    ];
    let pinned_toml: String = pinned.iter().map(|(k, v)| format!(", {k} = '{v}'")).collect();
    let tasks_toml = format!(
        "[tasks.build]\ncommand = \"{build}\"\ncwd = \"demo\"\nenv = {{ CARGO_TARGET_DIR = '{}', CARGO_HOME = '{}'{pinned_toml} }}\n",
        state.join("target").display(),
        state.join("cargo-home").display()
    );
    // The reference build first (two builds of one crate at once wait for each other's lock on
    // its `Cargo.lock`, printing that): its own target folder, output piped.
    let output = {
        let (demo, target, home) = (
            demo.clone(),
            state.join("reference-target"),
            state.join("reference-cargo-home"),
        );
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("sh")
                .args(["-c", build])
                .current_dir(demo)
                .env("CARGO_TARGET_DIR", target)
                .env("CARGO_HOME", home)
                .envs(pinned)
                .output()
                .unwrap()
        })
        .await
        .unwrap()
    };
    let mut app = open_app(&root, &state, Some(&tasks_toml));
    app.ui.panel_height = 22;
    draw_sized(&mut app, 200, 40);

    let id = pick_task(&mut app, "build");
    let saw_progress = std::cell::Cell::new(false);
    let ended = settle_ended_within(&mut app, id, std::time::Duration::from_secs(120), |a| {
        if screen_text(a, id).contains("Building [") {
            saw_progress.set(true);
        }
    })
    .await;
    assert!(ended, "{}", screen_text(&app, id));
    match app.tasks.get(id).unwrap().status() {
        Status::Exited(e) => assert!(e.success(), "{e:?}"),
        s => panic!("{s:?}"),
    }
    // Cargo draws no progress bar in a CI (`CI` or `TF_BUILD` set, whatever their value).
    if ["CI", "TF_BUILD"].iter().all(|v| std::env::var_os(v).is_none()) {
        assert!(saw_progress.get(), "cargo drew its progress bar");
    }
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let expected = reference(&String::from_utf8(output.stderr).unwrap());
    let text = reference_text(&expected);
    assert!(
        text.starts_with(&format!(
            "   Compiling demo v0.1.0 ({})\nwarning: unused variable: `unused`",
            demo.display()
        )),
        "{text}"
    );

    let term = draw_sized(&mut app, 200, 40);
    let time = regex::Regex::new(r" in \d+(\.\d+)?s$").unwrap();
    let rest = assert_drawn_as(&app, &term, &expected, |row| {
        row.trim_start()
            .starts_with("Finished")
            .then(|| row.find(" in ").unwrap())
    });
    let finished = find(&term, "Finished `dev` profile").unwrap();
    let row = &lines(&term)[finished.1 as usize];
    assert!(time.is_match(row.trim_end()), "{row}");
    assert_eq!(rest[0].trim_end(), "Task 'build' exited with code 0");
    assert!(!screen_text(&app, id).contains("Building"), "the progress bar is gone");
    // Spelled out.
    let key_fg = |k: &str| app.theme.get(k).fg.unwrap();
    for word in ["Compiling", "Finished"] {
        let at = find(&term, word).unwrap();
        assert_eq!(fg(&term, at), key_fg("terminal.ansiBrightGreen"), "{word}");
        assert!(term.backend().buffer()[at].modifier.contains(Modifier::BOLD));
    }
    let warning = find(&term, "warning: unused").unwrap();
    assert_eq!(fg(&term, warning), key_fg("terminal.ansiBrightYellow"));
    let note = find(&term, "Task 'build' exited with code 0").unwrap();
    assert_eq!(fg(&term, note), key_fg("terminal.foreground"));
    assert!(find(&term, "build ✓ ×").is_some());
}

/// The done-when: while the workspace isn't trusted — undecided (the dialog up), restricted from
/// the dialog, restricted as stored — no `.workspace` task is listed or runs (the marker file it
/// would create never appears, no run starts), the user's `tasks.toml` tasks still run, and a
/// name both define runs the user's; once trusted, the workspace's run.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_done_when_restricted_mode_runs_no_workspace_task() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let (ws_marker, user_marker) = (state.join("ws-marker"), state.join("user-marker"));
    let workspace = format!(
        "[tasks.deploy]\ncommand = \"touch '{}'\"\n\n[tasks.build]\ncommand = \"echo workspace build\"\n",
        ws_marker.display()
    );
    let user = format!(
        "[tasks.build]\ncommand = \"echo user build\"\n\n[tasks.touch-user]\ncommand = \"touch '{}'\"\n",
        user_marker.display()
    );
    fs::write(root.join(".workspace"), &workspace).unwrap();
    let names =
        |app: &App| -> Vec<(String, Source)> { app.task_templates().into_iter().map(|t| (t.name, t.source)).collect() };
    let user_only = vec![
        ("build".to_string(), Source::User),
        ("touch-user".to_string(), Source::User),
    ];
    let run_and_wait = async |app: &mut App, name: &str| {
        let id = pick_task(app, name);
        assert!(settle_ended(app, id).await);
        screen_text(app, id)
    };
    /// Restricted: the picker hides `deploy`, says why, runs nothing for it; `run_task` refuses.
    async fn refuses_deploy(app: &mut App) {
        open_run_task(app);
        let p = app.palette.as_ref().unwrap();
        let labels: Vec<&str> = p.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["build", "touch-user"]);
        assert!(
            p.hint.as_deref().is_some_and(|h| h.contains("2 tasks in .workspace")),
            "{:?}",
            p.hint
        );
        app.type_text("deploy");
        assert!(app.palette.as_ref().unwrap().rows.is_empty());
        let runs = app.tasks.runs().len();
        app.press(KeyCode::Enter, M::NONE);
        assert_eq!(app.run_task("deploy"), None);
        assert_eq!(app.message.as_ref().unwrap().text, "No task named 'deploy'");
        assert_eq!(app.tasks.runs().len(), runs, "no run started");
    }

    // Undecided: the trust dialog is up.
    let mut app = open_app(&root, &state, Some(&user));
    assert_eq!(app.trust, Trust::Undecided);
    assert!(app.prompt.is_some());
    assert_eq!(names(&app), user_only);
    assert_eq!(app.run_task("deploy"), None);
    assert!(app.tasks.runs().is_empty());
    let id = app.run_task("build").unwrap();
    assert!(settle_ended(&mut app, id).await);
    assert!(screen_text(&app, id).starts_with("user build\n"));

    // Restricted from the dialog.
    app.press(KeyCode::Char('r'), M::NONE);
    assert_eq!(app.trust, Trust::Restricted);
    refuses_deploy(&mut app).await;
    assert!(run_and_wait(&mut app, "build").await.starts_with("user build\n"));
    assert!(
        run_and_wait(&mut app, "touch-user")
            .await
            .ends_with("exited with code 0")
    );
    assert!(user_marker.exists(), "the user's task ran");
    assert!(!ws_marker.exists(), "no .workspace task ran");

    // Trusted (Manage Workspace Trust): the workspace's run, its build winning.
    app.test_execute(Command::ManageWorkspaceTrust);
    app.press(KeyCode::Char('t'), M::NONE);
    assert_eq!(app.trust, Trust::Trusted);
    assert!(run_and_wait(&mut app, "build").await.starts_with("workspace build\n"));
    assert!(run_and_wait(&mut app, "deploy").await.ends_with("exited with code 0"));
    assert!(ws_marker.exists(), "trusted: the workspace's task ran");
    drop(app);

    // Restricted as stored (no dialog), in a fresh workspace.
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    fs::write(root.join(".workspace"), &workspace).unwrap();
    fs::remove_file(&ws_marker).unwrap();
    let mut store = TrustStore::load(state.join("trust.toml"));
    store.record(&root, &Workspace::load(root.clone()).config, false);
    store.write().unwrap();
    let mut app = open_app(&root, &state, Some(&user));
    assert_eq!(app.trust, Trust::Restricted);
    assert!(app.prompt.is_none());
    refuses_deploy(&mut app).await;
    assert!(run_and_wait(&mut app, "build").await.starts_with("user build\n"));
    assert!(!ws_marker.exists(), "no .workspace task ran");
}

/// Keys typed while a run is still starting (its process not spawned yet) reach it once it runs.
#[tokio::test(flavor = "multi_thread")]
async fn milestone_12_keys_typed_while_a_task_starts_reach_it() {
    let dir = tempfile::tempdir().unwrap();
    let (root, state) = dirs(&dir);
    let mut app = open_app(&root, &state, Some(TASKS));
    let id = pick_task(&mut app, "ask");
    assert_eq!(app.tasks.get(id).unwrap().status(), Status::Starting);
    app.type_text("y");
    app.press(KeyCode::Enter, M::NONE);
    assert!(
        app.settle(|a| screen_text(a, id).contains("got y")).await,
        "{}",
        screen_text(&app, id)
    );
    app.test_execute(Command::KillTask);
    assert!(settle_ended(&mut app, id).await);
}
