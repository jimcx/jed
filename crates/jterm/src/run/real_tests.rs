//! Real-shell tests of `Run`: each starts `/bin/sh -c <command>` in a PTY through a small
//! `Tasks` stand-in (what jed's `Tasks` does with a template: build the `Spec`, number the runs).

use super::*;
use jtui::scrollbar::Scrollable;
use std::path::Path;

const TEST_ENV: EnvNames = EnvNames {
    ipc: "TEST_IPC",
    terminal: "TEST_TERMINAL",
};

/// The environment every run gets, as jed's templates add it.
const DEFAULT_ENV: [(&str, &str); 2] = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];

struct Task {
    command: String,
    cwd: Option<String>,
}

struct Template {
    task: Task,
}

fn template(command: &str) -> Template {
    Template {
        task: Task {
            command: command.into(),
            cwd: None,
        },
    }
}

/// The runs a test started, with the sink's events.
struct Tasks {
    size: (u16, u16),
    runs: Vec<Run>,
    next_id: u64,
    sink: Sink,
}

impl Tasks {
    fn run(&mut self, template: &Template, root: &Path, scrollback: impl Into<Scrollback>) -> RunId {
        let id = RunId(self.next_id);
        self.next_id += 1;
        let spec = Spec {
            name: "t".into(),
            command: template.task.command.clone(),
            argv: vec!["/bin/sh".into(), "-c".into(), template.task.command.clone().into()],
            cwd: match &template.task.cwd {
                Some(c) => root.join(c),
                None => root.to_path_buf(),
            },
            env: DEFAULT_ENV
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            start_gate: None,
        };
        let (rows, cols) = self.size;
        let options = StartOptions {
            env: TEST_ENV,
            command_ring: 50,
        };
        self.runs
            .push(Run::start(id, spec, rows, cols, self.sink.clone(), scrollback, options));
        id
    }

    fn get(&self, id: RunId) -> Option<&Run> {
        self.runs.iter().find(|r| r.id == id)
    }

    fn close(&mut self, id: RunId) -> bool {
        let before = self.runs.len();
        self.runs.retain(|r| r.id != id);
        self.runs.len() != before
    }
}

/// A `Tasks` whose events land in the returned list.
fn tasks() -> (Tasks, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let t = Tasks {
        size: (24, 80),
        runs: Vec::new(),
        next_id: 1,
        sink: Arc::new(move |e| sink_events.lock().unwrap().push(e)),
    };
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

/// Whether process `pid` is alive. A zombie isn't: a child killed by the hang-up stays one until
/// its parent reaps it, which a shell in a loop may not do before `KILL_GRACE` (seen on macOS).
fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    !stat.trim().is_empty() && !stat.trim_start().starts_with('Z')
}

