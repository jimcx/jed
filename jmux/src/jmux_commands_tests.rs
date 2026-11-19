//! End-to-end tests of jmux_commands-a (the history model's feed, the install command and the
//! missing-integration notice), driving the server `App` headlessly with real `/bin/sh` shells.
//! The integration is faked by typing a line that prints the codes the snippets would (`133;B`,
//! `633;E;<command>`, `133;D;<exit>`), so no init file is involved.
//!
//! Covered by `jmux_commands_b_*` (the side bar): a command is a running row with its terminal's
//! title, then `✓` with its duration (`..._a_command_is_a_running_row...`); `false` shows `✗`
//! (`..._a_failed_command_shows_a_cross`); rows from two terminals interleave by time
//! (`..._rows_of_two_terminals_interleave...`); clicking an old row in the other group focuses that
//! terminal and scrolls its command to the top (`..._clicking_an_old_row...`), a command the
//! scrollback lost only focuses with a notice (`..._a_lost_command...`); a closed terminal's rows are
//! dimmed and do nothing (`..._a_closed_terminals_rows...`); the filter narrows the list
//! (`..._the_filter_narrows...`); the list holds at most `command_history` rows
//! (`..._the_list_holds_at_most...`); without the integration the bar says how to install it
//! (`..._without_the_integration...`); the keys (`..._keys_...`), the border drag
//! (`..._dragging_the_border...`), the wheel and scroll bar (`..._the_wheel_and_scroll_bar...`) and
//! the tick that redraws only while the bar shows rows (`..._the_tick_...`).
//!
//! Covered by `jmux_commands_a_*`: a command shows as a running row with its terminal's title
//! and then finished with its exit code (`✓` / `✗`); rows of two terminals interleave by time; a
//! closed terminal's rows stay, marked (a running one ended); the list is capped at
//! `command_history`; the first shell without the integration puts a one-line notice in the status
//! bar until a key, an up-to-date or snippet-less shell none; `Command::InstallShellIntegration`
//! writes the snippet and the init line in temporary directories (never the real home).

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, Config};
use crate::command::{Command, SideCommand};
use crate::integration::Dirs;
use crate::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use crate::settings::COMMAND_HISTORY;

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
            settings: crate::settings::Settings {
                command_history,
                ..Default::default()
            },
            settings_path: None,
            settings_errors: Vec::new(),
            keybindings_path: None,
            keymap: None,
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
            touch: false,
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

    fn click(&mut self, column: u16, row: u16) {
        self.app
            .send_mouse(MouseEventKind::Down(MouseButton::Left), column, row);
        self.app.send_mouse(MouseEventKind::Up(MouseButton::Left), column, row);
    }

    /// Shows the side bar and waits for it to be drawn.
    async fn show_side(&mut self) {
        self.app.run(Command::Side(SideCommand::Toggle));
        self.settle("the side bar", |h| h.app.hit.side.is_some()).await;
    }

    /// The side bar's columns of each screen row (the bar is 32 wide until dragged).
    fn side_rows(&self) -> Vec<String> {
        let w = usize::from(self.app.side.width_in(100));
        self.screen
            .screen()
            .rows(0, 100)
            .map(|r| r.chars().take(w).collect::<String>())
            .collect()
    }

    fn side_has(&self, text: &str) -> bool {
        self.side_rows().iter().any(|r| r.contains(text))
    }

    /// The row index in the side bar's columns holding `text`.
    fn side_row_of(&self, text: &str) -> Option<usize> {
        self.side_rows().iter().position(|r| r.contains(text))
    }

    /// A command that prints `name`'s marker line and `lines` numbered lines.
    fn fake_out(&mut self, name: &str, lines: u32) {
        self.type_text(&format!(
            "printf '\\033]133;B\\007\\033]633;E;{name}\\007'; echo marker-{name}; seq 1 {lines}; printf '\\033]133;D;0\\007'"
        ));
    }

    /// Waits for the active terminal's shell to print its first prompt, so a line typed next is
    /// echoed after it (typed earlier, the tty echoes it first and the command's output then
    /// starts on the prompt's line).
    async fn prompted(&mut self) {
        self.settle("the shell's prompt", |h| {
            h.app
                .groups
                .active_group()
                .active_tab()
                .is_some_and(|t| jterm::testing::has_prompt(&t.run.screen().screen().contents()))
        })
        .await;
    }

    fn status_row(&self) -> String {
        self.screen.screen().rows(0, 100).last().unwrap_or_default()
    }
}

