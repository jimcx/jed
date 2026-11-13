//! End-to-end tests of jmux_commands-a (the history model's feed, the install command and the
//! missing-integration notice), driving the server `App` headlessly with real `/bin/sh` shells.
//! The integration is faked by typing a line that prints the codes the snippets would (`133;B`,
//! `633;E;<command>`, `133;D;<exit>`), so no init file is involved.
//!
//! Covered so far (`jmux_commands_a_*`): a command shows as a running row with its terminal's title
//! and then finished with its exit code (`✓` / `✗`); rows of two terminals interleave by time; a
//! closed terminal's rows stay, marked (a running one ended); the list is capped at
//! `command_history`; the first shell without the integration puts a one-line notice in the status
//! bar until a key, an up-to-date or snippet-less shell none; `Command::InstallShellIntegration`
//! writes the snippet and the init line in temporary directories (never the real home).

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers};
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, COMMAND_HISTORY, Config};
use crate::command::Command;
use crate::integration::Dirs;
use crate::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};

const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    app: App,
    events: UnboundedReceiver<AppEvent>,
    conn_rx: Receiver<ServerMsg>,
    terminal: Terminal<StreamBackend>,
    screen: vt100::Parser,
}

fn dirs(root: &Path) -> Dirs {
    Dirs {
        home: root.join("home"),
        config_home: root.join("home/.config"),
        state: root.join("state"),
    }
}

impl Harness {
    async fn new(program: &str, dirs: Option<Dirs>, command_history: usize) -> Harness {
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let config = Config {
            name: "work".into(),
            socket: PathBuf::new(),
            program: program.into(),
            cwd: std::env::temp_dir(),
            open_url: |_| Ok(()),
            dirs,
            command_history,
        };
        let mut app = App::new(config, tx);
        app.start();
        let (ctx, conn_rx) = tokio::sync::mpsc::channel(256);
        let writer = tokio::spawn(std::future::pending::<()>());
        app.handle(AppEvent::Connected {
            conn: 1,
            tx: ctx,
            writer,
        });
        let msg = ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            rows: 24,
            cols: 100,
            truecolor: true,
            term: "xterm".into(),
        };
        app.handle(AppEvent::Client { conn: 1, msg });
        Harness {
            app,
            events,
            conn_rx,
            terminal: Terminal::new(StreamBackend::new(100, 24)).unwrap(),
            screen: vt100::Parser::new(24.try_into().unwrap(), 100.try_into().unwrap(), 0),
        }
    }

    fn key(&mut self, code: KeyCode) {
        let msg = ClientMsg::Input(Input::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        self.app.handle(AppEvent::Client { conn: 1, msg });
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.key(KeyCode::Char(ch));
        }
        self.key(KeyCode::Enter);
    }

    /// Types a line that runs `sleep secs` as a command the integration would have reported as
    /// `name`, finishing with `exit`.
    fn fake(&mut self, name: &str, secs: u32, exit: i32) {
        self.type_text(&format!(
            "printf '\\033]133;B\\007\\033]633;E;{name}\\007'; sleep {secs}; printf '\\033]133;D;{exit}\\007'"
        ));
    }

    async fn step(&mut self) {
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(10), self.events.recv()).await {
            self.app.handle(ev);
            while let Ok(ev) = self.events.try_recv() {
                self.app.handle(ev);
            }
        }
        if self.app.needs_redraw {
            self.app.redraw(&mut self.terminal).unwrap();
        }
        while let Ok(msg) = self.conn_rx.try_recv() {
            if let ServerMsg::Output(bytes) = msg {
                self.screen.process(&bytes);
            }
        }
    }

    async fn settle(&mut self, what: &str, done: impl Fn(&Harness) -> bool) {
        let deadline = Instant::now() + WAIT;
        self.step().await;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}:\n{}",
                self.screen.screen().contents()
            );
            self.step().await;
        }
    }

    /// `(label, glyph, title, closed)` of every row, in list order.
    fn rows(&self) -> Vec<(String, char, String, bool)> {
        self.app
            .history
            .rows()
            .iter()
            .map(|r| (r.label().to_string(), r.glyph(), r.title.clone(), r.closed))
            .collect()
    }

    fn status_row(&self) -> String {
        self.screen.screen().rows(0, 100).last().unwrap_or_default()
    }
}

