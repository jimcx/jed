//! End-to-end tests of jmux_layout-a (groups and the tab bar), driving the server `App` headlessly
//! with real `/bin/sh` shells: a new terminal is a second tab and the keys cycle between them
//! (the first part of the task's "done when"), titles and numbering, the close confirm, a shell
//! that exits closing its tab, the last tab ending the session, clicks on the bar, mouse positions
//! offset by the bar, and a re-attach showing the same tabs. Splits, tab drag and the terminal
//! features follow in jmux_layout-b to -d.

use std::num::NonZeroU16;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, Config};
use crate::command::Command;
use crate::protocol::{ByeReason, ClientMsg, PROTOCOL_VERSION, ServerMsg};

/// The longest any wait lasts.
const WAIT: Duration = Duration::from_secs(10);

/// An `App` with a client attached, a screen the client's output is applied to, and the event
/// loop's steps (`settle`) run by hand.
struct Harness {
    app: App,
    events: UnboundedReceiver<AppEvent>,
    conn_rx: Receiver<ServerMsg>,
    terminal: Terminal<StreamBackend>,
    screen: vt100::Parser,
    bye: Option<ByeReason>,
    conn: u64,
    rows: u16,
    cols: u16,
}

fn screen(rows: u16, cols: u16) -> vt100::Parser {
    vt100::Parser::new(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap(), 0)
}

impl Harness {
    /// A started session (one tab) with client 1 attached at `rows` × `cols`.
    async fn new(rows: u16, cols: u16) -> Harness {
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let config = Config {
            name: "work".into(),
            socket: PathBuf::new(),
            program: "/bin/sh".into(),
            cwd: std::env::temp_dir(),
        };
        let mut app = App::new(config, tx);
        app.start();
        let (_, conn_rx) = tokio::sync::mpsc::channel(1);
        let mut h = Harness {
            app,
            events,
            conn_rx,
            terminal: Terminal::new(StreamBackend::new(cols, rows)).unwrap(),
            screen: screen(rows, cols),
            bye: None,
            conn: 1,
            rows,
            cols,
        };
        h.attach(1);
        h
    }