fn row(label: &str, glyph: char, title: &str, closed: bool) -> (String, char, String, bool) {
    (label.into(), glyph, title.into(), closed)
}

// FLAKE: 2026-11-16, failed once in a full `scripts/check` run under heavy load (overlapping
// runs; which step is unknown); the FLAKE lines below name the step and the rows seen.
#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_a_command_is_a_running_row_then_finished_with_its_exit_code() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    let flake = |step: &str, h: &Harness| {
        eprintln!(
            "FLAKE jmux_commands_a_a_command_is_a_running_row_then_finished_with_its_exit_code: before `{step}`, rows {:?}",
            h.rows()
        )
    };
    h.fake("slow", 2, 0);
    flake("a running row", &h);
    h.settle("a running row", |h| h.rows() == [row("slow", '●', "sh", false)])
        .await;
    flake("it finished", &h);
    h.settle("it finished", |h| h.rows() == [row("slow", '✓', "sh", false)])
        .await;
    let r = &h.app.history.rows()[0];
    let took = r.record.finished.map(|f| f - r.record.started);
    assert!(
        r.duration_text(Instant::now()).ends_with('s'),
        "FLAKE: duration text {:?}",
        r.duration_text(Instant::now())
    );
    // `sleep 2`, less however late jmux read the B mark (1.999 s seen under load).
    assert!(took.is_some_and(|t| t >= Duration::from_millis(1500)), "took {took:?}");
    h.fake("bad", 0, 1);
    flake("a failed row", &h);
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
    h.app.run(Command::NewTerminal);
    h.fake("two", 0, 0);
    h.settle("two", |h| h.rows().len() == 2 && !h.app.history.rows()[0].running())
        .await;
    h.app.run(Command::PrevTab);
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
    h.app.run(Command::NewTerminal);
    h.fake("done", 0, 0);
    h.settle("done", |h| h.rows().len() == 2 && !h.app.history.rows()[0].running())
        .await;
    h.fake("hang", 30, 0);
    h.settle("hang runs", |h| {
        h.rows().first().is_some_and(|r| r.0 == "hang" && r.1 == '●')
    })
    .await;
    // A running command asks first.
    h.app.run(Command::CloseTerminal);
    assert!(h.app.confirm.is_some());
    h.app.run(Command::ConfirmYes);
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
    h.app.run(Command::InstallShellIntegration);
    h.settle("the install notice", |h| {
        h.status_row().contains("Added to ~/.zshrc") || h.app.notice.as_deref().is_some_and(|n| n.starts_with("Added"))
    })
    .await;
    let rc = std::fs::read_to_string(d.home.join(".zshrc")).unwrap();
    assert!(rc.contains("shell-integration.zsh"), "{rc}");
    let snippet = std::fs::read_to_string(d.state.join("shell-integration.zsh")).unwrap();
    assert!(snippet.contains("JMUX"), "the guard is jmux's");
    // Again: the snippet is refreshed, the line is not added twice.
    h.app.run(Command::InstallShellIntegration);
    h.settle("the second notice", |h| {
        h.app.notice.as_deref().is_some_and(|n| n.starts_with("Updated"))
    })
    .await;
    assert_eq!(std::fs::read_to_string(d.home.join(".zshrc")).unwrap(), rc);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_a_closing_a_tab_keeps_a_finish_the_app_had_not_seen() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.app.run(Command::NewTerminal);
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
    h.app.run(Command::CloseTerminal);
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
    h.app.run(Command::NewTerminal);
    h.app.run(Command::NewTerminal);
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        h.step().await;
    }
    assert_eq!(h.app.groups.tab_count(), 3);
    assert_eq!(h.app.notice, None, "the check ran again");
}

