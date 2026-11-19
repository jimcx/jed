//! kitty_keys_passthrough-b "Done when" (jed's half): a program in a jed shell tab or task tab
//! that pushes the kitty keyboard flags (`fakesh`'s `keyecho`) has the query answered with them,
//! gets Ctrl+Shift+A, Esc and Alt+X as `CSI .. u`, gets a key release only with flag 2 (`:3`), and
//! after it pops the flags gets exactly the bytes a terminal without the protocol sends.
//! (Hand check, not tested: jed started in such a tab finds the protocol without delay.)

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use crate::app::App;
use crate::milestone12_tests::open_app;
use crate::shell_tests::{dirs, draw, shell_app};
use jterm::run::Run;

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) {
    app.handle_test_event(Event::Key(KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    }));
}

const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);

/// The `hex: ..` lines `keyecho` has printed.
fn hex_lines(run: &Run) -> Vec<String> {
    let text = run.screen().screen().contents();
    text.lines()
        .filter_map(|l| l.strip_prefix("hex: "))
        .map(|l| l.trim_end().to_string())
        .collect()
}

fn shell_run(app: &App) -> &Run {
    &app.group().active_shell().unwrap().run
}

async fn shell_with_keyecho(root: &std::path::Path, state: &std::path::Path, flags: &str) -> App {
    let mut app = shell_app(root, state);
    app.tasks.shell = jterm::testing::fake_shell();
    app.new_shell_in_editor();
    draw(&mut app, 80, 30);
    shell_run(&app).write(format!("keyecho {flags}\n").as_bytes());
    assert!(
        app.settle(|a| shell_run(a)
            .screen()
            .screen()
            .contents()
            .contains(&format!("flags={flags}")))
            .await,
        "the query was answered with {flags}:\n{}",
        shell_run(&app).screen().screen().contents()
    );
    app
}

/// Sends a key, then waits for the program to have printed `lines` lines (one key per read, so a
/// later key never joins an earlier one's bytes).
async fn send(
    app: &mut App,
    run: fn(&App) -> &Run,
    lines: usize,
    code: KeyCode,
    mods: KeyModifiers,
    kind: KeyEventKind,
) {
    key(app, code, mods, kind);
    assert!(
        app.settle(|a| hex_lines(run(a)).len() >= lines).await,
        "{lines} lines arrived:\n{}",
        run(app).screen().screen().contents()
    );
}

