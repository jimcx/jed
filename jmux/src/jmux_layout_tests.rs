//! End-to-end tests of jmux_layout-a (groups and the tab bar), driving the server `App` headlessly
//! with real `/bin/sh` shells: a new terminal is a second tab and the keys cycle between them
//! (the first part of the task's "done when"), titles and numbering, the close confirm, a shell
//! that exits closing its tab, the last tab ending the session, clicks on the bar, mouse positions
//! offset by the bar, and a re-attach showing the same tabs.
//!
//! jmux_layout-b (`jmux_layout_b_*`): split right gives two live shells side by side, each sized
//! to its half; the divider drags through the `HitMap` and the shells follow; focus moves with
//! keys and clicks; the pointer shape (OSC 22) follows the divider and the terminal text; a shell
//! exiting removes its group; a re-attach keeps the split.
//!
//! jmux_layout-c (`jmux_layout_c_*`): a tab drags along its bar, to another group's bar (an
//! emptied group goes), onto a group's edge to split it or its centre to join it; no target moves
//! nothing; the marker and shade are drawn; a detach clears the drag.
//!
//! jmux_layout-d (`jmux_layout_d_*`): dragging in a terminal selects (drawn in the theme's
//! selection colour) and Copy sends the text as OSC 52; double and triple clicks select a path-like
//! word and the row; a paste types into the shell, bracketed when it asked; a program with mouse
//! reporting gets the events but Shift keeps them for the selection; the wheel, Shift+PageUp and the
//! scroll bar (click and drag) scroll the scrollback and a typed key returns to the live screen.
//!
//! jmux_layout-e (`jmux_layout_e_*`): the search bar finds, counts ("2 of 3") and steps with wrap,
//! scrolls the view to a match in the scrollback and draws the matches in the theme's find colours;
//! the program gets no keys while the bar is open and Esc gives them back; the bar goes with its
//! tab. A Ctrl / Cmd + click on a URL (also one wrapped over two rows) opens exactly it through the
//! recording opener, a plain click does not. A full-screen program (a stand-in, and a real `vi`
//! when installed) gets its keys and mouse.
//!
//! The whole task's "done when", clause by clause, and the test that covers each:
//! - a new terminal is a second tab and the keys cycle: `jmux_layout_a_new_terminal_is_a_second_tab_and_the_keys_cycle`;
//! - split right gives two live shells side by side: `jmux_layout_b_split_right_gives_two_live_shells_side_by_side`;
//! - the divider drags: `jmux_layout_b_the_divider_drags_and_the_shells_follow`; focus moves with the keys
//!   and a click: `jmux_layout_b_split_down_stacks_and_focus_follows_the_keys_and_clicks`;
//! - a tab drags along its bar (`jmux_layout_c_a_tab_drags_along_its_bar_and_keeps_its_shell`), to the other
//!   group (`jmux_layout_c_a_tab_moves_to_the_other_groups_bar_and_an_emptied_group_goes`) and onto a
//!   group's edge to split it (`jmux_layout_c_a_drop_on_an_edge_splits_the_group_and_the_centre_joins_it`);
//! - dragging selects and copy sends OSC 52: `jmux_layout_d_dragging_selects_and_copy_sends_osc_52`; paste
//!   types it: `jmux_layout_d_a_paste_types_into_the_shell_bracketed_when_it_asked`; the wheel and the scroll
//!   bar scroll: `jmux_layout_d_the_wheel_and_the_scrollback_keys_scroll_and_a_typed_key_returns_to_the_live_screen`,
//!   `jmux_layout_d_the_scroll_bar_is_drawn_and_a_click_or_drag_on_it_scrolls`;
//! - search finds and steps: `jmux_layout_e_search_finds_counts_and_steps_with_wrap_and_scrolls_to_the_match`;
//!   a Ctrl+click on a URL opens it: `jmux_layout_e_a_ctrl_click_on_a_url_opens_exactly_it_and_a_plain_click_does_not`;
//! - a full-screen program gets its keys and mouse: `jmux_layout_e_a_full_screen_program_gets_its_keys_and_mouse`;
//! - detach and attach bring back the layout: `jmux_layout_b_a_new_terminal_opens_in_the_focused_group_and_reattach_keeps_the_split`,
//!   `jmux_layout_a_a_reattach_shows_the_same_tabs`.