    /// Connects `conn` and says hello at the harness's size (a fresh client screen).
    fn attach(&mut self, conn: u64) {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        self.conn_rx = rx;
        let writer = tokio::spawn(std::future::pending::<()>());
        self.app.handle(AppEvent::Connected { conn, tx, writer });
        let msg = ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            rows: self.rows,
            cols: self.cols,
            truecolor: true,
            term: "xterm".into(),
        };
        self.app.handle(AppEvent::Client { conn, msg });
        self.screen = screen(self.rows, self.cols);
        self.terminal = Terminal::new(StreamBackend::new(self.cols, self.rows)).unwrap();
        self.conn = conn;
    }

    fn input(&mut self, input: Input) {
        let msg = ClientMsg::Input(input);
        let conn = self.conn;
        self.app.handle(AppEvent::Client { conn, msg });
    }

    fn key(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.input(Input::Key(KeyEvent::new(code, mods)));
    }

    /// The key bound to `command`, pressed.
    fn command(&mut self, command: Command) {
        let combo = self.app.keymap.binding_for(command).expect("a default key");
        self.key(combo.code, combo.mods);
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            let code = if ch == '\n' { KeyCode::Enter } else { KeyCode::Char(ch) };
            self.key(code, KeyModifiers::NONE);
        }
    }

    fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        let modifiers = KeyModifiers::NONE;
        self.input(Input::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers,
        }));
    }

    fn click(&mut self, button: MouseButton, column: u16, row: u16) {
        self.mouse(MouseEventKind::Down(button), column, row);
    }

    /// One step of the event loop: handle what arrived, draw when a frame is due, apply the
    /// client's output.
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
            match msg {
                ServerMsg::Output(bytes) => self.screen.process(&bytes),
                ServerMsg::Bye { reason } => self.bye = Some(reason),
                _ => {}
            }
        }
    }

    /// Runs the loop until `done` (a panic after `WAIT`).
    async fn settle(&mut self, what: &str, done: impl Fn(&Harness) -> bool) {
        let deadline = Instant::now() + WAIT;
        self.step().await;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}:\n{}",
                self.text()
            );
            self.step().await;
        }
    }

    async fn settle_text(&mut self, needle: &str) {
        let needle = needle.to_string();
        self.settle(&format!("{needle:?} on screen"), move |h| h.text().contains(&needle))
            .await;
    }

    fn text(&self) -> String {
        self.screen.screen().contents()
    }

    fn row(&self, row: u16) -> String {
        self.screen
            .screen()
            .rows(0, self.cols)
            .nth(usize::from(row))
            .unwrap_or_default()
    }

    fn titles(&self) -> Vec<String> {
        self.app
            .groups
            .active_group()
            .tabs
            .iter()
            .map(|t| t.title.clone())
            .collect()
    }

    fn active(&self) -> usize {
        self.app.groups.active_group().active
    }

    fn tab_count(&self) -> usize {
        self.app.groups.tab_count()
    }

    /// Starts a command in the active tab: the shell integration's `133;B` marks it running.
    async fn run_long_command(&mut self) {
        self.type_text("printf '\\033]133;B\\a'; sleep 30\n");
        self.settle("the tab to show a running command", |h| {
            h.app.groups.active_group().active_tab().is_some_and(|t| !t.idle)
        })
        .await;
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Hangs every shell up.
        self.app.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_new_terminal_is_a_second_tab_and_the_keys_cycle() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    assert_eq!(h.titles(), ["sh ○"]);
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.titles().len() == 2).await;
    assert_eq!(h.titles(), ["sh ○", "sh (2) ○"], "numbered per program");
    assert_eq!(h.active(), 1, "the new tab is active");
    h.settle_text("sh (2) ○").await;
    assert!(
        h.row(0).contains("sh ○") && h.row(0).contains("sh (2) ○"),
        "{}",
        h.row(0)
    );

    // Each tab has its own shell: what is typed goes to the active one.
    h.type_text("echo SECOND\n");
    h.settle("the second shell's output", |h| {
        h.row(1).contains("SECOND") || h.text().matches("SECOND").count() >= 2
    })
    .await;
    h.command(Command::NextTab);
    assert_eq!(h.active(), 0, "next wraps");
    h.settle("the first shell's screen", |h| !h.text().contains("SECOND"))
        .await;
    h.command(Command::PrevTab);
    assert_eq!(h.active(), 1, "previous wraps");
    h.command(Command::GoToTab(1));
    assert_eq!(h.active(), 0);
    h.command(Command::GoToTab(2));
    assert_eq!(h.active(), 1);
    h.command(Command::GoToTab(9));
    assert_eq!(h.active(), 1, "no tab 9: nothing happens");
    h.command(Command::NewTerminal);
    h.settle("a third tab", |h| h.tab_count() == 3).await;
    assert_eq!(h.titles(), ["sh ○", "sh (2) ○", "sh (3) ○"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_every_shell_is_sized_to_its_groups_area_and_hidden_tabs_keep_it() {
    let mut h = Harness::new(12, 60).await;
    // The first tab is drawn (and sized) before the second one hides it.
    h.settle_text("sh ○").await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.settle("the tab bar", |h| h.row(0).contains("sh (2)")).await;
    // Tab bar above, status bar below: 12 - 2 rows.
    let size = |h: &Harness, i: usize| {
        let g = h.app.groups.active_group();
        let (r, c) = g.tabs[i].run.screen().screen().size();
        (r.get(), c.get())
    };
    assert_eq!(size(&h, 1), (10, 60));
    // The client resizes while the second tab shows: the first, hidden, keeps its last size.
    h.rows = 15;
    h.cols = 70;
    h.input(Input::Resize(70, 15));
    h.app.handle(AppEvent::Client {
        conn: h.conn,
        msg: ClientMsg::Resize { rows: 15, cols: 70 },
    });
    h.terminal = Terminal::new(StreamBackend::new(70, 15)).unwrap();
    h.settle("the active shell to follow", |h| size(h, 1) == (13, 70)).await;
    assert_eq!(size(&h, 0), (10, 60), "a hidden tab keeps its last size");
    h.command(Command::PrevTab);
    h.settle("the shown shell to follow", |h| size(h, 0) == (13, 70)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_close_terminal_hangs_up_an_idle_tab_at_once() {
    let mut h = Harness::new(12, 60).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.command(Command::CloseTerminal);
    assert!(h.app.confirm.is_none(), "an idle shell closes without asking");
    assert_eq!(h.titles(), ["sh ○"]);
    assert_eq!(h.active(), 0);
    h.settle("the bar to lose the tab", |h| !h.row(0).contains("sh (2)"))
        .await;
    assert!(!h.app.should_quit);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_closing_a_running_command_asks_first() {
    let mut h = Harness::new(12, 70).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.run_long_command().await;
    h.command(Command::CloseTerminal);
    assert!(h.app.confirm.is_some());
    assert_eq!(h.tab_count(), 2, "nothing closed yet");
    h.settle_text("Close terminal?").await;
    h.settle_text("[y] close  [n] cancel").await;
    // Other keys are swallowed (and do not reach the shell), a click does nothing.
    h.key(KeyCode::Char('x'), KeyModifiers::NONE);
    h.click(MouseButton::Left, 2, 0);
    assert!(h.app.confirm.is_some());
    assert_eq!(h.active(), 1);
    h.key(KeyCode::Char('n'), KeyModifiers::NONE);
    assert!(h.app.confirm.is_none());
    assert_eq!(h.tab_count(), 2, "cancelled");
    h.settle("the status bar back", |h| !h.text().contains("Close terminal?"))
        .await;
    // Again, and this time confirm: the shell is hung up.
    h.command(Command::CloseTerminal);
    assert!(h.app.confirm.is_some());
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(h.titles(), ["sh ○"]);
    assert!(h.app.confirm.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_a_shell_that_exits_closes_only_its_tab() {
    let mut h = Harness::new(12, 60).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.type_text("exit\n");
    h.settle("the exited tab to go", |h| h.tab_count() == 1).await;
    assert_eq!(h.titles(), ["sh ○"]);
    assert!(!h.app.should_quit, "the session goes on");
    assert!(h.bye.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_the_last_tab_closing_ends_the_session() {
    let mut h = Harness::new(12, 60).await;
    h.command(Command::CloseTerminal);
    assert!(h.app.should_quit);
    h.step().await;
    assert!(matches!(h.bye, Some(ByeReason::SessionEnded)), "{:?}", h.bye);

    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("exit\n");
    h.settle("the session to end", |h| h.app.should_quit).await;
    h.settle("the client to be told", |h| h.bye.is_some()).await;
    assert!(matches!(h.bye, Some(ByeReason::SessionEnded)));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_clicks_on_the_bar_pick_close_and_middle_click_closes() {
    let mut h = Harness::new(12, 60).await;
    for _ in 0..2 {
        h.command(Command::NewTerminal);
    }
    h.settle("three tabs on the bar", |h| {
        h.tab_count() == 3 && h.row(0).contains("sh (3)")
    })
    .await;
    let tabs = h.app.hit.groups[0].tabs.clone();
    assert_eq!(tabs.len(), 3);
    assert_eq!(h.app.hit.groups[0].term.y, 1, "the terminal starts below the bar");
    // A click on the first tab's title picks it.
    h.click(MouseButton::Left, tabs[0].x0 + 1, 0);
    assert_eq!(h.active(), 0);
    // The × of the second closes it (the shell is idle: no question).
    h.click(MouseButton::Left, tabs[1].close_x, 0);
    assert_eq!(h.titles(), ["sh ○", "sh (3) ○"]);
    h.settle("a redrawn bar", |h| !h.row(0).contains("sh (2)")).await;
    // A middle click on a title closes that tab too.
    let tabs = h.app.hit.groups[0].tabs.clone();
    h.click(MouseButton::Middle, tabs[1].x0 + 1, 0);
    assert_eq!(h.titles(), ["sh ○"]);
    // The bar's ×s and titles are real cells on the screen.
    h.settle("a redrawn bar", |h| h.row(0).contains('×')).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_a_tab_bar_wider_than_the_screen_scrolls_with_arrows() {
    let mut h = Harness::new(10, 30).await;
    for _ in 0..4 {
        h.command(Command::NewTerminal);
    }
    h.settle("five tabs", |h| h.tab_count() == 5).await;
    h.settle("scroll arrows", |h| {
        h.row(0).starts_with('‹') && h.row(0).trim_end().ends_with('›')
    })
    .await;
    let arrows = h.app.hit.groups[0].arrows.expect("arrows");
    assert!(
        h.app.groups.active_group().scroll > 0,
        "the active tab (last) was revealed"
    );
    let before = h.app.groups.active_group().scroll;
    h.click(MouseButton::Left, arrows.back, 0);
    assert_eq!(h.app.groups.active_group().scroll, before - 1);
    h.mouse(MouseEventKind::ScrollDown, 10, 0);
    assert_eq!(h.app.groups.active_group().scroll, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_mouse_positions_are_relative_to_the_terminal_below_the_bar() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    // A program asks for the mouse, then reads the 8 bytes of one report.
    h.type_text("printf '\\033[?1000h\\033[?1006h'; stty raw -echo; dd bs=1 count=8 2>/dev/null | od -An -tx1\n");
    h.settle("mouse reporting on", |h| {
        h.app
            .groups
            .active_group()
            .active_tab()
            .is_some_and(|t| t.run.screen().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
    })
    .await;
    // Screen cell (5, 4) is terminal cell (5, 3): reported 1-based as `ESC [ < 0 ; 6 ; 4 M`.
    h.click(MouseButton::Left, 5, 4);
    h.settle_text("1b 5b 3c 30 3b 36 3b 34").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_a_reattach_shows_the_same_tabs() {
    let mut h = Harness::new(12, 60).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.type_text("echo STILL_HERE\n");
    h.settle_text("STILL_HERE").await;
    h.command(Command::Detach);
    assert!(!h.app.should_quit, "a detach keeps the session");
    h.attach(2);
    h.settle_text("sh (2) ○").await;
    assert!(h.row(0).contains("sh ○"), "{}", h.row(0));
    assert_eq!(h.active(), 1, "the same tab is active");
    assert!(h.text().contains("STILL_HERE"), "{}", h.text());
    assert_eq!(h.titles(), ["sh ○", "sh (2) ○"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_the_status_bar_names_the_session_the_title_and_the_detach_key() {
    let mut h = Harness::new(12, 70).await;
    h.settle_text("work").await;
    let last = h.row(11);
    assert!(last.contains("work") && last.contains("sh ○"), "{last}");
    assert!(
        last.contains("Ctrl+Shift+D detach") || last.contains("detach"),
        "{last}"
    );
}

/// Whether any process of the group `pid` leads is still there.
fn group_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc::kill(-(pid as i32), 0) == 0 }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_each_shell_gets_jmux_and_its_own_jmux_terminal() {
    let mut h = Harness::new(14, 70).await;
    h.settle_text("sh ○").await;
    h.type_text("echo \"A=${JMUX+set}:$JMUX_TERMINAL.\"\n");
    let id0 = h.app.groups.active_group().tabs[0].run.id.0;
    h.settle_text(&format!("A=set:{id0}.")).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.type_text("echo \"B=${JMUX+set}:$JMUX_TERMINAL.\"\n");
    let id1 = h.app.groups.active_group().tabs[1].run.id.0;
    assert_ne!(id0, id1, "ids are unique per tab");
    h.settle_text(&format!("B=set:{id1}.")).await;
    // Closing a tab does not let its id come back for the next one.
    h.command(Command::CloseTerminal);
    h.command(Command::NewTerminal);
    h.settle("a new second tab", |h| h.tab_count() == 2).await;
    let id2 = h.app.groups.active_group().tabs[1].run.id.0;
    assert!(id2 != id0 && id2 != id1, "{id0} {id1} {id2}");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_a_closed_tabs_shell_process_is_really_gone() {
    let mut h = Harness::new(12, 70).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.run_long_command().await;
    h.settle("a started shell", |h| {
        h.app.groups.active_group().tabs[1].run.pid().is_some()
    })
    .await;
    let pid = h.app.groups.active_group().tabs[1].run.pid().expect("a running shell");
    assert!(group_alive(pid));
    h.command(Command::CloseTerminal);
    h.key(KeyCode::Char('y'), KeyModifiers::NONE);
    assert_eq!(h.tab_count(), 1);
    h.settle("the shell and its command to die", |_| !group_alive(pid))
        .await;

    // An idle tab's shell goes too.
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.settle("a started shell", |h| {
        h.app.groups.active_group().tabs[1].run.pid().is_some()
    })
    .await;
    let pid = h.app.groups.active_group().tabs[1].run.pid().expect("a running shell");
    h.command(Command::CloseTerminal);
    assert_eq!(h.tab_count(), 1);
    h.settle("the idle shell to die", |_| !group_alive(pid)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_a_failed_command_marks_the_tab_and_an_idle_one_closes_without_asking() {
    let mut h = Harness::new(12, 70).await;
    h.settle_text("sh ○").await;
    // The shell integration's `133;D;3`: the last command failed.
    h.type_text("printf '\\033]133;D;3\\a'\n");
    h.settle("the failed marker", |h| {
        h.app.groups.active_group().active_tab().is_some_and(|t| t.error)
    })
    .await;
    assert_eq!(h.titles(), ["sh ○"], "still idle, so still the ○ glyph");
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.command(Command::PrevTab);
    h.command(Command::CloseTerminal);
    assert!(h.app.confirm.is_none(), "a failed but idle shell closes without asking");
    assert_eq!(h.tab_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_plain_ctrl_d_reaches_the_shell_and_only_ctrl_shift_d_detaches() {
    let mut h = Harness::new(12, 60).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    // Wait for the new shell's prompt: an EOF typed before its line editor starts is lost.
    h.settle("the second shell's prompt", |h| {
        h.app.groups.active_group().tabs[1]
            .run
            .screen()
            .screen()
            .contents()
            .contains('$')
    })
    .await;
    // Ctrl+D is end of input to the shell: it exits and its tab goes; nobody is detached.
    h.key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    h.settle("the shell to exit on Ctrl+D", |h| h.tab_count() == 1).await;
    h.step().await;
    assert!(h.bye.is_none());
    h.key(KeyCode::Char('D'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    h.step().await;
    assert!(matches!(h.bye, Some(ByeReason::Detached)), "{:?}", h.bye);
    assert!(!h.app.should_quit);
    assert_eq!(h.tab_count(), 1, "the shell lives on");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_ctrl_chords_are_swallowed_by_the_confirm_but_y_and_esc_work() {
    let mut h = Harness::new(12, 70).await;
    h.command(Command::NewTerminal);
    h.settle("a second tab", |h| h.tab_count() == 2).await;
    h.run_long_command().await;
    h.command(Command::CloseTerminal);
    assert!(h.app.confirm.is_some());
    h.key(KeyCode::Char('d'), KeyModifiers::CONTROL);
    h.key(KeyCode::Char('n'), KeyModifiers::CONTROL);
    h.key(KeyCode::Char('y'), KeyModifiers::CONTROL);
    assert!(h.app.confirm.is_some());
    assert_eq!(h.tab_count(), 2);
    h.key(KeyCode::Esc, KeyModifiers::NONE);
    assert!(h.app.confirm.is_none());
    h.command(Command::CloseTerminal);
    h.key(KeyCode::Char('y'), KeyModifiers::NONE);
    assert_eq!(h.tab_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_a_running_command_in_a_hidden_tab_asks_when_its_x_is_clicked() {
    let mut h = Harness::new(12, 70).await;
    h.run_long_command().await;
    h.command(Command::NewTerminal);
    h.settle("a second tab and its bar", |h| {
        h.tab_count() == 2 && h.row(0).contains("sh (2)")
    })
    .await;
    let close_x = h.app.hit.groups[0].tabs[0].close_x;
    h.click(MouseButton::Left, close_x, 0);
    assert!(h.app.confirm.is_some(), "the running tab asks, shown or not");
    assert_eq!(h.tab_count(), 2);
}

/// Starts a one-second command that marks the tab running and ends by printing `DONE_2`.
async fn run_short_command(h: &mut Harness) {
    h.type_text("printf '\\033]133;B\\a'; sleep 1; echo DONE_$((1+1))\n");
    h.settle("the tab to show a running command", |h| {
        h.app.groups.active_group().active_tab().is_some_and(|t| !t.idle)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_a_keys_typed_during_the_confirm_never_reach_the_shell() {
    // The typed command prints LEAKED only when run (its echo shows `LEA""KED`). No Enter: that is
    // "yes" while the question is open, so it is pressed once the command has ended.
    let leak = "echo LEA\"\"KED";

    // Positive control: with no question open the same text is type-ahead the shell runs.
    let mut h = Harness::new(12, 70).await;
    run_short_command(&mut h).await;
    h.type_text(leak);
    h.settle_text("DONE_2").await;
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.settle_text("LEAKED").await;

    // With the question open the keys are swallowed: the shell never sees them.
    let mut h = Harness::new(12, 70).await;
    run_short_command(&mut h).await;
    h.command(Command::CloseTerminal);
    assert!(h.app.confirm.is_some());
    h.type_text(leak);
    h.key(KeyCode::Esc, KeyModifiers::NONE);
    assert!(h.app.confirm.is_none());
    h.settle_text("DONE_2").await;
    // Anything the shell had been sent is on its line: Enter runs it, before the marker's command.
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.type_text("echo END_$((2+3))\n");
    h.settle_text("END_5").await;
    assert!(!h.text().contains("LEAKED"), "{}", h.text());
}
