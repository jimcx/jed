//! Tasks: throwaway terminals running one command each (no drawing; `app/tasks.rs` drives them).
//!
//! - `templates`: `.workspace` `[tasks]` + the user's `tasks.toml`, merged (the workspace wins a
//!   name; untrusted workspaces contribute none), resolved to a `Spec` (`$SHELL -c command`, cwd,
//!   env)
//! - `run`, `keys`, `mouse`, `select` re-export `jterm`'s (docs/dev/crates.md); `run`: one `Run` in a PTY, its output in a `vt100` screen with scrollback; `Event`s through a
//!   `Sink` (the app's channel); input via `Run::write`
//! - `keys`: keys and pastes encoded as a terminal sends them
//! - `mouse`: mouse events encoded as a terminal sends them (xterm's mouse protocol)
//! - `select`: a left-drag selection over a run's screen (its cells and the text they copy), and
//!   `top_row` (the first row a screen taller than its area is drawn from; `ui/task.rs` uses it)
//! - `Tasks` (`app.tasks`): the user's templates, the runs (kept until closed), their sink

pub mod keys;
pub mod mouse;
pub mod run;
pub mod search;
pub mod select;
pub mod templates;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use run::{EnvNames, Event, Exit, Run, RunId, Scrollback, Sink, StartOptions, Status, TerminalScroll};
pub use templates::{Environment, NamedEnvironment, Source, Task, Template};

/// How many finished commands a run remembers.
pub const COMMAND_RING: usize = 50;

/// What every run of jed is started with: its two environment variables and the command ring.
pub const START_OPTIONS: StartOptions = StartOptions {
    env: EnvNames {
        ipc: crate::ipc::ENV,
        terminal: crate::ipc::TERMINAL_ENV,
    },
    command_ring: COMMAND_RING,
};

/// How long quitting waits for SIGHUP'd tasks before SIGKILLing what's left.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

/// A new run's size until the panel tells it otherwise (`Run::resize`).
pub const DEFAULT_SIZE: (u16, u16) = (24, 80);

pub struct Tasks {
    /// `tasks.toml`'s tasks (read once at startup by `main`; tests set them).
    pub user: BTreeMap<String, Task>,
    /// `tasks.toml`'s `[environments]`.
    pub user_environments: BTreeMap<String, Environment>,
    /// Runs commands with `-c`: `$SHELL`, else `/bin/sh`.
    pub shell: PathBuf,
    /// New runs' (rows, cols).
    pub size: (u16, u16),
    /// Every run, oldest first, until closed (dropping one kills it).
    runs: Vec<Run>,
    next_id: u64,
    /// Set by `App::start`; runs started before then report nowhere.
    sink: Option<Sink>,
    /// The socket path every run gets as `JED_IPC` (set once `App` listens).
    pub ipc: Option<PathBuf>,
    /// Test-only: the next shell opened waits on this before its spawn finishes (kept
    /// `Starting`), so a test can quit while it is still starting deterministically.
    #[cfg(test)]
    pub start_gate: Option<templates::StartGate>,
}

impl Default for Tasks {
    fn default() -> Self {
        Self {
            user: BTreeMap::new(),
            user_environments: BTreeMap::new(),
            shell: templates::default_shell(),
            size: DEFAULT_SIZE,
            runs: Vec::new(),
            next_id: 1,
            sink: None,
            ipc: None,
            #[cfg(test)]
            start_gate: None,
        }
    }
}

/// The line written below a run's output once it ends ("{noun} '{name}' exited with code 2":
/// `noun` is "Task" or "Shell"); `None` while it starts or runs.
pub fn end_note(noun: &str, name: &str, status: &Status) -> Option<String> {
    Some(match status {
        Status::Starting | Status::Running => return None,
        Status::Failed(e) => format!("{noun} '{name}' failed to start: {e}"),
        Status::Exited(Exit { killed: true, .. }) => format!("{noun} '{name}' was killed"),
        Status::Exited(Exit {
            signal: Some(signal), ..
        }) => format!("{noun} '{name}' ended by signal: {signal}"),
        Status::Exited(e) => format!("{noun} '{name}' exited with code {}", e.code),
    })
}

impl Tasks {
    pub fn start(&mut self, sink: Sink) {
        self.sink = Some(sink);
    }

    /// Every template, sorted by name: `workspace` (`.workspace` `[tasks]`, `None` unless trusted)
    /// and the user's, the workspace's winning a clash.
    pub fn templates(&self, workspace: Option<&BTreeMap<String, Task>>) -> Vec<Template> {
        templates::merge(workspace, &self.user)
    }