use std::num::NonZeroU16;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, Config};
use crate::command::Command;
use crate::group::Side;
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
    /// Everything the client was sent, for the escape sequences the screen does not keep.
    raw: Vec<u8>,
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
            open_url: record_url,
            dirs: None,
            command_history: crate::app::COMMAND_HISTORY,
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
            raw: Vec::new(),
            conn: 1,
            rows,
            cols,
        };
        h.attach(1);
        // Typing before the shell's first prompt races the prompt against the command's echo. The
        // prompt is `$ ` (dash) or `sh-5.3$ ` (bash as sh).
        h.settle("the shell's first prompt", |h| h.row(1).trim_end().ends_with('$'))
            .await;
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
        self.raw.clear();
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
                ServerMsg::Output(bytes) => {
                    self.screen.process(&bytes);
                    self.raw.extend_from_slice(&bytes);
                }
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

    /// The pointer shapes (OSC 22 names) sent so far, in order.
    fn pointer_shapes(&self) -> Vec<String> {
        let raw = String::from_utf8_lossy(&self.raw);
        raw.split("\x1b]22;")
            .skip(1)
            .filter_map(|p| p.split("\x1b\\").next().map(str::to_string))
            .collect()
    }

    /// The size (rows, columns) of the active shell of `group`.
    fn shell_size(&self, group: u64) -> (u16, u16) {
        let tab = self.app.groups.get(group).unwrap().active_tab().unwrap();
        let (rows, cols) = tab.run.screen().screen().size();
        (rows.get(), cols.get())
    }

    /// Whether every group's active shell is the size of its terminal area.
    fn shells_fit(&self) -> bool {
        let fit = |g: &crate::hit::GroupHit| self.shell_size(g.id) == (g.text.height, g.text.width);
        self.app.hit.groups.iter().all(fit)
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
    assert_eq!(size(&h, 1), (10, 59));
    // The client resizes while the second tab shows: the first, hidden, keeps its last size.
    h.rows = 15;
    h.cols = 70;
    h.input(Input::Resize(70, 15));
    h.app.handle(AppEvent::Client {
        conn: h.conn,
        msg: ClientMsg::Resize { rows: 15, cols: 70 },
    });
    h.terminal = Terminal::new(StreamBackend::new(70, 15)).unwrap();
    h.settle("the active shell to follow", |h| size(h, 1) == (13, 69)).await;
    assert_eq!(size(&h, 0), (10, 59), "a hidden tab keeps its last size");
    h.command(Command::PrevTab);
    h.settle("the shown shell to follow", |h| size(h, 0) == (13, 69)).await;
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
    // The release ends the tab drag the press started.
    h.mouse(MouseEventKind::Up(MouseButton::Left), tabs[0].x0 + 1, 0);
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

/// A split session: the first group on the left, the new one on the right (focused).
async fn split_right(rows: u16, cols: u16) -> Harness {
    let mut h = Harness::new(rows, cols).await;
    h.settle_text("sh ○").await;
    h.command(Command::SplitRight);
    h.settle("two groups with a shell each", |h| {
        h.app.hit.groups.len() == 2 && h.tab_count() == 2 && h.shells_fit()
    })
    .await;
    h
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_split_right_gives_two_live_shells_side_by_side() {
    let mut h = split_right(12, 61).await;
    let (left, right) = (h.app.hit.groups[0].clone(), h.app.hit.groups[1].clone());
    assert_eq!(h.app.groups.active, right.id, "the new group has the focus");
    assert!(left.term.x + left.term.width < right.term.x, "a divider between them");
    assert_eq!(left.term.width + right.term.width + 1, 61);
    assert_eq!(h.app.hit.dividers.len(), 1);
    // The divider's line is drawn between the two terminals.
    let line = h.app.hit.dividers[0].rect;
    let drawn = h.row(line.y + 3).chars().nth(usize::from(line.x));
    assert_eq!(drawn, Some('│'), "{}", h.row(line.y + 3));

    h.type_text("echo RIGHT_$((1+1))\n");
    h.settle_text("RIGHT_2").await;
    h.command(Command::FocusSide(Side::Left));
    assert_eq!(h.app.groups.active, left.id);
    h.type_text("echo LEFT_$((1+2))\n");
    h.settle_text("LEFT_3").await;
    // Side by side: each output is on its own half of the screen.
    let col_of = |h: &Harness, needle: &str| {
        let line = (0..h.rows).map(|r| h.row(r)).find(|l| l.contains(needle)).unwrap();
        line.split(needle).next().unwrap().chars().count()
    };
    assert!(col_of(&h, "LEFT_3") < usize::from(left.term.width));
    assert!(col_of(&h, "RIGHT_2") >= usize::from(right.term.x));
    assert_eq!(h.shell_size(left.id), (left.text.height, left.text.width));
    assert_eq!(h.shell_size(right.id), (right.text.height, right.text.width));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_split_down_stacks_and_focus_follows_the_keys_and_clicks() {
    let mut h = split_right(20, 61).await;
    let (a, b) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    h.command(Command::SplitDown);
    h.settle("three groups", |h| {
        h.app.hit.groups.len() == 3 && h.tab_count() == 3 && h.shells_fit()
    })
    .await;
    let c = h.app.groups.active;
    assert!(![a, b].contains(&c));
    let rect = |h: &Harness, id| h.app.hit.group(id).unwrap().term;
    assert!(
        rect(&h, c).y > rect(&h, b).y && rect(&h, c).x == rect(&h, b).x,
        "c is below b"
    );
    h.command(Command::FocusSide(Side::Up));
    assert_eq!(h.app.groups.active, b);
    h.command(Command::FocusSide(Side::Up));
    assert_eq!(h.app.groups.active, b, "nothing above: stays");
    h.command(Command::FocusSide(Side::Left));
    assert_eq!(h.app.groups.active, a);
    h.command(Command::FocusSide(Side::Right));
    assert_eq!(h.app.groups.active, b);
    h.command(Command::FocusNextGroup);
    assert_eq!(h.app.groups.active, c);
    h.command(Command::FocusNextGroup);
    assert_eq!(h.app.groups.active, a, "wraps");
    h.command(Command::FocusPreviousGroup);
    assert_eq!(h.app.groups.active, c);
    h.command(Command::FocusGroup(2));
    assert_eq!(h.app.groups.active, b);
    h.command(Command::FocusGroup(7));
    assert_eq!(h.app.groups.active, b);
    // A click on a group's terminal or its bar focuses it.
    let t = rect(&h, a);
    h.click(MouseButton::Left, t.x + 2, t.y + 2);
    h.mouse(MouseEventKind::Up(MouseButton::Left), t.x + 2, t.y + 2);
    assert_eq!(h.app.groups.active, a);
    let bar = h.app.hit.group(c).unwrap().bar;
    h.click(MouseButton::Left, bar.x + bar.width - 1, bar.y);
    assert_eq!(h.app.groups.active, c);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_the_divider_drags_and_the_shells_follow() {
    let mut h = split_right(12, 61).await;
    let before = h.app.hit.dividers[0].rect;
    let (left_id, right_id) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    let wide_before = h.app.hit.group(left_id).unwrap().term.width;
    h.mouse(MouseEventKind::Down(MouseButton::Left), before.x, before.y + 4);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), before.x + 10, before.y + 4);
    // Wherever the pointer goes during the drag, the divider is what moves.
    h.mouse(MouseEventKind::Drag(MouseButton::Left), before.x + 12, before.y + 9);
    h.mouse(MouseEventKind::Up(MouseButton::Left), before.x + 12, before.y + 9);
    h.settle("the divider to move and the shells to follow", |h| {
        h.app.hit.dividers[0].rect.x == before.x + 12 && h.shells_fit()
    })
    .await;
    assert_eq!(h.app.hit.group(left_id).unwrap().term.width, wide_before + 12);
    assert_eq!(h.shell_size(left_id).1, wide_before + 11);
    assert_eq!(h.shell_size(right_id).1, h.app.hit.group(right_id).unwrap().text.width);
    // Released: more motion moves nothing.
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 5, 4);
    h.settle("a frame", |h| !h.app.needs_redraw).await;
    assert_eq!(h.app.hit.dividers[0].rect.x, before.x + 12);
    // The terminals still work after the resize.
    h.type_text("echo RESIZED_$((4+4))\n");
    h.settle_text("RESIZED_8").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_the_pointer_follows_the_divider_and_the_terminal_text() {
    let mut h = split_right(12, 61).await;
    let d = h.app.hit.dividers[0].rect;
    let term = h.app.hit.groups[0].term;
    let bar = h.app.hit.groups[0].bar;
    h.mouse(MouseEventKind::Moved, term.x + 2, term.y + 2);
    h.settle("the text pointer", |h| h.pointer_shapes() == ["text"]).await;
    h.mouse(MouseEventKind::Moved, d.x, d.y + 3);
    h.settle("the resize pointer", |h| h.pointer_shapes() == ["text", "ew-resize"])
        .await;
    // Held through a drag, wherever it goes.
    h.mouse(MouseEventKind::Down(MouseButton::Left), d.x, d.y + 3);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), d.x + 5, d.y + 3);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), d.x + 5, bar.y);
    h.settle("a frame", |h| !h.app.needs_redraw).await;
    assert_eq!(h.pointer_shapes(), ["text", "ew-resize"], "no change while dragging");
    h.mouse(MouseEventKind::Up(MouseButton::Left), 1, bar.y);
    h.settle("the default pointer over the bar", |h| {
        h.pointer_shapes().last().is_some_and(|s| s == "default")
    })
    .await;
    // A program that takes the mouse itself keeps the default shape over its text.
    h.type_text("printf '\\033[?1000h'\n");
    h.settle("mouse reporting on", |h| {
        let tab = h.app.groups.active_group().active_tab().unwrap();
        tab.run.screen().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    })
    .await;
    let right = h.app.hit.groups[1].term;
    let before = h.pointer_shapes().len();
    h.mouse(MouseEventKind::Moved, right.x + 2, right.y + 2);
    h.settle("a frame", |h| !h.app.needs_redraw).await;
    assert_eq!(h.pointer_shapes().len(), before, "the default stays");
    // A new client starts from the default; the old pointer position means nothing to it, so it
    // is told a shape only once its own pointer moves.
    h.attach(2);
    h.settle("the new client's frame", |h| h.text().contains("sh")).await;
    assert!(h.pointer_shapes().is_empty(), "{:?}", h.pointer_shapes());
    let left = h.app.hit.groups[0].term;
    h.mouse(MouseEventKind::Moved, left.x + 2, left.y + 2);
    h.settle("the text pointer for the new client", |h| {
        h.pointer_shapes() == ["text"]
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_a_shell_exiting_removes_its_group_and_the_other_fills_the_width() {
    let mut h = split_right(12, 61).await;
    let left = h.app.hit.groups[0].id;
    h.type_text("exit\n");
    h.settle("the right group to go", |h| {
        h.app.hit.groups.len() == 1 && h.shells_fit()
    })
    .await;
    assert_eq!(h.app.groups.active, left, "focus moves to the neighbour");
    assert_eq!(h.app.hit.groups[0].term.width, 61);
    assert!(h.app.hit.dividers.is_empty());
    assert_eq!(h.shell_size(left).1, 60);
    assert!(h.bye.is_none(), "the session goes on");
    h.type_text("echo STILL_$((5+5))\n");
    h.settle_text("STILL_10").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_closing_a_groups_last_tab_removes_it_and_the_last_group_ends_the_session() {
    let mut h = split_right(12, 61).await;
    h.command(Command::CloseTerminal);
    h.settle("one group", |h| h.app.hit.groups.len() == 1).await;
    assert_eq!(h.app.groups.order().len(), 1);
    h.command(Command::CloseTerminal);
    h.settle("the session to end", |h| h.bye.is_some()).await;
    assert_eq!(h.bye, Some(ByeReason::SessionEnded));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_a_new_terminal_opens_in_the_focused_group_and_reattach_keeps_the_split() {
    let mut h = split_right(12, 61).await;
    let x = h.app.hit.dividers[0].rect.x;
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, 5);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 20, 5);
    h.mouse(MouseEventKind::Up(MouseButton::Left), 20, 5);
    h.settle("the divider at 20", |h| {
        h.app.hit.dividers[0].rect.x == 20 && h.shells_fit()
    })
    .await;
    let (left, right) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    h.type_text("echo KEPT_$((6+1))\n");
    h.settle_text("KEPT_7").await;
    h.command(Command::FocusSide(Side::Left));
    h.command(Command::NewTerminal);
    h.settle("a tab in the left group", |h| {
        h.app.groups.get(left).unwrap().tabs.len() == 2
    })
    .await;
    assert_eq!(h.app.groups.get(right).unwrap().tabs.len(), 1);

    h.command(Command::Detach);
    h.settle("the detach", |h| h.bye == Some(ByeReason::Detached)).await;
    h.attach(2);
    h.settle("the same split again", |h| {
        h.app.hit.dividers.len() == 1 && h.app.hit.dividers[0].rect.x == 20 && h.text().contains("KEPT_7")
    })
    .await;
    assert_eq!(h.app.groups.get(left).unwrap().tabs.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_b_a_drag_does_not_outlive_its_client_and_hidden_tabs_keep_their_size() {
    let mut h = split_right(12, 61).await;
    let left = h.app.hit.groups[0].id;
    // A second tab in the left group; it is hidden once the first is picked again.
    h.command(Command::FocusSide(Side::Left));
    h.command(Command::NewTerminal);
    h.settle("two tabs on the left", |h| {
        h.app.groups.get(left).unwrap().tabs.len() == 2 && h.shells_fit()
    })
    .await;
    let size_of = |h: &Harness, i: usize| {
        let tab = &h.app.groups.get(left).unwrap().tabs[i];
        let (r, c) = tab.run.screen().screen().size();
        (r.get(), c.get())
    };
    let hidden_before = size_of(&h, 0);
    let d = h.app.hit.dividers[0].rect;
    h.mouse(MouseEventKind::Down(MouseButton::Left), d.x, 5);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), d.x + 8, 5);
    h.settle("the left group wider", |h| {
        h.app.hit.dividers[0].rect.x == d.x + 8 && h.shells_fit()
    })
    .await;
    assert_eq!(size_of(&h, 0), hidden_before, "the hidden tab keeps its last size");
    assert_ne!(size_of(&h, 1).1, hidden_before.1);
    // The client goes away mid-drag; the next one's motion must not move the divider.
    h.attach(2);
    h.settle("the new client's frame", |h| h.app.hit.dividers.len() == 1)
        .await;
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 40, 5);
    h.settle("a frame", |h| !h.app.needs_redraw).await;
    assert_eq!(h.app.hit.dividers[0].rect.x, d.x + 8);
    assert!(
        h.pointer_shapes().iter().all(|s| s != "ew-resize"),
        "{:?}",
        h.pointer_shapes()
    );
}

// jmux_layout-c: tab drag and drop. Each shell prints its own `TAG10<n>`, so a screen says which
// shell it is.

/// `n` tabs in one group, shell `i` having printed `TAG10<i>`.
async fn tagged_tabs(rows: u16, cols: u16, n: usize) -> Harness {
    let mut h = Harness::new(rows, cols).await;
    h.settle_text("sh ○").await;
    for i in 0..n {
        if i > 0 {
            h.command(Command::NewTerminal);
            h.settle("a new tab", |h| h.tab_count() == i + 1 && h.shells_fit())
                .await;
        }
        h.type_text(&format!("echo TAG$((100+{i}))\n"));
        h.settle_text(&format!("TAG10{i}")).await;
    }
    h
}

/// Which shells `group` holds, in tab order, by the tag each printed.
fn tags(h: &Harness, group: u64) -> Vec<usize> {
    let g = h.app.groups.get(group).unwrap();
    g.tabs
        .iter()
        .map(|t| {
            let text = t.run.screen().screen().contents();
            (0..9)
                .find(|i| text.contains(&format!("TAG10{i}")))
                .expect("a tagged shell")
        })
        .collect()
}

impl Harness {
    async fn frame(&mut self) {
        self.settle("a frame", |h| !h.app.needs_redraw).await;
    }

    /// The cell inside tab `index` of `group`'s bar (left of its close button).
    fn tab_cell(&self, group: u64, index: usize) -> (u16, u16) {
        let g = self.app.hit.group(group).unwrap();
        let t = g.tabs.iter().find(|t| t.index == index).unwrap();
        (t.x0 + 1, g.bar.y)
    }

    /// Presses at `from`, moves to `to` (a frame drawn there) and releases.
    async fn drag(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.press_and_move(from, to).await;
        self.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1);
        self.frame().await;
    }

    async fn press_and_move(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
        self.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
        self.frame().await;
    }

    fn bg_at(&self, x: u16, y: u16) -> vt100::Color {
        self.screen.screen().cell(y, x).unwrap().bgcolor()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_a_tab_drags_along_its_bar_and_keeps_its_shell() {
    let mut h = tagged_tabs(12, 61, 3).await;
    let g = h.app.groups.active;
    assert_eq!(tags(&h, g), [0, 1, 2]);
    let bar = h.app.hit.group(g).unwrap().bar;
    // First tab to past the last one.
    let from = h.tab_cell(g, 0);
    h.drag(from, (bar.x + bar.width - 1, bar.y)).await;
    assert_eq!(tags(&h, g), [1, 2, 0]);
    assert_eq!(h.app.groups.get(g).unwrap().active, 2, "the dragged tab is active");
    h.settle_text("TAG100").await;
    // Last back to the start.
    let from = h.tab_cell(g, 2);
    h.drag(from, (bar.x, bar.y)).await;
    assert_eq!(tags(&h, g), [0, 1, 2]);
    // Dropped beside itself: the order stays, and it is still just a click on it.
    let from = h.tab_cell(g, 1);
    h.drag(from, from).await;
    assert_eq!((tags(&h, g), h.active()), (vec![0, 1, 2], 1));
    // Where it ends up, a keystroke still reaches the same live shell.
    h.type_text("echo AFTER$((1+1))\n");
    h.settle_text("AFTER2").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_a_tab_moves_to_the_other_groups_bar_and_an_emptied_group_goes() {
    let mut h = split_right(12, 61).await;
    let (left, right) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    // Tag the right shell, then give the left group a second tab.
    h.type_text("echo TAG$((100+1))\n");
    h.settle_text("TAG101").await;
    h.command(Command::FocusSide(Side::Left));
    h.type_text("echo TAG$((100+0))\n");
    h.settle_text("TAG100").await;
    h.command(Command::NewTerminal);
    h.settle("two tabs on the left", |h| {
        h.app.groups.get(left).unwrap().tabs.len() == 2 && h.shells_fit()
    })
    .await;
    h.type_text("echo TAG$((100+2))\n");
    h.settle_text("TAG102").await;
    assert_eq!((tags(&h, left), tags(&h, right)), (vec![0, 2], vec![1]));

    // Left tab 0 onto the right bar's far end.
    let rbar = h.app.hit.group(right).unwrap().bar;
    let from = h.tab_cell(left, 0);
    h.drag(from, (rbar.x + rbar.width - 1, rbar.y)).await;
    assert_eq!((tags(&h, left), tags(&h, right)), (vec![2], vec![1, 0]));
    assert_eq!(
        (h.app.groups.active, h.app.groups.get(right).unwrap().active),
        (right, 1)
    );
    h.settle("the shells sized to their groups", |h| h.shells_fit()).await;

    // The left group's only tab to the start of the right bar: the left group is gone, the other
    // fills the screen.
    let from = h.tab_cell(left, 0);
    h.drag(from, (rbar.x, rbar.y)).await;
    assert!(h.app.groups.get(left).is_none());
    assert_eq!(tags(&h, right), [2, 1, 0]);
    h.settle("the lone group to fill the width", |h| {
        h.app.hit.groups.len() == 1 && h.app.hit.groups[0].term.width == 61 && h.shells_fit()
    })
    .await;
    assert!(h.app.hit.dividers.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_a_drop_on_an_edge_splits_the_group_and_the_centre_joins_it() {
    let mut h = tagged_tabs(14, 61, 2).await;
    let g = h.app.groups.active;
    let term = h.app.hit.group(g).unwrap().term;
    let from = h.tab_cell(g, 0);
    // The right edge: a new group to the right takes shell 0.
    h.drag(from, (term.x + term.width - 1, term.y + term.height / 2)).await;
    h.settle("two groups, each shell sized", |h| {
        h.app.hit.groups.len() == 2 && h.shells_fit()
    })
    .await;
    let order = h.app.groups.order();
    assert_eq!(order.len(), 2);
    assert_eq!((tags(&h, order[0]), tags(&h, order[1])), (vec![1], vec![0]));
    assert_eq!(h.app.groups.active, order[1], "the new group has the focus");
    let (l, r) = (h.app.hit.groups[0].term, h.app.hit.groups[1].term);
    assert_eq!(l.width + r.width + 1, 61, "halves");
    assert!(r.width >= 28 && l.width >= 28);
    h.settle_text("TAG100").await;

    // The new group's only tab dropped on the other's top edge: it moves there as a split above
    // and the emptied group leaves, so there are two groups stacked.
    let tab = h.tab_cell(order[1], 0);
    let other = h.app.hit.group(order[0]).unwrap().term;
    h.drag(tab, (other.x + other.width / 2, other.y)).await;
    h.settle("two stacked groups", |h| {
        let g = &h.app.hit.groups;
        g.len() == 2 && g[0].term.y < g[1].term.y && g[0].term.x == g[1].term.x && h.shells_fit()
    })
    .await;
    let order = h.app.groups.order();
    assert_eq!((tags(&h, order[0]), tags(&h, order[1])), (vec![0], vec![1]));

    // The centre of the other group: joins it, right of its active tab.
    let tab = h.tab_cell(order[0], 0);
    let centre = h.app.hit.group(order[1]).unwrap().term;
    h.drag(tab, (centre.x + centre.width / 2, centre.y + centre.height / 2))
        .await;
    assert_eq!(h.app.groups.order(), vec![order[1]]);
    assert_eq!(tags(&h, order[1]), [1, 0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_a_drop_on_its_own_centre_or_outside_any_target_moves_nothing() {
    let mut h = tagged_tabs(14, 61, 2).await;
    let g = h.app.groups.active;
    let term = h.app.hit.group(g).unwrap().term;
    let from = h.tab_cell(g, 0);
    h.drag(from, (term.x + term.width / 2, term.y + term.height / 2)).await;
    assert_eq!((tags(&h, g), h.app.hit.groups.len()), (vec![0, 1], 1), "own centre");
    // The status bar is no target.
    let from = h.tab_cell(g, 0);
    h.drag(from, (5, 13)).await;
    assert_eq!((tags(&h, g), h.app.hit.groups.len()), (vec![0, 1], 1), "outside");
    // Nor is a divider's cell.
    h.command(Command::SplitRight);
    h.settle("two groups", |h| {
        h.app.hit.groups.len() == 2 && h.tab_count() == 3 && h.shells_fit()
    })
    .await;
    let d = h.app.hit.dividers[0].rect;
    let from = h.tab_cell(g, 0);
    h.drag(from, (d.x, 6)).await;
    assert_eq!(tags(&h, g).len(), 2);
    assert_eq!(h.app.hit.groups.len(), 2);
    assert!(h.app.drag.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_the_marker_and_the_shade_are_drawn_while_dragging() {
    let mut h = tagged_tabs(14, 61, 3).await;
    let g = h.app.groups.active;
    let (bar, term) = {
        let hit = h.app.hit.group(g).unwrap();
        (hit.bar, hit.term)
    };
    let before = h.bg_at(term.x + term.width - 2, term.y + 5);
    // Over the bar, at the gap before tab 1: one cell marked.
    let from = h.tab_cell(g, 0);
    let gap_x = h.app.hit.group(g).unwrap().tabs[1].x0;
    h.press_and_move(from, (gap_x + 1, bar.y)).await;
    assert_ne!(h.bg_at(gap_x, bar.y), h.bg_at(gap_x + 1, bar.y), "the marker cell");
    let marker = h.bg_at(gap_x, bar.y);
    assert_ne!(h.bg_at(gap_x - 1, bar.y), marker, "one cell wide");
    // Over the terminal's right edge: the right half is shaded, the left is not.
    h.mouse(
        MouseEventKind::Drag(MouseButton::Left),
        term.x + term.width - 1,
        term.y + 5,
    );
    h.frame().await;
    let shade = h.bg_at(term.x + term.width - 2, term.y + 5);
    assert_ne!(shade, before);
    assert_eq!(h.bg_at(term.x + term.width - 1, term.y + term.height - 1), shade);
    assert_eq!(h.bg_at(term.x + term.width / 2, term.y), shade, "from the middle");
    assert_ne!(h.bg_at(term.x + term.width / 2 - 1, term.y), shade, "not the left half");
    assert_ne!(h.bg_at(gap_x, bar.y), marker, "the marker moved away");
    // The centre shades all of it.
    h.mouse(
        MouseEventKind::Drag(MouseButton::Left),
        term.x + term.width / 2,
        term.y + 5,
    );
    h.frame().await;
    assert_eq!(h.bg_at(term.x, term.y), shade);
    // Released outside: nothing stays drawn shaded.
    h.mouse(MouseEventKind::Up(MouseButton::Left), 3, 13);
    h.frame().await;
    assert_ne!(h.bg_at(term.x, term.y), shade);
    assert_eq!(tags(&h, g), [0, 1, 2]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_a_detach_mid_drag_clears_it_and_a_reattach_shows_the_moved_layout() {
    let mut h = tagged_tabs(14, 61, 2).await;
    let g = h.app.groups.active;
    let term = h.app.hit.group(g).unwrap().term;
    let from = h.tab_cell(g, 0);
    let edge = (term.x + term.width - 1, term.y + 5);
    h.press_and_move(from, edge).await;
    assert!(h.app.drag.is_some());
    h.attach(2);
    assert!(h.app.drag.is_none(), "the drag belonged to the old client");
    h.mouse(MouseEventKind::Up(MouseButton::Left), edge.0, edge.1);
    h.frame().await;
    assert_eq!((h.app.hit.groups.len(), tags(&h, g)), (1, vec![0, 1]));
    // A finished drag's layout comes back on the next attach.
    let from = h.tab_cell(g, 0);
    h.drag(from, edge).await;
    h.settle("two groups", |h| h.app.hit.groups.len() == 2 && h.shells_fit())
        .await;
    let before: Vec<_> = h.app.hit.groups.iter().map(|g| (g.id, g.term)).collect();
    h.attach(3);
    h.settle("the same two groups", |h| {
        h.app.hit.groups.iter().map(|g| (g.id, g.term)).collect::<Vec<_>>() == before && h.shells_fit()
    })
    .await;
    let order = h.app.groups.order();
    assert_eq!((tags(&h, order[0]), tags(&h, order[1])), (vec![1], vec![0]));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_a_tab_closed_mid_drag_is_not_replaced_by_the_one_that_took_its_index() {
    let mut h = tagged_tabs(14, 61, 3).await;
    let g = h.app.groups.active;
    let term = h.app.hit.group(g).unwrap().term;
    let edge = (term.x + term.width - 1, term.y + 5);
    // Pick up tab 0 (it is the active one), then it goes away while the pointer is over the edge.
    let from = h.tab_cell(g, 0);
    h.press_and_move(from, edge).await;
    h.command(Command::CloseTerminal);
    h.settle("tab 0 closed", |h| h.tab_count() == 2).await;
    assert_eq!(tags(&h, g), [1, 2]);
    h.mouse(MouseEventKind::Up(MouseButton::Left), edge.0, edge.1);
    h.frame().await;
    assert_eq!((h.app.hit.groups.len(), tags(&h, g)), (1, vec![1, 2]), "nothing moved");
    assert!(h.app.drag.is_none());
    // The dragged tab's group losing it by other means moves nothing either.
    let from = h.tab_cell(g, 0);
    h.press_and_move(from, edge).await;
    h.app.groups.close_tab(g, 0);
    h.mouse(MouseEventKind::Up(MouseButton::Left), edge.0, edge.1);
    h.frame().await;
    assert_eq!((h.app.hit.groups.len(), tags(&h, g)), (1, vec![2]));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_c_a_group_too_narrow_to_split_takes_an_edge_drop_as_the_centre() {
    // Three groups across 61 columns: the middle one is about 15 wide, under two minimum groups.
    let mut h = Harness::new(12, 61).await;
    h.settle_text("sh ○").await;
    for _ in 0..2 {
        h.command(Command::SplitRight);
        let n = h.app.hit.groups.len() + 1;
        h.settle("another group", |h| h.app.hit.groups.len() == n && h.shells_fit())
            .await;
    }
    let order = h.app.groups.order();
    let narrow = h.app.hit.group(order[1]).unwrap().term;
    assert!(narrow.width <= 2 * jtui::split::MIN_GROUP_WIDTH, "{narrow:?}");
    let from = h.tab_cell(order[0], 0);
    // The narrow group's left edge cell: a centre drop would join it, a split would add a group.
    h.drag(from, (narrow.x, narrow.y + 3)).await;
    assert!(h.app.groups.get(order[0]).is_none(), "the emptied group left");
    assert_eq!(h.app.groups.get(order[1]).unwrap().tabs.len(), 2, "joined, not split");
    assert_eq!(h.app.groups.order().len(), 2);
}

// ---- jmux_layout-d: inside a terminal ----

fn b64(text: &str) -> String {
    jtui::clipboard::base64(text.as_bytes())
}

impl Harness {
    /// The first screen row containing `needle`.
    fn row_of(&self, needle: &str) -> u16 {
        (0..self.rows)
            .find(|r| self.row(*r).contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{}", self.text()))
    }

    fn release(&mut self, x: u16, y: u16) {
        self.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }

    /// A press and release (one click) at `x`, `y`.
    fn click_at(&mut self, x: u16, y: u16) {
        self.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        self.release(x, y);
    }

    fn mouse_with(&mut self, kind: MouseEventKind, x: u16, y: u16, modifiers: KeyModifiers) {
        self.input(Input::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers,
        }));
    }

    /// Whether the client was sent an OSC 52 copy of exactly `text`.
    fn copied(&self, text: &str) -> bool {
        let want = format!("]52;c;{}\x07", b64(text));
        String::from_utf8_lossy(&self.raw).contains(&want)
    }

    fn selected_text(&self) -> Option<String> {
        self.app
            .groups
            .active_group()
            .active_tab()
            .unwrap()
            .run
            .copy_selection()
    }

    fn scrollback(&self) -> usize {
        self.app
            .groups
            .active_group()
            .active_tab()
            .unwrap()
            .run
            .scrollbar_extent()
            .0
    }

    /// The number the first row of the shell shows (after `seq`).
    fn top_number(&self) -> u32 {
        let row = self.row(1);
        row.split_whitespace()
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no number on the shell's first row: {row:?}"))
    }

    /// Whether the shell's first row is the command line that printed the numbers (the oldest
    /// output) or a number below `below`.
    fn at_oldest(&self, below: u32) -> bool {
        let row = self.row(1);
        row.split_once("$ ")
            .is_some_and(|(_, line)| line.starts_with("seq 1 200"))
            || row
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .is_some_and(|n| n < below)
    }

    /// Prints 1..=200 and waits for the end of it.
    async fn seq_200(&mut self) {
        self.type_text("seq 1 200\n");
        self.settle("the end of seq", |h| (0..h.rows).any(|r| h.row(r).trim() == "200"))
            .await;
        self.frame().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_dragging_selects_and_copy_sends_osc_52() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("printf 'ONE_%s\\n' TWO\n");
    h.settle("the output", |h| h.text().contains("ONE_TWO")).await;
    h.frame().await;
    let row = h.row_of("ONE_TWO");
    // Nothing selected: Copy sends nothing.
    h.command(Command::Copy);
    h.frame().await;
    assert!(!String::from_utf8_lossy(&h.raw).contains("]52;"));
    // Press on the O, drag past the last O of TWO (the head cell is excluded: 7 columns).
    h.mouse(MouseEventKind::Down(MouseButton::Left), 0, row);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 7, row);
    h.frame().await;
    assert_eq!(h.pointer_shapes().last().map(String::as_str), Some("text"));
    let selection_bg = vt100::Color::Rgb(0x26, 0x4F, 0x78);
    assert_eq!(h.bg_at(3, row), selection_bg, "the dragged cells are drawn selected");
    assert_ne!(h.bg_at(9, row), selection_bg, "the cells past it are not");
    h.release(7, row);
    assert_eq!(h.selected_text().as_deref(), Some("ONE_TWO"));
    h.command(Command::Copy);
    h.settle("the copy to reach the client", |h| h.copied("ONE_TWO")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_a_click_selects_nothing_and_double_and_triple_clicks_take_a_word_and_the_row() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("printf 'see %s done\\n' /usr/lib/x_y\n");
    h.settle("the output", |h| h.text().lines().any(|l| l.starts_with("see /usr")))
        .await;
    h.frame().await;
    let row = (0..h.rows).find(|r| h.row(*r).starts_with("see /usr")).unwrap();
    h.click_at(1, row);
    assert_eq!(h.selected_text(), None, "a click selects no cell");
    // Double click inside the path: the path-like run.
    h.click_at(8, row);
    h.click_at(8, row);
    assert_eq!(h.selected_text().as_deref(), Some("/usr/lib/x_y"));
    h.command(Command::Copy);
    h.settle("the copy", |h| h.copied("/usr/lib/x_y")).await;
    // A third click in quick succession: the whole row.
    h.click_at(8, row);
    assert_eq!(h.selected_text().as_deref(), Some("see /usr/lib/x_y done"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_a_paste_types_into_the_shell_bracketed_when_it_asked() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    // bash's readline (bash as sh) turns bracketed paste on at its prompt, where a pasted newline
    // doesn't run the line: Enter does, in either shell.
    h.input(Input::Paste("echo PASTED_$((6+1))".into()));
    h.type_text("\r");
    h.settle_text("PASTED_7").await;
    // `cat -v` shows the bracket markers a program that asked for them receives.
    h.type_text("printf '\\033[?2004h'; cat -v\n");
    h.settle("bracketed paste on", |h| {
        h.app
            .groups
            .active_group()
            .active_tab()
            .unwrap()
            .run
            .screen()
            .screen()
            .bracketed_paste()
    })
    .await;
    h.input(Input::Paste("hello\n".into()));
    h.settle_text("^[[200~hello").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_a_program_with_mouse_reporting_gets_the_mouse_and_shift_selects_instead() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text(
        "printf 'ABC_%s\\n' DEF; printf '\\033[?1000h\\033[?1006h'; stty raw -echo; dd bs=1 count=8 2>/dev/null | od -An -tx1\n",
    );
    h.settle("mouse reporting on", |h| {
        h.app
            .groups
            .active_group()
            .active_tab()
            .is_some_and(|t| t.run.screen().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
    })
    .await;
    h.settle("the output", |h| h.text().lines().any(|l| l.starts_with("ABC_DEF")))
        .await;
    h.frame().await;
    let row = h.row_of("ABC_DEF");
    // A drag is the program's: reported, nothing selected.
    h.mouse(MouseEventKind::Down(MouseButton::Left), 0, row);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 5, row);
    h.release(5, row);
    h.settle_text("1b 5b 3c 30 3b 31 3b").await;
    assert_eq!(h.selected_text(), None);
    assert!(h.app.drag.is_none());
    // Shift keeps the mouse for jmux's selection.
    let shift = KeyModifiers::SHIFT;
    h.mouse_with(MouseEventKind::Down(MouseButton::Left), 0, row, shift);
    h.mouse_with(MouseEventKind::Drag(MouseButton::Left), 7, row, shift);
    h.mouse_with(MouseEventKind::Up(MouseButton::Left), 7, row, shift);
    assert_eq!(h.selected_text().as_deref(), Some("ABC_DEF"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_the_wheel_and_the_scrollback_keys_scroll_and_a_typed_key_returns_to_the_live_screen() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.seq_200().await;
    let live = h.top_number();
    assert_eq!(h.scrollback(), 0);
    h.mouse(MouseEventKind::ScrollUp, 5, 5);
    h.frame().await;
    assert_eq!(h.top_number(), live - 3, "the wheel is three lines");
    h.mouse(MouseEventKind::ScrollDown, 5, 5);
    h.frame().await;
    assert_eq!(h.top_number(), live);
    // A page is the screen's rows less one (10 rows).
    h.key(KeyCode::PageUp, KeyModifiers::SHIFT);
    h.frame().await;
    assert_eq!(h.top_number(), live - 9);
    h.key(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    h.frame().await;
    assert_eq!(h.top_number(), live - 10);
    h.key(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    h.key(KeyCode::PageDown, KeyModifiers::SHIFT);
    h.frame().await;
    assert_eq!(h.top_number(), live);
    h.key(KeyCode::Home, KeyModifiers::CONTROL);
    h.frame().await;
    assert!(h.at_oldest(2), "the oldest output: {:?}", h.row(1));
    h.key(KeyCode::End, KeyModifiers::CONTROL);
    h.frame().await;
    assert_eq!(h.top_number(), live);
    // A key typed while scrolled back goes to the shell and the view returns to the live screen.
    h.mouse(MouseEventKind::ScrollUp, 5, 5);
    assert!(h.scrollback() > 0);
    h.key(KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(h.scrollback(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_the_scroll_bar_is_drawn_and_a_click_or_drag_on_it_scrolls() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    // Nothing to scroll yet: no thumb. The shell leaves the bar's column alone.
    let g = h.app.hit.groups[0].clone();
    assert_eq!((g.text.width, g.scroll.track.x, g.scroll.track.width), (59, 59, 1));
    assert_eq!(g.scroll.track.height, g.text.height);
    assert!(g.scroll.thumb.is_none());
    // The shell itself is sized to the text area, one column narrower than the terminal area.
    let size = h
        .app
        .groups
        .active_group()
        .active_tab()
        .unwrap()
        .run
        .screen()
        .screen()
        .size();
    assert_eq!((size.0.get(), size.1.get()), (g.text.height, 59));
    h.seq_200().await;
    let live = h.top_number();
    let g = h.app.hit.groups[0].clone();
    let thumb = g.scroll.thumb.expect("a thumb once there is scrollback");
    assert_eq!(
        thumb.start + thumb.len,
        g.scroll.track.height,
        "at the bottom while live"
    );
    let slider = vt100::Color::Rgb(0x42, 0x42, 0x42);
    let (x, top) = (g.scroll.track.x, g.scroll.track.y);
    let bottom = top + g.scroll.track.height - 1;
    // The thumb's end is a half block in the slider colour (or a full cell of it).
    let cell = h.screen.screen().cell(bottom, x).unwrap();
    assert!(cell.fgcolor() == slider || cell.bgcolor() == slider, "{cell:?}");
    // A click above the thumb jumps there: the top of the scrollback.
    h.click_at(x, top);
    h.frame().await;
    assert!(h.at_oldest(live / 2), "scrolled far back: {:?}", h.row(1));
    assert!(h.app.drag.is_none(), "released");
    // Grab the thumb (now at the top) and drag it down, past the track's end (the drag owns the
    // mouse wherever the pointer goes).
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, top);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), x, bottom + 1);
    h.release(x, bottom + 1);
    h.frame().await;
    assert_eq!(h.scrollback(), 0, "dragged back to the live screen");
    assert_eq!(h.top_number(), live);
    h.type_text("printf '%059d\\n' 0\n");
    h.settle("a full-width line", |h| h.text().contains(&"0".repeat(59)))
        .await;
    h.frame().await;
    let full = h.row_of(&"0".repeat(59));
    assert_ne!(
        h.screen
            .screen()
            .cell(full, 59)
            .map(|c| c.contents().to_string())
            .as_deref(),
        Some("0"),
        "the bar's column holds no shell text"
    );
    // The wheel over the bar scrolls too.
    h.mouse(MouseEventKind::ScrollUp, x, top + 3);
    assert_eq!(h.scrollback(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_a_new_client_starts_with_no_drag_and_the_selection_stays() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("printf 'KEEP_%s\\n' ME\n");
    h.settle("the output", |h| h.text().contains("KEEP_ME")).await;
    h.frame().await;
    let row = h.row_of("KEEP_ME");
    h.mouse(MouseEventKind::Down(MouseButton::Left), 0, row);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 7, row);
    assert!(h.app.drag.is_some());
    h.attach(2);
    assert!(h.app.drag.is_none(), "a new client starts with no drag");
    assert_eq!(h.selected_text().as_deref(), Some("KEEP_ME"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_the_wheel_goes_to_a_reporting_program_and_shift_scrolls_instead() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.seq_200().await;
    h.type_text("printf '\\033[?1000h\\033[?1006h'; stty raw -echo; dd bs=1 count=6 2>/dev/null | od -An -tx1\n");
    h.settle("mouse reporting on", |h| {
        h.app
            .groups
            .active_group()
            .active_tab()
            .is_some_and(|t| t.run.screen().screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None)
    })
    .await;
    h.frame().await;
    h.mouse(MouseEventKind::ScrollUp, 5, 5);
    // The program got the wheel report (button 64), and the scrollback did not move.
    h.settle_text("1b 5b 3c 36 34 3b").await;
    assert_eq!(h.scrollback(), 0, "the wheel was the program's, not the scrollback's");
    h.mouse_with(MouseEventKind::ScrollUp, 5, 5, KeyModifiers::SHIFT);
    assert_eq!(h.scrollback(), 3, "Shift keeps the wheel for the scrollback");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_shift_click_extends_the_selection() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("printf 'ALPHA %s\\n' BETA\n");
    h.settle("the output", |h| h.text().lines().any(|l| l.starts_with("ALPHA BETA")))
        .await;
    h.frame().await;
    let row = (0..h.rows).find(|r| h.row(*r).starts_with("ALPHA BETA")).unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), 0, row);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 5, row);
    h.release(5, row);
    assert_eq!(h.selected_text().as_deref(), Some("ALPHA"));
    h.mouse_with(MouseEventKind::Down(MouseButton::Left), 10, row, KeyModifiers::SHIFT);
    h.mouse_with(MouseEventKind::Up(MouseButton::Left), 10, row, KeyModifiers::SHIFT);
    assert_eq!(
        h.selected_text().as_deref(),
        Some("ALPHA BETA"),
        "extended from the anchor"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_d_a_copy_while_no_client_is_attached_is_not_sent_to_the_next_one() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("printf 'STALE_%s\\n' COPY\n");
    h.settle("the output", |h| h.text().contains("STALE_COPY")).await;
    h.frame().await;
    let row = h.row_of("STALE_COPY");
    h.mouse(MouseEventKind::Down(MouseButton::Left), 0, row);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 10, row);
    h.release(10, row);
    assert_eq!(h.selected_text().as_deref(), Some("STALE_COPY"));
    // The client goes away, then a copy runs and a frame is due with nobody attached.
    let conn = h.conn;
    h.app.handle(AppEvent::Closed { conn });
    h.app.execute(Command::Copy);
    h.app.redraw(&mut h.terminal).unwrap();
    // A new client gets a fresh screen without the old copy.
    h.attach(2);
    h.frame().await;
    assert!(!String::from_utf8_lossy(&h.raw).contains("]52;"), "no stale OSC 52");
}

// ---- jmux_layout-e: search and links in a terminal, and the whole done-when ----

/// URLs the test opener was asked to open (a recorder: no test launches a browser). Tests use URLs
/// of their own, so parallel tests do not see each other's.
static OPENED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn record_url(url: &str) -> Result<(), String> {
    OPENED.lock().unwrap().push(url.to_string());
    Ok(())
}

fn opened(url: &str) -> usize {
    OPENED.lock().unwrap().iter().filter(|u| *u == url).count()
}

/// Three lines holding "MARK" (the first two scroll off a 12-row screen), none of them in the
/// command's own text.
const MARKS: &str =
    "printf 'MAR%s one\\n' K; seq 1 30; printf 'MAR%s two\\n' K; seq 31 40; printf 'MAR%s three\\n' K\n";

impl Harness {
    fn search_label(&self) -> String {
        self.app.search.as_ref().map(|s| s.label()).unwrap_or_default()
    }

    /// The background colour of the cell, as the theme names it.
    fn bg_is(&self, row: u16, col: u16, key: &str) -> bool {
        let want = self.app.theme.get(key).bg;
        let got = self.screen.screen().cell(row, col).map(|c| c.bgcolor());
        match (want, got) {
            (Some(ratatui::style::Color::Rgb(r, g, b)), Some(vt100::Color::Rgb(r2, g2, b2))) => {
                (r, g, b) == (r2, g2, b2)
            }
            _ => false,
        }
    }

    async fn search_open(&mut self) {
        self.command(Command::Search(crate::command::SearchCommand::Open));
        self.settle("the search bar", |h| h.app.search.is_some() && !h.app.needs_redraw)
            .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_search_finds_counts_and_steps_with_wrap_and_scrolls_to_the_match() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text(MARKS);
    h.settle_text("MARK three").await;
    assert!(
        h.scrollback() == 0 && !h.text().contains("MARK one"),
        "the first mark has scrolled off"
    );
    h.search_open().await;
    h.type_text("mark");
    h.settle("three matches, the newest current", |h| h.search_label() == "3 of 3")
        .await;
    h.settle_text("3 of 3").await;
    assert!(h.row(1).contains("mark"), "the query is in the bar: {:?}", h.row(1));
    // Forward from the last wraps to the first, which is in the scrollback: the view goes there.
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.settle("the first match", |h| {
        h.search_label() == "1 of 3" && h.text().contains("MARK one")
    })
    .await;
    assert!(h.scrollback() > 0);
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.settle("the second match", |h| {
        h.search_label() == "2 of 3" && h.text().contains("MARK two")
    })
    .await;
    h.key(KeyCode::F(3), KeyModifiers::SHIFT);
    h.settle("back to the first", |h| h.search_label() == "1 of 3").await;
    h.key(KeyCode::Enter, KeyModifiers::SHIFT);
    h.settle("back wraps to the last", |h| {
        h.search_label() == "3 of 3" && h.text().contains("MARK three")
    })
    .await;
    h.settle_text("3 of 3").await;
    // Case sensitivity: "mark" no longer matches "MARK".
    h.key(KeyCode::Char('c'), KeyModifiers::ALT);
    h.settle("no results", |h| h.search_label() == "No results").await;
    h.settle_text("No results").await;
    // Stepping with no matches stays put, and does not panic.
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.key(KeyCode::Enter, KeyModifiers::SHIFT);
    h.type_text("x");
    h.settle("still no results", |h| {
        h.app.search.as_ref().is_some_and(|s| s.field.text == "markx")
    })
    .await;
    assert_eq!(h.search_label(), "No results");
    assert!(h.app.search.as_ref().unwrap().current.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_the_current_match_and_the_others_are_drawn_in_the_find_colours() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("printf 'MAR%s MAR%s\\n' K K\n");
    h.settle("the output", |h| h.text().lines().any(|l| l.starts_with("MARK MARK")))
        .await;
    h.search_open().await;
    h.type_text("MARK");
    h.settle("two matches", |h| h.search_label() == "2 of 2").await;
    h.frame().await;
    h.settle("the highlights drawn", |h| {
        let row = (0..h.rows).find(|r| h.row(*r).starts_with("MARK MARK"));
        row.is_some_and(|r| h.bg_is(r, 5, "editor.findMatch") && h.bg_is(r, 0, "editor.findMatchHighlight"))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_the_program_gets_no_keys_while_the_bar_is_open_and_esc_gives_them_back() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.search_open().await;
    h.type_text("qzx");
    h.key(KeyCode::Enter, KeyModifiers::NONE);
    h.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
    h.settle("the query", |h| {
        h.app.search.as_ref().is_some_and(|s| s.field.text == "qzx")
    })
    .await;
    h.frame().await;
    // The shell saw none of it: its own screen has no "qzx".
    let shell_text = h
        .app
        .groups
        .active_group()
        .active_tab()
        .unwrap()
        .run
        .screen()
        .screen()
        .contents();
    assert!(!shell_text.contains("qzx"), "{shell_text}");
    // An edit key and a paste go to the query too.
    h.key(KeyCode::Backspace, KeyModifiers::NONE);
    h.input(Input::Paste("ab\ncd".into()));
    h.settle("the edited query", |h| {
        h.app.search.as_ref().is_some_and(|s| s.field.text == "qzabcd")
    })
    .await;
    h.key(KeyCode::Esc, KeyModifiers::NONE);
    h.settle("the bar closed", |h| h.app.search.is_none()).await;
    h.type_text("echo BACK_$((20+2))\n");
    h.settle_text("BACK_22").await;
    assert!(!h.text().contains("qzx"));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_the_bar_belongs_to_its_tab_and_goes_with_it() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.search_open().await;
    h.type_text("zzz");
    h.command(Command::NewTerminal);
    h.settle("the second tab", |h| h.tab_count() == 2 && h.active() == 1)
        .await;
    // The bar is over the first tab: the second one's keys are its shell's.
    h.type_text("echo TWO_$((1+1))\n");
    h.settle_text("TWO_2").await;
    assert!(
        h.app.search.as_ref().is_some_and(|s| s.field.text == "zzz"),
        "the query is kept"
    );
    h.command(Command::PrevTab);
    h.settle("the bar again", |h| h.active() == 0 && h.text().contains("zzz"))
        .await;
    // Closing the tab drops the bar.
    h.command(Command::CloseTerminal);
    h.settle("one tab", |h| h.tab_count() == 1).await;
    assert!(h.app.search.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_a_ctrl_click_on_a_url_opens_exactly_it_and_a_plain_click_does_not() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text("printf 'see https:/%s now\\n' /example.com/e-single\n");
    h.settle("the output", |h| {
        h.text().lines().any(|l| l.starts_with("see https://example.com"))
    })
    .await;
    h.frame().await;
    let row = (0..h.rows).find(|r| h.row(*r).starts_with("see https://")).unwrap();
    let col = h.row(row).find("example").unwrap() as u16;
    let url = "https://example.com/e-single";
    h.click_at(col, row);
    h.click_at(col, row);
    h.app.drag = None;
    h.mouse_with(MouseEventKind::Down(MouseButton::Left), 1, row, KeyModifiers::CONTROL);
    assert!(h.app.drag.is_some(), "a Ctrl+click off a link is a normal press");
    h.mouse_with(MouseEventKind::Up(MouseButton::Left), 1, row, KeyModifiers::CONTROL);
    h.mouse_with(MouseEventKind::Down(MouseButton::Left), col, row, KeyModifiers::CONTROL);
    h.mouse_with(MouseEventKind::Up(MouseButton::Left), col, row, KeyModifiers::CONTROL);
    h.settle("the link opened", |_| opened(url) >= 1).await;
    assert_eq!(
        opened(url),
        1,
        "plain clicks and a Ctrl+click beside the link open nothing"
    );
    // Cmd (Super) is the other modifier, and the press selected nothing.
    h.mouse_with(MouseEventKind::Down(MouseButton::Left), col, row, KeyModifiers::SUPER);
    h.settle("opened again", |_| opened(url) >= 2).await;
    assert_eq!(h.selected_text(), None);
    assert!(h.app.drag.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_a_url_wrapped_over_two_rows_opens_whole() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    let tail = "a".repeat(80);
    h.type_text(&format!("printf 'https:/%s\\n' /example.com/{tail}\n"));
    h.settle("the output", |h| {
        h.text().lines().any(|l| l.starts_with("https://example.com"))
    })
    .await;
    h.frame().await;
    let row = (0..h.rows).find(|r| h.row(*r).starts_with("https://")).unwrap();
    let url = format!("https://example.com/{tail}");
    assert!(!h.row(row).contains(&tail), "it wraps");
    // On the second row.
    h.mouse_with(
        MouseEventKind::Down(MouseButton::Left),
        3,
        row + 1,
        KeyModifiers::CONTROL,
    );
    h.settle("the whole URL opened", |_| opened(&url) == 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_a_full_screen_program_gets_its_keys_and_mouse() {
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    // The alternate screen, application cursor keys and SGR mouse reporting, then raw input read
    // back as hex: Up, then a press at column 3, row 3 (1-based).
    h.type_text(
        "printf '\\033[?1049h\\033[?1h\\033[?1000h\\033[?1006h'; stty raw -echo; dd bs=1 count=12 2>/dev/null | od -An -tx1\n",
    );
    h.settle("the program's modes", |h| {
        h.app.groups.active_group().active_tab().is_some_and(|t| {
            let r = t.run.screen();
            let s = r.screen();
            s.alternate_screen() && s.application_cursor() && s.mouse_protocol_mode() != vt100::MouseProtocolMode::None
        })
    })
    .await;
    h.frame().await;
    h.key(KeyCode::Up, KeyModifiers::NONE);
    // Terminal text starts below the tab bar: screen row 3 is the shell's row 2.
    h.mouse(MouseEventKind::Down(MouseButton::Left), 2, 3);
    h.settle_text("1b 4f 41 1b 5b 3c 30 3b 33 3b 33 4d").await;
    assert_eq!(h.selected_text(), None, "the press was the program's, not a selection");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_layout_e_a_real_vi_gets_its_keys() {
    let Some(vi) = ["/usr/bin/vi", "/bin/vi"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
    else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("jmux-vi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    let mut h = Harness::new(12, 60).await;
    h.settle_text("sh ○").await;
    h.type_text(&format!("{vi} {}\n", file.display()));
    h.settle("vi on the alternate screen", |h| {
        h.app
            .groups
            .active_group()
            .active_tab()
            .is_some_and(|t| t.run.screen().screen().alternate_screen())
    })
    .await;
    h.frame().await;
    h.type_text("iFROM_VI");
    h.key(KeyCode::Esc, KeyModifiers::NONE);
    h.type_text(":wq\n");
    h.settle("vi to exit", |h| {
        h.app
            .groups
            .active_group()
            .active_tab()
            .is_some_and(|t| !t.run.screen().screen().alternate_screen())
    })
    .await;
    assert!(std::fs::read_to_string(&file).unwrap().contains("FROM_VI"));
    let _ = std::fs::remove_dir_all(&dir);
}
