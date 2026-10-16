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
//! (`milestone_12_kill_and_close_task_tabs`).

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