async fn finished(h: &mut Harness, count: usize) {
    h.settle("rows finished", move |h| {
        h.rows().len() == count && h.app.history.rows().iter().all(|r| !r.running())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_a_command_is_a_running_row_with_its_title_then_a_check_with_its_duration() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.fake("slow", 2, 0);
    h.settle("a running row", |h| {
        h.side_rows()
            .iter()
            .any(|r| r.contains('●') && r.contains("slow") && r.contains("sh"))
    })
    .await;
    h.settle("the check", |h| {
        h.side_rows()
            .iter()
            .any(|r| r.contains('✓') && r.contains("slow") && r.contains("s ") && !r.contains('●'))
    })
    .await;
    let row = h.side_rows().into_iter().find(|r| r.contains("slow")).unwrap();
    // About 2 s: the start is when jmux reads the B mark, which a loaded machine can read late
    // (1.9s seen with eight test runs at once).
    assert!(
        ["1.", "2.", "3."].iter().any(|d| row.contains(d)),
        "the duration shows: {row:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_a_failed_command_shows_a_cross() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.fake("bad", 0, 1);
    h.settle("the cross", |h| {
        h.side_rows().iter().any(|r| r.contains('✗') && r.contains("bad"))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_rows_of_two_terminals_interleave_by_time_on_screen() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.fake("one", 0, 0);
    finished(&mut h, 1).await;
    h.app.run(Command::NewTerminal);
    h.fake("two", 0, 0);
    finished(&mut h, 2).await;
    h.app.run(Command::PrevTab);
    h.fake("three", 0, 0);
    finished(&mut h, 3).await;
    h.settle("all three drawn", |h| {
        ["one", "two", "three"].iter().all(|n| h.side_has(n))
    })
    .await;
    let (three, two, one) = (
        h.side_row_of("three").unwrap(),
        h.side_row_of("two").unwrap(),
        h.side_row_of("one").unwrap(),
    );
    assert!(three < two && two < one, "newest first");
    assert!(h.side_rows()[two].contains("sh (2)"), "the other terminal's title");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_clicking_an_old_row_in_the_other_group_focuses_it_and_scrolls_its_command_to_the_top() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    let first = h.app.groups.active;
    h.prompted().await;
    h.fake_out("old", 60);
    finished(&mut h, 1).await;
    h.app.run(Command::SplitRight);
    let second = h.app.groups.active;
    assert_ne!(first, second);
    h.prompted().await;
    h.fake("other", 0, 0);
    finished(&mut h, 2).await;
    h.settle("both rows drawn", |h| h.side_has("old") && h.side_has("other"))
        .await;
    let y = h.side_row_of("old").unwrap() as u16;
    h.click(5, y);
    assert_eq!(h.app.groups.active, first, "the old terminal has the focus");
    assert!(!h.app.side.focused);
    let x = usize::from(h.app.side.reserved(100));
    h.settle("the command at the top", |h| {
        let rows: Vec<String> = h.screen.screen().rows(0, 100).collect();
        // Row 0 is the group's tab bar; its terminal starts below it.
        rows[1].chars().skip(x).collect::<String>().starts_with("marker-old")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_a_lost_command_only_focuses_with_a_notice() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.fake_out("gone", 5);
    finished(&mut h, 1).await;
    h.fake_out("flood", 6500);
    finished(&mut h, 2).await;
    h.settle("both rows", |h| h.side_has("gone")).await;
    let y = h.side_row_of("gone").unwrap() as u16;
    h.click(5, y);
    assert_eq!(
        h.app.notice.as_deref(),
        Some("That command's output is no longer in the scrollback")
    );
    assert!(!h.app.side.focused);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_a_closed_terminals_rows_are_dimmed_and_do_nothing() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.app.run(Command::NewTerminal);
    h.fake_out("gone", 3);
    finished(&mut h, 1).await;
    h.app.run(Command::CloseTerminal);
    h.settle("the row is closed", |h| h.rows().first().is_some_and(|r| r.3))
        .await;
    h.settle("the dimmed row", |h| {
        h.side_row_of("gone").is_some_and(|y| {
            let cell = h.screen.screen().cell(y as u16, 3);
            cell.is_some_and(|c| c.fgcolor() == vt100::Color::Rgb(0x5A, 0x5A, 0x5A))
        })
    })
    .await;
    // The surviving shell has output and is scrolled away from the bottom, so a wrong jump shows.
    h.fake_out("fill", 60);
    finished(&mut h, 2).await;
    let run = &h.app.groups.tabs().last().unwrap().run;
    run.scroll(-5);
    let offset = run.screen().screen().scrollback();
    assert_eq!(offset, 5);
    h.settle("both rows drawn", |h| h.side_has("gone") && h.side_has("fill"))
        .await;
    let y = h.side_row_of("gone").unwrap() as u16;
    let (group, before) = (h.app.groups.active, h.app.groups.active_group().active);
    h.app.run(Command::Side(SideCommand::Focus));
    h.click(5, y);
    assert_eq!(h.app.notice, None);
    assert_eq!(h.app.groups.active, group);
    assert_eq!(h.app.groups.active_group().active, before);
    assert_eq!(h.app.groups.tab_count(), 1);
    let run = &h.app.groups.tabs().last().unwrap().run;
    assert_eq!(run.screen().screen().scrollback(), offset, "no scroll");
    assert_eq!(
        h.app.history.selected().unwrap().label(),
        "gone",
        "the click selected it"
    );
    assert!(h.app.side.focused, "the click did not hand the keys to the shell");
    // Enter on it does nothing either.
    h.app.run(Command::Side(SideCommand::Focus));
    h.key(KeyCode::Enter);
    assert!(h.app.side.focused, "Enter on a closed row keeps the bar's keys");
    assert_eq!(h.app.notice, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_the_filter_narrows_the_list() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    for name in ["alpha", "beta", "alpine"] {
        h.fake(name, 0, 0);
        let n = h.rows().len() + 1;
        finished(&mut h, n).await;
    }
    h.settle("rows drawn", |h| h.side_has("beta")).await;
    assert!(h.side_has("Filter"), "the placeholder");
    h.app.run(Command::Side(SideCommand::Focus));
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('l'));
    h.settle("narrowed", |h| {
        h.side_has("alpine") && h.side_has("alpha") && !h.side_has("beta")
    })
    .await;
    assert_eq!(h.app.history.visible().len(), 2);
    assert!(h.side_rows()[0].contains("al"));
    // Backspace widens it again.
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Backspace);
    h.settle("all again", |h| h.side_has("beta")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_the_list_holds_at_most_command_history_rows() {
    let mut h = Harness::new("/bin/sh", None, 3).await;
    h.show_side().await;
    h.type_text(
        "for i in 1 2 3 4 5; do printf '\\033]133;B\\007\\033]633;E;c%s\\007' $i; printf '\\033]133;D;0\\007'; done",
    );
    h.settle("the newest three drawn", |h| {
        h.side_has("c5") && h.side_has("c3") && !h.side_has("c2")
    })
    .await;
    assert_eq!(h.app.history.visible().len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_without_the_integration_the_bar_says_how_to_install_it() {
    let tmp = tempfile::tempdir().unwrap();
    let zsh = tmp.path().join("zsh");
    symlink("/bin/sh", &zsh).unwrap();
    let mut h = Harness::new(zsh.to_str().unwrap(), Some(dirs(tmp.path())), COMMAND_HISTORY).await;
    h.settle("the check", |h| h.app.integration_missing).await;
    h.show_side().await;
    h.settle("the hint", |h| h.side_has("install-shell-integration")).await;
    assert!(h.side_has("No commands yet"));
    assert!(
        h.side_rows().iter().any(|r| r.contains("Ctrl+Shift+I")),
        "{:?}",
        h.side_rows()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_with_the_integration_unknown_the_empty_bar_just_says_no_commands() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    assert!(h.side_has("No commands yet"));
    assert!(!h.side_has("install-shell-integration"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_keys_move_the_selection_open_a_row_and_hand_the_keys_back() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.fake_out("first", 3);
    finished(&mut h, 1).await;
    h.fake_out("second", 3);
    finished(&mut h, 2).await;
    // Ctrl+Shift+E (Cmd+Shift+E) shows the bar and gives it the keys; Ctrl+Shift+B (Cmd+B) hides it.
    h.app.run(Command::Side(SideCommand::Focus));
    assert!(h.app.side.visible && h.app.side.focused);
    h.app.press(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(h.app.history.selected().unwrap().label(), "second");
    h.app.press(KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(h.app.history.selected().unwrap().label(), "first");
    h.app.press(KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(h.app.history.selected().unwrap().label(), "second");
    h.app.press(KeyCode::End, KeyModifiers::NONE);
    assert_eq!(h.app.history.selected().unwrap().label(), "first");
    h.app.press(KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(h.app.history.selected().unwrap().label(), "second");
    h.app.press(KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(h.app.history.selected().unwrap().label(), "first");
    h.app.press(KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(h.app.history.selected().unwrap().label(), "second");
    // Typing goes to the filter, never to the shell.
    h.key(KeyCode::Char('s'));
    assert_eq!(h.app.side.filter.text, "s");
    // Enter opens the selected row and the terminal has the keys again.
    h.app.press(KeyCode::Enter, KeyModifiers::NONE);
    assert!(!h.app.side.focused);
    assert_eq!(h.app.notice, None);
    // Esc gives the keys back without opening anything.
    h.app.run(Command::Side(SideCommand::Focus));
    assert!(h.app.side.focused);
    h.app.press(KeyCode::Esc, KeyModifiers::NONE);
    assert!(!h.app.side.focused && h.app.side.visible);
    h.app.run(Command::Side(SideCommand::Toggle));
    assert!(!h.app.side.visible);
    // Plain Ctrl+B stays the shell's.
    h.app.press(KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(!h.app.side.visible);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_dragging_the_border_resizes_the_bar_and_the_shells() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    let cols = |h: &Harness| {
        h.app
            .groups
            .active_group()
            .active_tab()
            .unwrap()
            .run
            .screen()
            .screen()
            .size()
            .1
            .get()
    };
    h.settle("the shell fits", |h| cols(h) == 100 - 33 - 1).await;
    h.app.send_mouse(MouseEventKind::Down(MouseButton::Left), 32, 5);
    h.app.send_mouse(MouseEventKind::Drag(MouseButton::Left), 40, 5);
    h.settle("the bar is wider", |h| h.app.side.width == 40).await;
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), 40, 5);
    h.settle("the shell follows", |h| cols(h) == 100 - 41 - 1).await;
    // Too far: the groups keep their room; too narrow: the minimum.
    h.app.send_mouse(MouseEventKind::Down(MouseButton::Left), 40, 5);
    h.app.send_mouse(MouseEventKind::Drag(MouseButton::Left), 99, 5);
    assert_eq!(h.app.side.width, 100 - 20 - 1);
    h.app.send_mouse(MouseEventKind::Drag(MouseButton::Left), 0, 5);
    assert_eq!(h.app.side.width, 12);
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), 0, 5);
    assert!(h.app.drag.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_the_wheel_and_scroll_bar_move_a_long_list() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.type_text(
        "for i in $(seq 1 40); do printf '\\033]133;B\\007\\033]633;E;c%s\\007' $i; printf '\\033]133;D;0\\007'; done",
    );
    h.settle("forty rows", |h| h.rows().len() == 40 && h.side_has("c40"))
        .await;
    let thumb = h.app.hit.side.as_ref().unwrap().scroll.thumb;
    assert!(thumb.is_some(), "a scroll bar thumb");
    h.app.send_mouse(MouseEventKind::ScrollDown, 5, 5);
    assert_eq!(h.app.history.scroll(), 3);
    h.app.send_mouse(MouseEventKind::ScrollUp, 5, 5);
    assert_eq!(h.app.history.scroll(), 0);
    // A press below the thumb on the track jumps the list down.
    let track = h.app.hit.side.as_ref().unwrap().scroll.track;
    h.app.send_mouse(
        MouseEventKind::Down(MouseButton::Left),
        track.x,
        track.y + track.height - 1,
    );
    assert!(h.app.history.scroll() > 0);
    h.app.send_mouse(
        MouseEventKind::Up(MouseButton::Left),
        track.x,
        track.y + track.height - 1,
    );
    assert!(h.app.drag.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_the_tick_redraws_only_while_the_bar_shows_rows() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.fake("a", 0, 0);
    finished(&mut h, 1).await;
    h.settle("drawn", |h| !h.app.needs_redraw).await;
    h.app.needs_redraw = false;
    h.app.handle(AppEvent::Tick);
    assert!(!h.app.needs_redraw, "hidden bar: no frame");
    h.show_side().await;
    h.app.needs_redraw = false;
    h.app.handle(AppEvent::Tick);
    assert!(h.app.needs_redraw, "a visible bar with rows: a frame");
    // An empty bar has nothing that moves.
    let mut empty = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    empty.show_side().await;
    empty.app.needs_redraw = false;
    empty.app.handle(AppEvent::Tick);
    assert!(!empty.app.needs_redraw);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_the_tick_does_not_redraw_without_a_client() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.fake("a", 0, 0);
    finished(&mut h, 1).await;
    h.show_side().await;
    h.app.run(Command::Detach);
    h.app.needs_redraw = false;
    h.app.handle(AppEvent::Tick);
    assert!(!h.app.needs_redraw, "no client: no frame");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_keys_typed_in_the_bar_never_reach_the_shell() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.app.run(Command::Side(SideCommand::Focus));
    for ch in "zzq".chars() {
        h.key(KeyCode::Char(ch));
    }
    h.app.press(KeyCode::Char('c'), KeyModifiers::CONTROL);
    h.key(KeyCode::Tab);
    h.key(KeyCode::F(5));
    assert!(h.app.side.focused);
    h.key(KeyCode::Esc);
    assert!(!h.app.side.focused);
    h.type_text("echo mark$((20+2))");
    let text = |h: &Harness| h.app.groups.tabs().last().unwrap().run.screen().screen().contents();
    h.settle("the shell answers", |h| text(h).contains("mark22")).await;
    assert!(
        !text(&h).contains("zzq"),
        "the bar's keys reached the shell: {}",
        text(&h)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_the_close_question_has_the_keys_over_a_focused_bar() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.fake("busy", 3, 0);
    h.settle("running", |h| h.app.history.rows().iter().any(|r| r.running()))
        .await;
    h.show_side().await;
    h.app.run(Command::Side(SideCommand::Focus));
    // Not through `run`: the key path hands the bar's keys back first; the question is asked of a still focused bar.
    h.app.execute(Command::CloseTerminal);
    assert!(h.app.confirm.is_some());
    h.key(KeyCode::Char('n'));
    assert!(h.app.confirm.is_none(), "n answers the question");
    assert!(h.app.side.focused, "the bar keeps the keys");
    assert_eq!(h.app.side.filter.text, "", "the answer was not typed into the filter");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_commands_b_a_paste_into_the_focused_bar_goes_to_the_filter() {
    let mut h = Harness::new("/bin/sh", None, COMMAND_HISTORY).await;
    h.show_side().await;
    h.app.run(Command::Side(SideCommand::Focus));
    let msg = ClientMsg::Input(Input::Paste("ab\ncd".into()));
    h.app.handle(AppEvent::Client { conn: 1, msg });
    assert_eq!(h.app.side.filter.text, "abcd");
}