    /// Every environment, sorted by name: `workspace` (`None` unless trusted) and the user's, the
    /// workspace's winning a clash.
    pub fn environments(&self, workspace: Option<&BTreeMap<String, Environment>>) -> Vec<NamedEnvironment> {
        templates::merge_environments(workspace, &self.user_environments)
    }

    /// Starts `template` (cwd relative to `root`, `{{NAME}}`s filled from `vars`) at `self.size`,
    /// keeping `scrollback` lines above the screen; spawning happens off-thread, so it starts as
    /// `Status::Starting`. `Err(NAME)` (nothing started) when `vars` lacks a variable it uses.
    pub fn run_with(
        &mut self,
        template: &Template,
        root: &Path,
        vars: &Environment,
        scrollback: impl Into<Scrollback>,
    ) -> Result<RunId, String> {
        let mut spec = template.resolve(root, &self.shell, vars)?;
        let id = self.new_run_id();
        if let Some(socket) = &self.ipc {
            spec.env.retain(|(k, _)| k != crate::ipc::ENV);
            spec.env
                .push((crate::ipc::ENV.into(), socket.to_string_lossy().into_owned()));
        }
        let sink = self.sink.clone().unwrap_or_else(|| Arc::new(|_| {}));
        let (rows, cols) = self.size;
        self.runs
            .push(Run::start(id, spec, rows, cols, sink, scrollback, START_OPTIONS));
        Ok(id)
    }

    /// [`run_with`](Self::run_with) with no variables (tests; a template using `{{NAME}}` panics).
    #[cfg(test)]
    pub fn run(&mut self, template: &Template, root: &Path, scrollback: impl Into<Scrollback>) -> RunId {
        self.run_with(template, root, &Environment::new(), scrollback).unwrap()
    }

    /// A new run's id, for a run this `Tasks` doesn't own (a shell tab's).
    pub fn new_run_id(&mut self) -> RunId {
        let id = RunId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    pub fn get(&self, id: RunId) -> Option<&Run> {
        self.runs.iter().find(|r| r.id == id)
    }

    /// The newest run still starting or running.
    pub fn newest_running(&self) -> Option<&Run> {
        self.runs.iter().rev().find(|r| r.status().is_running())
    }

    /// Drops the run (killing it if it still runs). False if there's none.
    pub fn close(&mut self, id: RunId) -> bool {
        let before = self.runs.len();
        self.runs.retain(|r| r.id != id);
        self.runs.len() != before
    }

    /// Runs still starting or running.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running(&self) -> usize {
        self.runs.iter().filter(|r| r.status().is_running()).count()
    }

    /// On quit: SIGHUP to every running task, up to `SHUTDOWN_TIMEOUT` for them to end, then
    /// SIGKILL for the rest. Runs are kept (their status says how they ended).
    pub async fn shutdown(&mut self) {
        hang_up(self.runs.iter(), "task").await;
    }
}