async fn settle_lines(app: &mut App, run: fn(&App) -> &Run, expected: &[&str]) {
    assert!(
        app.settle(|a| hex_lines(run(a)).len() >= expected.len()).await,
        "{} lines arrived:\n{}",
        expected.len(),
        run(app).screen().screen().contents()
    );
    assert_eq!(
        hex_lines(run(app)),
        expected,
        "{}",
        run(app).screen().screen().contents()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_shell_gets_csi_u_keys_without_releases() {
    let (_d, root, state) = dirs();
    let mut app = shell_with_keyecho(&root, &state, "1").await;
    assert_eq!(shell_run(&app).kitty_flags(), 1);
    let (press, release) = (KeyEventKind::Press, KeyEventKind::Release);
    send(&mut app, shell_run, 1, KeyCode::Char('a'), CTRL_SHIFT, press).await;
    // Flag 1 has no releases: this one reaches nothing, so Esc is the next line.
    key(&mut app, KeyCode::Char('a'), CTRL_SHIFT, release);
    send(&mut app, shell_run, 2, KeyCode::Esc, KeyModifiers::NONE, press).await;
    send(&mut app, shell_run, 3, KeyCode::Char('x'), KeyModifiers::ALT, press).await;
    settle_lines(
        &mut app,
        shell_run,
        &["1b 5b 39 37 3b 36 75", "1b 5b 32 37 75", "1b 5b 31 32 30 3b 33 75"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_shell_release_with_flag_2() {
    let (_d, root, state) = dirs();
    let mut app = shell_with_keyecho(&root, &state, "3").await;
    let (press, release) = (KeyEventKind::Press, KeyEventKind::Release);
    send(&mut app, shell_run, 1, KeyCode::Char('a'), CTRL_SHIFT, press).await;
    send(&mut app, shell_run, 2, KeyCode::Char('a'), CTRL_SHIFT, release).await;
    settle_lines(
        &mut app,
        shell_run,
        &["1b 5b 39 37 3b 36 75", "1b 5b 39 37 3b 36 3a 33 75"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_shell_back_to_xterm_after_the_pop() {
    let (_d, root, state) = dirs();
    let mut app = shell_with_keyecho(&root, &state, "3").await;
    key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Press);
    assert!(
        app.settle(|a| shell_run(a).kitty_flags() == 0 && shell_run(a).screen().screen().contents().contains("bye"))
            .await,
        "the flags were popped:\n{}",
        shell_run(&app).screen().screen().contents()
    );
    // Flags 0 pushed again: a program that asked for nothing gets xterm's bytes, releases none.
    shell_run(&app).write(b"keyecho 0\n");
    assert!(
        app.settle(|a| shell_run(a).screen().screen().contents().contains("flags=0"))
            .await
    );
    send(
        &mut app,
        shell_run,
        1,
        KeyCode::Esc,
        KeyModifiers::NONE,
        KeyEventKind::Press,
    )
    .await;
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Release);
    send(
        &mut app,
        shell_run,
        2,
        KeyCode::Char('x'),
        KeyModifiers::ALT,
        KeyEventKind::Press,
    )
    .await;
    settle_lines(&mut app, shell_run, &["1b", "1b 78"]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_task_tab_gets_csi_u_keys_and_releases() {
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state, Some("[tasks.echo]\ncommand = \"keyecho 3\"\n"));
    app.tasks.shell = jterm::testing::fake_shell();
    app.run_task("echo").unwrap();
    fn task_run(a: &App) -> &Run {
        let id = a.tasks.runs().first().expect("the task").id;
        a.tasks.get(id).unwrap()
    }
    let run = task_run;
    let text = |a: &App| run(a).screen().screen().contents();
    assert!(
        app.settle(|a| text(a).contains("flags=3")).await,
        "the query was answered:\n{}",
        text(&app)
    );
    let (press, release) = (KeyEventKind::Press, KeyEventKind::Release);
    send(&mut app, run, 1, KeyCode::Char('a'), CTRL_SHIFT, press).await;
    send(&mut app, run, 2, KeyCode::Char('a'), CTRL_SHIFT, release).await;
    assert_eq!(
        hex_lines(run(&app)),
        ["1b 5b 39 37 3b 36 75", "1b 5b 39 37 3b 36 3a 33 75"]
    );
}

/// A release never runs a binding: in an editor, releasing the key bound to a command (and any
/// plain key) changes nothing; the press of the same key does run it.
#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_release_runs_no_binding() {
    use crate::command::Command;
    let (_d, root, state) = dirs();
    let mut app = open_app(&root, &state, None);
    app.test_execute(Command::NewUntitledFile);
    app.type_text("hi");
    let rope = |a: &App| a.active_buffer().unwrap().rope.to_string();
    assert_eq!(rope(&app), "hi");
    let combo = app
        .keymap
        .bindings()
        .find(|(_, _, c)| *c == Command::DeleteLine)
        .map(|(combo, _, _)| combo)
        .expect("DeleteLine is bound");
    key(&mut app, combo.code, combo.mods, KeyEventKind::Release);
    key(&mut app, KeyCode::Char('z'), KeyModifiers::NONE, KeyEventKind::Release);
    assert_eq!(rope(&app), "hi", "a release ran nothing");
    key(&mut app, combo.code, combo.mods, KeyEventKind::Press);
    assert_eq!(rope(&app), "", "the press of the same key runs the binding");
}

/// A release of a key bound in the Shell context (its press would run the command) is not sent to
/// a program that asked for releases: Esc's press is the next line.
#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_shell_bound_key_release_stays_out() {
    use crate::config::keymap::Context;
    let (_d, root, state) = dirs();
    let mut app = shell_with_keyecho(&root, &state, "3").await;
    assert_eq!(app.context(), Context::Shell);
    let combo = app
        .keymap
        .bindings()
        .map(|(combo, _, _)| combo)
        .find(|c| {
            let ev = KeyEvent {
                code: c.code,
                modifiers: c.mods,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            };
            c.code != KeyCode::Esc && app.keymap.lookup(&ev, Context::Shell).is_some()
        })
        .expect("a key bound in the Shell context");
    key(&mut app, combo.code, combo.mods, KeyEventKind::Release);
    send(
        &mut app,
        shell_run,
        1,
        KeyCode::Esc,
        KeyModifiers::NONE,
        KeyEventKind::Press,
    )
    .await;
    settle_lines(&mut app, shell_run, &["1b 5b 32 37 75"]).await;
}