/// Whether any live process is in group `pgid`. Zombies don't count: where nothing reaps orphans
/// (a container whose pid 1 is not an init) a killed process stays a zombie, which `kill(-pgid, 0)`
/// still sees.
fn group_alive(pgid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(-(pgid as i32), 0) } != 0 {
        return false;
    }
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return true;
    };
    procs.flatten().any(|e| {
        // /proc/<pid>/stat: `pid (comm) state ppid pgrp ...`; comm may hold spaces and parens.
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
            return false;
        };
        let mut rest = stat.rsplit_once(')').map_or("", |(_, r)| r).split_whitespace();
        let state = rest.next();
        let pgrp = rest.nth(1).and_then(|g| g.parse::<u32>().ok());
        pgrp == Some(pgid) && state != Some("Z")
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn output_and_exit_code() {
    let (mut t, events) = tasks();
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(
        &template("printf 'hello\\n\\033[31mred\\033[0m\\n'; exit 3"),
        dir.path(),
        super::SCROLLBACK,
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

/// Input written before the process exists (status `Starting`) is queued and reaches it, in
/// order, ahead of later input; a run that fails to start takes none.
#[tokio::test(flavor = "multi_thread")]
async fn input_written_while_starting_is_sent_once_running() {
    let (mut t, _) = tasks();
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(
        &template("read -r a; read -r b; echo \"got $a $b\""),
        dir.path(),
        super::SCROLLBACK,
    );
    let run = t.get(id).unwrap();
    assert_eq!(run.status(), Status::Starting);
    assert!(run.write(b"one\n"));
    wait_until("running", || run.status() == Status::Running).await;
    assert!(run.write(b"two\n"));
    wait_until("exit", || !run.status().is_running()).await;
    assert!(exit(run).success());
    assert!(run.screen().screen().contents().ends_with("got one two"));
    assert!(!run.write(b"late\n"), "ended");

    let mut failing = template("true");
    failing.task.cwd = Some("missing".into());
    let id = t.run(&failing, dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    // Queued while starting; the failed start may already have finished under load.
    let queued = run.write(b"x");
    assert!(queued || matches!(run.status(), Status::Failed(_)));
    wait_until("failed", || matches!(run.status(), Status::Failed(_))).await;
    assert!(!run.write(b"x"));
}

/// A paste far bigger than the PTY's input buffer, to a process that never reads it: `write`
/// returns at once (the writer thread blocks, not the caller), and the run still dies.
#[tokio::test(flavor = "multi_thread")]
async fn writing_to_a_process_that_doesnt_read_never_blocks() {
    let (mut t, _) = tasks();
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(&template("echo ready; sleep 30"), dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    wait_until("ready", || run.screen().screen().contents().contains("ready")).await;
    let big = vec![b'x'; 4 << 20];
    let started = Instant::now();
    for _ in 0..4 {
        assert!(run.write(&big));
    }
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    let pid = run.pid().unwrap();
    run.kill();
    wait_until("exit", || !run.status().is_running()).await;
    assert!(exit(run).killed);
    assert!(!run.write(b"x"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while group_alive(pid) {
        assert!(Instant::now() < deadline, "the group outlived the kill");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn input_reaches_the_process() {
    let (mut t, _) = tasks();
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(
        &template("printf 'name? '; read n; echo \"hi $n\""),
        dir.path(),
        super::SCROLLBACK,
    );
    let run = t.get(id).unwrap();
    wait_until("prompt", || run.screen().screen().contents().contains("name?")).await;
    let key = |c| crate::keys::encode(&crossterm::event::KeyEvent::from(c), Default::default()).unwrap();
    for c in "bob".chars() {
        assert!(run.write(&key(crossterm::event::KeyCode::Char(c))));
    }
    assert!(run.write(&key(crossterm::event::KeyCode::Enter)));
    wait_until("exit", || !run.status().is_running()).await;
    assert_eq!(run.screen().screen().contents(), "name? bob\nhi bob");
    assert!(!run.write(b"x"), "nothing to write to once ended");
    run.append_line("Task 't' exited with code 0");
    assert_eq!(
        run.screen().screen().contents(),
        "name? bob\nhi bob\nTask 't' exited with code 0"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_interrupts_the_foreground_process() {
    let (mut t, _) = tasks();
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(&template("echo ready; sleep 30"), dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    wait_until("ready", || run.screen().screen().contents().contains("ready")).await;
    // A Ctrl+C between the shell's `echo` and its start of `sleep` can be lost (seen with
    // dash, and under load): wait until `sleep` runs in the task's process group.
    let group = run.pid().expect("running").to_string();
    let sleeping = || {
        std::process::Command::new("pgrep")
            .args(["-g", &group, "-x", "sleep"])
            .output()
            .is_ok_and(|o| o.status.success())
    };
    wait_until("sleep running", sleeping).await;
    let ctrl_c = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('c'),
        crossterm::event::KeyModifiers::CONTROL,
    );
    assert!(run.write(&crate::keys::encode(&ctrl_c, Default::default()).unwrap()));
    let screen = || run.screen().screen().contents();
    let deadline = Instant::now() + Duration::from_secs(5);
    while run.status().is_running() {
        assert!(
            Instant::now() < deadline,
            "still {:?} 5 s after Ctrl+C; screen {:?}; sleep still running {}",
            run.status(),
            screen(),
            sleeping()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let e = exit(run);
    assert!(!e.success() && !e.killed, "{e:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn scrolling_through_the_scrollback() {
    let (mut t, _) = tasks();
    t.size = (5, 40);
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(&template("seq 1 50"), dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    wait_until("exit", || !run.status().is_running()).await;
    run.scroll(-3);
    assert_eq!(run.screen().screen().scrollback(), 3);
    assert!(run.screen().screen().contents().starts_with("44\n"));
    run.scroll(1);
    assert_eq!(run.screen().screen().scrollback(), 2);
    run.scroll(10);
    assert_eq!(run.screen().screen().scrollback(), 0);
    run.scroll_to(true);
    assert_eq!(run.screen().screen().scrollback(), 46);
    run.scroll_to(false);
    assert_eq!(run.screen().screen().scrollback(), 0);
}

/// The terminal's scroll bar: its `max_top` is the kept lines (probed), `top == max_top` on
/// the live screen and `top == 0` at the oldest output, and `set_top` moves the offset,
/// clearing a selection.
#[tokio::test(flavor = "multi_thread")]
async fn terminal_scroll_bar_extent_and_set_top() {
    let (mut t, _) = tasks();
    t.size = (5, 40);
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(&template("seq 1 50"), dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    wait_until("exit", || !run.status().is_running()).await;

    let (back, max, rows) = run.scrollbar_extent();
    assert_eq!((back, rows), (0, 5), "live screen, five rows");
    {
        let mut p = run.screen();
        p.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(p.screen().scrollback(), max, "the probe's max is the kept lines");
        p.screen_mut().set_scrollback(0);
    }

    let scroll = run.scrollable().scroll();
    assert_eq!((scroll.max_top, scroll.height), (max, rows));
    assert_eq!(scroll.top, max, "live screen: thumb at the bottom");

    run.scroll_to(true);
    assert_eq!(run.scrollable().scroll().top, 0, "oldest output: thumb at the top");

    run.scroll_to(false);
    run.set_selection(select::Selection {
        anchor: (0, 0),
        head: (0, 0),
    });
    assert!(run.selection().is_some());
    let mut bar = run.scrollable();
    jtui::scrollbar::Scrollable::set_top(&mut bar, 3);
    assert_eq!(run.screen().screen().scrollback(), max - 3);
    assert!(run.selection().is_some(), "the selection moves with its text");
    jtui::scrollbar::Scrollable::set_top(&mut bar, max);
    assert_eq!(run.screen().screen().scrollback(), 0);
    jtui::scrollbar::Scrollable::set_top(&mut bar, 0);
    assert_eq!(run.screen().screen().scrollback(), max, "back to the oldest output");
}

/// No scrollback (the alternate screen, a run that printed nothing): `max == 0`, so the bar
/// shows no thumb.
#[tokio::test(flavor = "multi_thread")]
async fn terminal_scroll_bar_without_scrollback_shows_no_thumb() {
    let (mut t, _) = tasks();
    t.size = (5, 40);
    let dir = tempfile::tempdir().unwrap();
    let id = t.run(&template("true"), dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    wait_until("exit", || !run.status().is_running()).await;
    let (back, max, rows) = run.scrollbar_extent();
    assert_eq!((back, max, rows), (0, 0, 5));
    let scroll = run.scrollable().scroll();
    assert_eq!((scroll.max_top, scroll.shown), (0, false), "no scrollback: no thumb");
}

// FLAKE: 2026-10-28, failed under load (load avg ~14) when run in the full suite, passes alone
#[tokio::test(flavor = "multi_thread")]
async fn output_events_are_throttled() {
    let (mut t, events) = tasks();
    let dir = tempfile::tempdir().unwrap();
    // ~2000 separate writes over ~100 ms or more.
    let id = t.run(
        &template("i=0; while [ $i -lt 2000 ]; do echo line $i; i=$((i+1)); done"),
        dir.path(),
        super::SCROLLBACK,
    );
    let run = t.get(id).unwrap();
    let started = Instant::now();
    wait_until("exit", || !run.status().is_running()).await;
    let elapsed = started.elapsed();
    let contents = run.screen().screen().contents();
    assert!(
        contents.ends_with("line 1999"),
        "FLAKE output_events_are_throttled: screen does not end with `line 1999` after {elapsed:?}; status {:?}, \
         screen tail {:?}",
        run.status(),
        contents
            .chars()
            .rev()
            .take(80)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    );
    let outputs = events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| **e == Event::Output(id))
        .count();
    let most = elapsed.as_millis() / super::FRAME.as_millis() + 2;
    if !(1..=most as usize).contains(&outputs) {
        let all = events.lock().unwrap();
        eprintln!(
            "FLAKE output_events_are_throttled: {outputs} output events (allowed 1..={most}) in {elapsed:?}, \
             FRAME {:?}, {} events in all, the first 40: {:?}",
            super::FRAME,
            all.len(),
            all.iter().take(40).collect::<Vec<_>>(),
        );
    }
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
    let id = t.run(&template("seq 1 50"), dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    wait_until("exit", || !run.status().is_running()).await;
    let mut p = run.screen();
    assert_eq!(p.screen().contents(), "47\n48\n49\n50");
    p.screen_mut().set_scrollback(usize::MAX);
    assert_eq!(p.screen().scrollback(), 46, "scrolled to the top");
    assert!(p.screen().contents().starts_with("1\n2\n"));
}

/// The process starts as portable-pty's `pre_exec` left it (now `posix_spawn`'s job on Linux):
/// leader of its own session and process group, the PTY its controlling terminal, no signal
/// blocked (seen in what it runs: bash blocks SIGCHLD in itself while it waits), only stdin /
/// stdout / stderr open, in the run's folder.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn the_process_leads_a_session_on_its_pty() {
    let (mut t, _) = tasks();
    let dir = tempfile::tempdir().unwrap();
    let script = r#"set -- $(cat /proc/$$/stat)
[ "$1" = "$5" ] && [ "$1" = "$6" ] && echo LEADER
[ "$7" != 0 ] && (exec 3</dev/tty) 2>/dev/null && echo CTTY
grep -q '^SigBlk:[[:space:]]*0*$' /proc/self/status && echo UNBLOCKED
ls -C /proc/$$/fd
pwd -P"#;
    let id = t.run(&template(script), dir.path(), super::SCROLLBACK);
    let run = t.get(id).unwrap();
    wait_until("exit", || !run.status().is_running()).await;
    let out = run.screen().screen().contents();
    let want = format!(
        "LEADER\nCTTY\nUNBLOCKED\n0  1  2\n{}",
        dir.path().canonicalize().unwrap().display()
    );
    assert_eq!(out, want);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_cwd_fails_to_start() {
    let (mut t, events) = tasks();
    let dir = tempfile::tempdir().unwrap();
    let mut tpl = template("true");
    tpl.task.cwd = Some("nope".into());
    let id = t.run(&tpl, dir.path(), super::SCROLLBACK);
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
        &template(
            "trap 'stty size; exit 0' WINCH; echo ready; i=0; while [ $i -lt 500 ]; do sleep 0.01; i=$((i+1)); done",
        ),
        dir.path(),
        super::SCROLLBACK,
    );
    let run = t.get(id).unwrap();
    wait_until("ready", || run.screen().screen().contents().contains("ready")).await;
    run.resize(10, 50);
    assert_eq!(select::size(run.screen().screen()), (10, 50));
    wait_until("exit", || !run.status().is_running()).await;
    assert_eq!(run.screen().screen().contents(), "ready\n10 50");
    assert!(exit(run).success());
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_ends_the_whole_group() {
    let (mut t, _) = tasks();
    let dir = tempfile::tempdir().unwrap();
    // A background child too: the kill must reach it.
    let id = t.run(
        &template("sleep 30 & sleep 30; echo never"),
        dir.path(),
        super::SCROLLBACK,
    );
    let run = t.get(id).unwrap();
    wait_until("running", || run.pid().is_some()).await;
    let pgid = run.pid().unwrap();
    // Let the shell start both sleeps.
    wait_until("children", || child_count(pgid) >= 2).await;
    let at = Instant::now();
    run.kill();
    wait_until("exit", || !run.status().is_running()).await;
    assert!(at.elapsed() < super::KILL_GRACE, "SIGHUP was enough");
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
    let id = t.run(
        &template("trap '' HUP; echo ready; sleep 30"),
        dir.path(),
        super::SCROLLBACK,
    );
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
    let id = t.run(
        &template("(trap '' HUP; echo ready; exec sleep 30) & wait"),
        dir.path(),
        super::SCROLLBACK,
    );
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
        super::SCROLLBACK,
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
    let a = t.run(&template("sleep 30"), dir.path(), super::SCROLLBACK);
    let b = t.run(&template("sleep 30"), dir.path(), super::SCROLLBACK);
    // Killed while still starting: killed as soon as it exists.
    let c = t.run(&template("sleep 30"), dir.path(), super::SCROLLBACK);
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

// Was flaky (2026-11-01 under load; 2026-11-15 on macOS CI: "hup" shown, child gone after 1.15 s):
// `kill(child, 0)` also sees the hung-up child as a zombie its looping shell hadn't reaped, so
// "gone" now means "not alive or a zombie".
#[tokio::test(flavor = "multi_thread")]
async fn kill_hangs_up_the_whole_group() {
    let (mut t, _) = tasks();
    let dir = tempfile::tempdir().unwrap();
    // The shell survives SIGHUP (its trap); its background child doesn't. Only a SIGHUP to
    // the group reaches the child before `KILL_GRACE`'s SIGKILL.
    let id = t.run(
        &template("trap 'echo hup' HUP; sleep 30 & echo $! > child; echo ready; while :; do sleep 0.05; done"),
        dir.path(),
        super::SCROLLBACK,
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
    wait_until("child gone", || !process_alive(child)).await;
    let elapsed = at.elapsed();
    if elapsed >= super::KILL_GRACE {
        eprintln!(
            "FLAKE kill_hangs_up_the_whole_group: child {child} gone after {elapsed:?} (grace {:?}), status {:?}, screen {:?}",
            super::KILL_GRACE,
            run.status(),
            run.screen().screen().contents()
        );
    }
    assert!(
        elapsed < super::KILL_GRACE,
        "FLAKE: {elapsed:?} >= {:?}",
        super::KILL_GRACE
    );
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

/// Starts `command` under `/bin/sh -c` with exactly `env` in the spec and returns the run.
fn start_with(id: u64, command: &str, env: &[(&str, &str)], options: StartOptions) -> (Run, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let spec = Spec {
        name: "t".into(),
        command: command.into(),
        argv: vec!["/bin/sh".into(), "-c".into(), command.into()],
        cwd: dir.path().to_path_buf(),
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        start_gate: None,
    };
    let sink: Sink = Arc::new(|_| {});
    (
        Run::start(RunId(id), spec, 24, 80, sink, super::SCROLLBACK, options),
        dir,
    )
}

/// The first screen line of a finished run.
async fn first_line(run: &Run) -> String {
    wait_until("exit", || !run.status().is_running()).await;
    run.screen()
        .screen()
        .contents()
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// With the app's IPC variable in the spec, the run also gets the terminal variable set to its
/// id (the names come from `EnvNames`, not from the engine), replacing one the spec carries.
#[tokio::test(flavor = "multi_thread")]
async fn env_names_terminal_var_follows_the_ipc_var() {
    let names = EnvNames {
        ipc: "ALICE_SOCK",
        terminal: "ALICE_TERM",
    };
    let options = StartOptions {
        env: names,
        command_ring: 50,
    };
    let (run, _dir) = start_with(
        42,
        "echo \"[$ALICE_SOCK][$ALICE_TERM][${TEST_IPC-unset}][${TEST_TERMINAL-unset}]\"",
        &[("ALICE_SOCK", "/s"), ("ALICE_TERM", "stale")],
        options,
    );
    assert_eq!(first_line(&run).await, "[/s][42][unset][unset]");
    // Without the IPC variable the terminal variable is not added.
    let (run, _dir) = start_with(43, "echo \"[${ALICE_SOCK-unset}][${ALICE_TERM-unset}]\"", &[], options);
    assert_eq!(first_line(&run).await, "[unset][unset]");
}

/// An IPC or terminal variable leaked from the parent process is stripped, not inherited.
#[tokio::test(flavor = "multi_thread")]
async fn env_names_inherited_vars_are_stripped() {
    let names = EnvNames {
        ipc: "JTERM_LEAK_SOCK",
        terminal: "JTERM_LEAK_TERM",
    };
    // SAFETY: names unique to this test, so nothing else reads them. The spawn path reads the
    // parent environment only through std (`CommandBuilder::from_argv` snapshots
    // `std::env::vars_os`, which holds std's env lock; the child is a std `Command` with
    // `env_clear` and an explicit environment), so concurrent spawns in other tests don't race.
    unsafe {
        std::env::set_var(names.ipc, "outer-sock");
        std::env::set_var(names.terminal, "outer-term");
    }
    let options = StartOptions {
        env: names,
        command_ring: 50,
    };
    let (run, _dir) = start_with(
        1,
        "echo \"[${JTERM_LEAK_SOCK-unset}][${JTERM_LEAK_TERM-unset}]\"",
        &[],
        options,
    );
    assert_eq!(first_line(&run).await, "[unset][unset]");
}

/// The ring size is `StartOptions::command_ring`, and a record's wall-clock start is now-ish.
#[tokio::test(flavor = "multi_thread")]
async fn command_ring_size_is_the_start_option() {
    let options = StartOptions {
        env: TEST_ENV,
        command_ring: 3,
    };
    let before = SystemTime::now();
    let script = "for i in 1 2 3 4 5; do printf '\\033]133;B\\007x\\r\\n\\033]133;D;0\\007'; done";
    let (run, _dir) = start_with(1, script, &[], options);
    wait_until("exit", || !run.status().is_running()).await;
    wait_until("five commands parsed", || {
        run.commands().last().is_some_and(|c| c.id == 5 && c.finished.is_some())
    })
    .await;
    let cmds = run.commands();
    assert_eq!(cmds.iter().map(|c| c.id).collect::<Vec<_>>(), [3, 4, 5]);
    let after = SystemTime::now();
    for c in &cmds {
        assert!(c.started_at >= before && c.started_at <= after, "{:?}", c.started_at);
    }
}
