//! `fakesh` on a real PTY through `jterm::run`, the way the apps' tests start it: the prompt and
//! bracketed paste mode, a typed line, a paste, Ctrl+C (at a line and during `sleep`), the
//! window size, `exit` and Ctrl+D. Also what makes `cargo test --workspace` build the binary.

use jterm::run::{EnvNames, Run, RunId, Sink, StartOptions, Status};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn start() -> Run {
    let spec = jterm::shell::spec(Path::new(env!("CARGO_BIN_EXE_fakesh")), &std::env::temp_dir(), None);
    let sink: Sink = Arc::new(|_| {});
    let options = StartOptions {
        env: EnvNames {
            ipc: "FAKESH_IPC",
            terminal: "FAKESH_TERMINAL",
        },
        command_ring: 10,
    };
    Run::start(RunId(1), spec, 10, 40, sink, 100usize, options)
}

fn screen(run: &Run) -> String {
    run.screen().screen().contents()
}

async fn wait_for(what: &str, run: &Run, ok: impl Fn(&Run) -> bool) {
    wait_within(Duration::from_secs(30), what, run, ok).await;
}

async fn wait_within(limit: Duration, what: &str, run: &Run, ok: impl Fn(&Run) -> bool) {
    let start = Instant::now();
    while !ok(run) {
        assert!(
            start.elapsed() < limit,
            "timed out waiting for {what}:\n{}",
            screen(run)
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The screen's last non-empty line.
fn last_line(run: &Run) -> String {
    screen(run)
        .lines()
        .rev()
        .map(str::trim_end)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

async fn wait_prompt(run: &Run) {
    wait_for("a prompt", run, |r| last_line(r) == "$").await;
}

async fn exited(run: &Run) -> u32 {
    wait_for("the exit", run, |r| !r.status().is_running()).await;
    match run.status() {
        Status::Exited(exit) => exit.code,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn prompts_runs_pastes_and_survives_ctrl_c() {
    let run = start();
    wait_prompt(&run).await;
    assert!(
        run.screen().screen().bracketed_paste(),
        "bracketed paste is on at the first prompt"
    );

    run.write(b"echo hi-$((1+2)); stty size\n");
    wait_for("the output", &run, |r| screen(r).contains("hi-3\n10 40\n$")).await;

    // A bracketed paste is noted; the same text typed is not. (Each line waits for the prompt
    // before the next, so its typeahead isn't echoed ahead of the prompt.)
    run.write(b"\x1b[200~echo pasted\x1b[201~\n");
    wait_for("the output", &run, |r| screen(r).contains("\npasted\n$")).await;
    run.write(b"pasted\n");
    wait_for("yes", &run, |r| screen(r).contains("\nyes\n$")).await;
    run.write(b"pasted\n");
    wait_for("no", &run, |r| screen(r).contains("\nno\n$")).await;

    // Ctrl+C drops the line and draws a fresh prompt on a new line.
    run.write(b"echo dropped");
    wait_for("the typed line", &run, |r| last_line(r) == "$ echo dropped").await;
    run.write(b"\x03");
    wait_for("a fresh prompt", &run, |r| screen(r).contains("echo dropped^C\n$")).await;
    run.write(b"sleep 120\n");
    wait_for("sleep to start", &run, |r| screen(r).contains("sleep 120")).await;
    run.write(b"\x03");
    // Far below the sleep's 120 s: Ctrl+C has to end it, not let it run out.
    wait_within(Duration::from_secs(20), "the prompt after sleep", &run, |r| {
        last_line(r) == "$"
    })
    .await;
    run.write(b"echo alive-$((2*3))\n");
    wait_for("alive", &run, |r| screen(r).contains("alive-6\n$")).await;

    // Ctrl+D at an empty line ends the shell, status 0.
    run.write(b"\x04");
    assert_eq!(exited(&run).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_takes_a_status() {
    let run = start();
    wait_prompt(&run).await;
    run.write(b"echo leaving; exit 3\n");
    assert_eq!(exited(&run).await, 3);
    assert!(screen(&run).contains("leaving"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_commands_are_not_found() {
    let run = start();
    wait_prompt(&run).await;
    run.write(b"nope; echo status=$?\n");
    wait_for("not found", &run, |r| {
        let text = screen(r);
        text.contains("fakesh: nope: not found") && text.contains("status=127")
    })
    .await;
}
