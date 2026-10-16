//! Tasks: throwaway terminals running one command each (no drawing; `app/tasks.rs` drives them).
//!
//! - `templates`: `.workspace` `[tasks]` + the user's `tasks.toml`, merged (the workspace wins a
//!   name; untrusted workspaces contribute none), resolved to a `Spec` (`$SHELL -c command`, cwd,
//!   env)
//! - `run`: one `Run` in a PTY, its output in a `vt100` screen with scrollback; `Event`s through a
//!   `Sink` (the app's channel)
//! - `Tasks` (`app.tasks`): the user's templates, the runs (kept until closed), their sink

// Until 12b's picker and panel tabs use the rest of the API.
#![cfg_attr(not(test), allow(dead_code, unused_imports))]

pub mod run;
pub mod templates;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use run::{Event, Exit, Run, RunId, Sink, Status};
pub use templates::{Source, Task, Template};

/// How long quitting waits for SIGHUP'd tasks before SIGKILLing what's left.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

/// A new run's size until the panel tells it otherwise (`Run::resize`).
pub const DEFAULT_SIZE: (u16, u16) = (24, 80);

pub struct Tasks {
    /// `tasks.toml`'s tasks (read once at startup by `main`; tests set them).
    pub user: BTreeMap<String, Task>,
    /// Runs commands with `-c`: `$SHELL`, else `/bin/sh`.
    pub shell: PathBuf,
    /// New runs' (rows, cols).
    pub size: (u16, u16),
    /// Every run, oldest first, until closed (dropping one kills it).
    runs: Vec<Run>,
    next_id: u64,
    /// Set by `App::start`; runs started before then report nowhere.
    sink: Option<Sink>,
}

impl Default for Tasks {
    fn default() -> Self {
        Self {
            user: BTreeMap::new(),
            shell: templates::default_shell(),
            size: DEFAULT_SIZE,
            runs: Vec::new(),
            next_id: 1,
            sink: None,
        }
    }
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