/// On quit: SIGHUP to every run in `runs`, up to `SHUTDOWN_TIMEOUT` for them to end, then SIGKILL
/// for the rest. `label` names them in the warn log ("task" / "shell"). Runs are kept (their
/// status says how they ended).
pub async fn hang_up<'a>(runs: impl Iterator<Item = &'a Run> + Clone, label: &str) {
    let running = || runs.clone().filter(|r| r.status().is_running()).count();
    if running() == 0 {
        return;
    }
    for r in runs.clone() {
        r.kill();
    }
    let deadline = tokio::time::Instant::now() + SHUTDOWN_TIMEOUT;
    while running() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for r in runs.clone() {
        if r.status().is_running() {
            tracing::warn!(run = %r.name, "{label} still running at quit: killing it");
            r.force_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    fn template(command: &str) -> Template {
        Template {
            name: "t".into(),
            task: Task {
                command: command.into(),
                cwd: None,
                env: BTreeMap::new(),
            },
            source: Source::User,
        }
    }

    /// A `Tasks` whose events land in the returned list, running with `/bin/sh`.
    fn tasks() -> (Tasks, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        let mut t = Tasks {
            shell: PathBuf::from("/bin/sh"),
            ..Default::default()
        };
        t.start(Arc::new(move |e| sink_events.lock().unwrap().push(e)));
        (t, events)
    }

    async fn wait_until(what: &str, cond: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// What jed starts every run with: its two variable names and a command ring of 50, so a run
    /// remembers more than a couple of commands.
    #[tokio::test(flavor = "multi_thread")]
    async fn start_options_are_jeds_values() {
        assert_eq!(START_OPTIONS.env.ipc, "JED_IPC");
        assert_eq!(START_OPTIONS.env.terminal, "JED_TERMINAL");
        assert_eq!(START_OPTIONS.command_ring, 50);
        let (mut t, _events) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let script = "for i in 1 2 3 4 5; do printf '\\033]133;B\\007x\\r\\n\\033]133;D;0\\007'; done";
        let id = t.run(&template(script), dir.path(), 100);
        let run = t.get(id).unwrap();
        wait_until("five commands", || {
            run.commands().last().is_some_and(|c| c.id == 5 && c.finished.is_some())
        })
        .await;
        assert_eq!(run.commands().len(), 5);
    }

    fn exit(run: &Run) -> Exit {
        match run.status() {
            Status::Exited(e) => e,
            s => panic!("not exited: {s:?}"),
        }
    }

    /// Whether any process is in group `pgid`.
    fn group_alive(pgid: u32) -> bool {
        crate::milestone12_tests::group_alive(pgid)
    }

    #[test]
    fn end_notes() {
        let exit = |code, signal: Option<&str>, killed| {
            Status::Exited(Exit {
                code,
                signal: signal.map(str::to_string),
                killed,
            })
        };
        assert_eq!(end_note("Task", "b", &Status::Running), None);
        assert_eq!(
            end_note("Task", "b", &exit(2, None, false)).unwrap(),
            "Task 'b' exited with code 2"
        );
        assert_eq!(
            end_note("Task", "b", &exit(1, Some("Hangup"), true)).unwrap(),
            "Task 'b' was killed"
        );
        assert_eq!(
            end_note("Task", "b", &exit(1, Some("Segmentation fault"), false)).unwrap(),
            "Task 'b' ended by signal: Segmentation fault"
        );
        assert_eq!(
            end_note("Task", "b", &Status::Failed("x: no such folder".into())).unwrap(),
            "Task 'b' failed to start: x: no such folder"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cwd_and_env() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::workspace::normalize(dir.path());
        std::fs::create_dir(root.join("sub")).unwrap();
        let mut tpl = template("pwd; echo \"$TERM $COLORTERM $GREETING\"");
        tpl.task.cwd = Some("sub".into());
        tpl.task.env.insert("GREETING".into(), "hi there".into());
        let id = t.run(&tpl, &root, run::SCROLLBACK);
        let run = t.get(id).unwrap();
        wait_until("exit", || !run.status().is_running()).await;
        assert_eq!(
            run.screen().screen().contents(),
            format!("{}\nxterm-256color truecolor hi there", root.join("sub").display())
        );
        assert_eq!(run.cwd, root.join("sub"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_kills_running_tasks() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let a = t.run(
            &template("trap '' HUP; echo ready; sleep 30"),
            dir.path(),
            run::SCROLLBACK,
        );
        let b = t.run(&template("sleep 30"), dir.path(), run::SCROLLBACK);
        let done = t.run(&template("true"), dir.path(), run::SCROLLBACK);
        wait_until("running", || {
            t.get(a).unwrap().screen().screen().contents() == "ready" && t.get(b).unwrap().pid().is_some()
        })
        .await;
        wait_until("done", || !t.get(done).unwrap().status().is_running()).await;
        let pa = t.get(a).unwrap().pid().unwrap();
        let at = Instant::now();
        t.shutdown().await;
        let took = at.elapsed();
        assert!(took >= SHUTDOWN_TIMEOUT && took < SHUTDOWN_TIMEOUT * 2, "{took:?}");
        wait_until("all ended", || t.running() == 0).await;
        wait_until("group gone", || !group_alive(pa)).await;
        // Shutdown's own SIGKILL, not `kill`'s escalation a `KILL_GRACE` after the SIGHUP.
        assert!(at.elapsed() < run::KILL_GRACE, "{:?}", at.elapsed());
        let signal = |id| exit(t.get(id).unwrap()).signal.unwrap_or_default();
        assert!(signal(a).contains("Kill"), "{}", signal(a));
        assert!(signal(b).contains("Hangup"), "asked to end first: {}", signal(b));
        assert!(exit(t.get(done).unwrap()).success(), "an ended run is left alone");
    }
}