fn row(label: &str, glyph: char, title: &str, closed: bool) -> (String, char, String, bool) {
    (label.into(), glyph, title.into(), closed)
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_a_command_is_a_running_row_then_finished_with_its_exit_code() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.fake("slow", 2, 0);
    h.settle("a running row", |h| h.rows() == [row("slow", '●', "sh", false)])
        .await;
    h.settle("it finished", |h| h.rows() == [row("slow", '✓', "sh", false)])
        .await;
    let r = &h.app.history.rows()[0];
    assert!(r.duration_text(Instant::now()).ends_with('s'));
    assert!(r.record.finished.unwrap() - r.record.started >= Duration::from_secs(2));
    h.fake("bad", 0, 1);
    h.settle("a failed row", |h| {
        h.rows() == [row("bad", '✗', "sh", false), row("slow", '✓', "sh", false)]
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_rows_of_two_terminals_interleave_by_time() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.fake("one", 0, 0);
    h.settle("one", |h| h.rows().len() == 1 && !h.app.history.rows()[0].running())
        .await;
    h.app.execute(Command::NewTerminal);
    h.fake("two", 0, 0);
    h.settle("two", |h| h.rows().len() == 2 && !h.app.history.rows()[0].running())
        .await;
    h.app.execute(Command::PrevTab);
    h.fake("three", 0, 3);
    h.settle("three", |h| h.rows().len() == 3 && !h.app.history.rows()[0].running())
        .await;
    assert_eq!(
        h.rows(),
        [
            row("three", '✗', "sh", false),
            row("two", '✓', "sh (2)", false),
            row("one", '✓', "sh", false)
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_a_closed_terminals_rows_stay_marked_and_a_running_one_ends() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.fake("keep", 0, 0);
    h.settle("keep", |h| h.rows().len() == 1 && !h.app.history.rows()[0].running())
        .await;
    h.app.execute(Command::NewTerminal);
    h.fake("done", 0, 0);
    h.settle("done", |h| h.rows().len() == 2 && !h.app.history.rows()[0].running())
        .await;
    h.fake("hang", 30, 0);
    h.settle("hang runs", |h| {
        h.rows().first().is_some_and(|r| r.0 == "hang" && r.1 == '●')
    })
    .await;
    // A running command asks first.
    h.app.execute(Command::CloseTerminal);
    assert!(h.app.confirm.is_some());
    h.app.execute(Command::ConfirmYes);
    h.settle("the tab closed", |h| h.app.groups.tab_count() == 1).await;
    assert_eq!(
        h.rows(),
        [
            row("hang", '·', "sh (2)", true),
            row("done", '✓', "sh (2)", true),
            row("keep", '✓', "sh", false)
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_the_list_holds_at_most_command_history_rows() {
    let mut h = Harness::new("/bin/sh", None, 3).await;
    h.type_text(
        "for i in 1 2 3 4 5; do printf '\\033]133;B\\007\\033]633;E;c%s\\007' $i; printf '\\033]133;D;0\\007'; done",
    );
    h.settle("the newest three", |h| {
        h.rows().first().is_some_and(|r| r.0 == "c5" && r.1 == '✓')
    })
    .await;
    let labels: Vec<String> = h.rows().into_iter().map(|r| r.0).collect();
    assert_eq!(labels, ["c5", "c4", "c3"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_the_first_shell_without_the_integration_gets_a_notice_until_a_key() {
    let tmp = tempfile::tempdir().unwrap();
    // The shell's base name picks the snippet: a `zsh` that is really `/bin/sh`.
    let zsh = tmp.path().join("zsh");
    symlink("/bin/sh", &zsh).unwrap();
    let mut h = Harness::new(zsh.to_str().unwrap(), Some(dirs(tmp.path())), COMMAND_HISTORY).await;
    h.settle("the notice", |h| {
        h.status_row().contains("Shell integration for zsh isn't installed")
    })
    .await;
    assert!(h.status_row().contains("jmux install-shell-integration"));
    h.key(KeyCode::Char(' '));
    h.settle("the notice is gone", |h| !h.status_row().contains("Shell integration"))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_a_shell_with_no_snippet_or_no_directories_gets_no_notice() {
    let tmp = tempfile::tempdir().unwrap();
    for d in [Some(dirs(tmp.path())), None] {
        let mut h = Harness::new("/bin/sh", d, COMMAND_HISTORY).await;
        let until = Instant::now() + Duration::from_millis(500);
        while Instant::now() < until {
            h.step().await;
        }
        assert_eq!(h.app.notice, None);
    }
    assert!(!tmp.path().join("state").exists(), "a check writes nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_install_writes_the_snippet_and_the_init_line_once_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let zsh = tmp.path().join("zsh");
    symlink("/bin/sh", &zsh).unwrap();
    let d = dirs(tmp.path());
    let mut h = Harness::new(zsh.to_str().unwrap(), Some(d.clone()), COMMAND_HISTORY).await;
    h.settle("the missing notice", |h| h.app.notice.is_some()).await;
    h.app.execute(Command::InstallShellIntegration);
    h.settle("the install notice", |h| {
        h.status_row().contains("Added to ~/.zshrc") || h.app.notice.as_deref().is_some_and(|n| n.starts_with("Added"))
    })
    .await;
    let rc = std::fs::read_to_string(d.home.join(".zshrc")).unwrap();
    assert!(rc.contains("shell-integration.zsh"), "{rc}");
    let snippet = std::fs::read_to_string(d.state.join("shell-integration.zsh")).unwrap();
    assert!(snippet.contains("JMUX"), "the guard is jmux's");
    // Again: the snippet is refreshed, the line is not added twice.
    h.app.execute(Command::InstallShellIntegration);
    h.settle("the second notice", |h| {
        h.app.notice.as_deref().is_some_and(|n| n.starts_with("Updated"))
    })
    .await;
    assert_eq!(std::fs::read_to_string(d.home.join(".zshrc")).unwrap(), rc);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_closing_a_tab_keeps_a_finish_the_app_had_not_seen() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.app.execute(Command::NewTerminal);
    h.fake("late", 0, 7);
    // Wait on the run itself, not the app: no event is handled meanwhile, so the history has
    // not seen the finish when the tab closes.
    let until = Instant::now() + WAIT;
    loop {
        let done = h.app.groups.tabs().last().unwrap().run.commands();
        if done
            .iter()
            .any(|r| r.command.as_deref() == Some("late") && r.exit == Some(7))
        {
            break;
        }
        assert!(Instant::now() < until, "the run never finished the command");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    h.app.execute(Command::CloseTerminal);
    assert_eq!(h.app.groups.tab_count(), 1, "the tab closed without a question");
    assert_eq!(h.rows(), [row("late", '✗', "sh (2)", true)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_the_integration_is_checked_once_per_session() {
    let tmp = tempfile::tempdir().unwrap();
    let zsh = tmp.path().join("zsh");
    symlink("/bin/sh", &zsh).unwrap();
    let mut h = Harness::new(zsh.to_str().unwrap(), Some(dirs(tmp.path())), COMMAND_HISTORY).await;
    h.settle("the notice", |h| h.app.notice.is_some()).await;
    h.key(KeyCode::Char(' '));
    assert_eq!(h.app.notice, None);
    // More terminals start; a second check would show the notice again.
    h.app.execute(Command::NewTerminal);
    h.app.execute(Command::NewTerminal);
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        h.step().await;
    }
    assert_eq!(h.app.groups.tab_count(), 3);
    assert_eq!(h.app.notice, None, "the check ran again");
}