    /// Starts `template` (cwd relative to `root`) at `self.size`; spawning happens off-thread, so
    /// it starts as `Status::Starting`.
    pub fn run(&mut self, template: &Template, root: &Path) -> RunId {
        let id = RunId(self.next_id);
        self.next_id += 1;
        let spec = template.resolve(root, &self.shell);
        let sink = self.sink.clone().unwrap_or_else(|| Arc::new(|_| {}));
        let (rows, cols) = self.size;
        self.runs.push(Run::start(id, spec, rows, cols, sink));
        id
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    pub fn get(&self, id: RunId) -> Option<&Run> {
        self.runs.iter().find(|r| r.id == id)
    }

    /// Drops the run (killing it if it still runs). False if there's none.
    pub fn close(&mut self, id: RunId) -> bool {
        let before = self.runs.len();
        self.runs.retain(|r| r.id != id);
        self.runs.len() != before
    }

    /// Runs still starting or running.
    pub fn running(&self) -> usize {
        self.runs.iter().filter(|r| r.status().is_running()).count()
    }

    /// On quit: SIGHUP to every running task, up to `SHUTDOWN_TIMEOUT` for them to end, then
    /// SIGKILL for the rest. Runs are kept (their status says how they ended).
    pub async fn shutdown(&mut self) {
        if self.running() == 0 {
            return;
        }
        for r in &self.runs {
            r.kill();
        }
        let deadline = tokio::time::Instant::now() + SHUTDOWN_TIMEOUT;
        while self.running() > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for r in &self.runs {
            if r.status().is_running() {
                tracing::warn!(task = %r.name, "task still running at quit: killing it");
                r.force_kill();
            }
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

    fn exit(run: &Run) -> Exit {
        match run.status() {
            Status::Exited(e) => e,
            s => panic!("not exited: {s:?}"),
        }
    }

    /// Whether any process is in group `pgid`.
    fn group_alive(pgid: u32) -> bool {
        // SAFETY: signal 0 only checks for existence.
        unsafe { libc::kill(-(pgid as i32), 0) == 0 }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn output_and_exit_code() {
        let (mut t, events) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let id = t.run(
            &template("printf 'hello\\n\\033[31mred\\033[0m\\n'; exit 3"),
            dir.path(),
        );
        let run = t.get(id).unwrap();
        wait_until("exit", || !run.status().is_running()).await;
        assert_eq!(exit(run).code, 3);
        assert!(!exit(run).success());
        {
            let screen = run.screen();
            assert_eq!(screen.screen().contents(), "hello\nred");
            let cell = screen.screen().cell(1, 0).unwrap();
            assert_eq!(cell.fgcolor(), vt100::Color::Idx(1));
            assert_eq!(screen.screen().cell(0, 0).unwrap().fgcolor(), vt100::Color::Default);
        }
        // The events are sent after the status changes (from other threads): wait for them.
        wait_until("events", || {
            let events = events.lock().unwrap();
            events.contains(&Event::Output(id)) && events.contains(&Event::Ended(id))
        })
        .await;
        assert_eq!(events.lock().unwrap().first(), Some(&Event::Started(id)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn output_events_are_throttled() {
        let (mut t, events) = tasks();
        let dir = tempfile::tempdir().unwrap();
        // ~2000 separate writes over ~100 ms or more.
        let id = t.run(
            &template("i=0; while [ $i -lt 2000 ]; do echo line $i; i=$((i+1)); done"),
            dir.path(),
        );
        let run = t.get(id).unwrap();
        let started = Instant::now();
        wait_until("exit", || !run.status().is_running()).await;
        let elapsed = started.elapsed();
        assert!(run.screen().screen().contents().ends_with("line 1999"));
        let outputs = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Event::Output(id))
            .count();
        let most = elapsed.as_millis() / run::FRAME.as_millis() + 2;
        assert!(
            (1..=most as usize).contains(&outputs),
            "{outputs} output events in {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scrollback_keeps_earlier_lines() {
        let (mut t, _) = tasks();
        t.size = (5, 40);
        let dir = tempfile::tempdir().unwrap();
        let id = t.run(&template("seq 1 50"), dir.path());
        let run = t.get(id).unwrap();
        wait_until("exit", || !run.status().is_running()).await;
        let mut p = run.screen();
        assert_eq!(p.screen().contents(), "47\n48\n49\n50");
        p.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(p.screen().scrollback(), 46, "scrolled to the top");
        assert!(p.screen().contents().starts_with("1\n2\n"));
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
        let id = t.run(&tpl, &root);
        let run = t.get(id).unwrap();
        wait_until("exit", || !run.status().is_running()).await;
        assert_eq!(
            run.screen().screen().contents(),
            format!("{}\nxterm-256color truecolor hi there", root.join("sub").display())
        );
        assert_eq!(run.cwd, root.join("sub"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_cwd_fails_to_start() {
        let (mut t, events) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let mut tpl = template("true");
        tpl.task.cwd = Some("nope".into());
        let id = t.run(&tpl, dir.path());
        let run = t.get(id).unwrap();
        wait_until("failure", || !run.status().is_running()).await;
        match run.status() {
            Status::Failed(m) => assert!(m.ends_with("nope: no such folder"), "{m}"),
            s => panic!("{s:?}"),
        }
        wait_until("ended", || events.lock().unwrap().contains(&Event::Ended(id))).await;
        assert!(!events.lock().unwrap().contains(&Event::Started(id)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resize_reaches_the_process() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        // Prints the size it sees once told (SIGWINCH) or after a while.
        let id = t.run(
            &template("trap 'stty size; exit 0' WINCH; echo ready; i=0; while [ $i -lt 500 ]; do sleep 0.01; i=$((i+1)); done"),
            dir.path(),
        );
        let run = t.get(id).unwrap();
        wait_until("ready", || run.screen().screen().contents().contains("ready")).await;
        run.resize(10, 50);
        assert_eq!(run.screen().screen().size(), (10, 50));
        wait_until("exit", || !run.status().is_running()).await;
        assert_eq!(run.screen().screen().contents(), "ready\n10 50");
        assert!(exit(run).success());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kill_ends_the_whole_group() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        // A background child too: the kill must reach it.
        let id = t.run(&template("sleep 30 & sleep 30; echo never"), dir.path());
        let run = t.get(id).unwrap();
        wait_until("running", || run.pid().is_some()).await;
        let pgid = run.pid().unwrap();
        // Let the shell start both sleeps.
        wait_until("children", || child_count(pgid) >= 2).await;
        let at = Instant::now();
        run.kill();
        wait_until("exit", || !run.status().is_running()).await;
        assert!(at.elapsed() < run::KILL_GRACE, "SIGHUP was enough");
        let e = exit(run);
        assert!(e.killed && !e.success(), "{e:?}");
        assert!(e.signal.is_some(), "{e:?}");
        wait_until("group gone", || !group_alive(pgid)).await;
        assert!(!run.screen().screen().contents().contains("never"));
        run.kill(); // nothing once ended
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kill_escalates_when_hangup_is_ignored() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let id = t.run(&template("trap '' HUP; echo ready; sleep 30"), dir.path());
        let run = t.get(id).unwrap();
        wait_until("ready", || run.screen().screen().contents().contains("ready")).await;
        let pgid = run.pid().unwrap();
        run.kill();
        // `sleep` ignores SIGHUP too (inherited): only the SIGKILL ends it.
        wait_until("exit", || !run.status().is_running()).await;
        assert_eq!(exit(run).signal.as_deref().map(|s| s.contains("Kill")), Some(true));
        // The orphaned `sleep` stays a group member until its new parent reaps it.
        wait_until("group gone", || !group_alive(pgid)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kill_escalates_to_leftovers_of_an_ended_leader() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        // The shell dies of the SIGHUP; its child ignores it and keeps the terminal.
        let id = t.run(&template("(trap '' HUP; echo ready; exec sleep 30) & wait"), dir.path());
        let run = t.get(id).unwrap();
        wait_until("ready", || run.screen().screen().contents().contains("ready")).await;
        let pgid = run.pid().unwrap();
        run.kill();
        wait_until("leader exit", || !run.status().is_running()).await;
        assert!(group_alive(pgid), "the child ignored SIGHUP");
        wait_until("group gone", || !group_alive(pgid)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_an_ended_run_kills_its_leftovers() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        // The shell exits by itself once its child ignores the hangup; the child keeps the
        // terminal.
        let id = t.run(
            &template("(trap '' HUP; touch ready; exec sleep 30) & while [ ! -e ready ]; do sleep 0.01; done"),
            dir.path(),
        );
        let run = t.get(id).unwrap();
        let pgid = loop {
            if let Some(p) = run.pid() {
                break p;
            }
            assert!(run.status().is_running());
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        wait_until("exit", || !run.status().is_running()).await;
        assert!(exit(run).success());
        assert!(group_alive(pgid), "the child outlives the shell");
        assert!(t.close(id));
        wait_until("group gone", || !group_alive(pgid)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closing_or_dropping_a_run_kills_it() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let a = t.run(&template("sleep 30"), dir.path());
        let b = t.run(&template("sleep 30"), dir.path());
        // Killed while still starting: killed as soon as it exists.
        let c = t.run(&template("sleep 30"), dir.path());
        t.get(c).unwrap().kill();
        wait_until("running", || {
            t.get(a).unwrap().pid().is_some() && t.get(b).unwrap().pid().is_some()
        })
        .await;
        let (pa, pb) = (t.get(a).unwrap().pid().unwrap(), t.get(b).unwrap().pid().unwrap());
        wait_until("c killed", || !t.get(c).unwrap().status().is_running()).await;
        assert!(exit(t.get(c).unwrap()).killed);
        assert!(t.close(a));
        assert!(!t.close(a));
        wait_until("a gone", || !group_alive(pa)).await;
        assert!(group_alive(pb));
        drop(t);
        wait_until("b gone", || !group_alive(pb)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_kills_running_tasks() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        let a = t.run(&template("trap '' HUP; echo ready; sleep 30"), dir.path());
        let b = t.run(&template("sleep 30"), dir.path());
        let done = t.run(&template("true"), dir.path());
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

    #[tokio::test(flavor = "multi_thread")]
    async fn kill_hangs_up_the_whole_group() {
        let (mut t, _) = tasks();
        let dir = tempfile::tempdir().unwrap();
        // The shell survives SIGHUP (its trap); its background child doesn't. Only a SIGHUP to
        // the group reaches the child before `KILL_GRACE`'s SIGKILL.
        let id = t.run(
            &template("trap 'echo hup' HUP; sleep 30 & echo $! > child; echo ready; while :; do sleep 0.05; done"),
            dir.path(),
        );
        let run = t.get(id).unwrap();
        wait_until("ready", || run.screen().screen().contents().contains("ready")).await;
        let child: i32 = std::fs::read_to_string(dir.path().join("child"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let at = Instant::now();
        run.kill();
        // SAFETY: signal 0 only checks for existence (the reaped child is gone or a zombie of init).
        wait_until("child gone", || unsafe { libc::kill(child, 0) } != 0).await;
        assert!(at.elapsed() < run::KILL_GRACE, "{:?}", at.elapsed());
        wait_until("hup", || run.screen().screen().contents().contains("hup")).await;
        wait_until("exit", || !run.status().is_running()).await;
        assert!(exit(run).signal.unwrap_or_default().contains("Kill"));
    }

    /// Processes in group `pgid`, by `ps`.
    fn child_count(pgid: u32) -> usize {
        let out = std::process::Command::new("ps")
            .args(["-o", "pgid=", "-A"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.trim() == pgid.to_string())
            .count()
    }
}
