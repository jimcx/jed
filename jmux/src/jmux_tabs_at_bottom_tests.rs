//! End-to-end tests of jmux_tabs_at_bottom (the `tabs_at_bottom` setting), driving the server `App`
//! headlessly with real shells and a settings file in a temporary directory.
//!
//! Covered by `jmux_tabs_at_bottom_*`: with the setting on each group's bar is drawn on its last
//! row and the shell starts on the first, also with a split, and the shell's size is the drawn
//! area's, the same height as with the bar on top (`..._the_bar_is_on_the_groups_last_row_...`,
//! `..._each_group_of_a_split_...`); clicking a tab, middle-clicking one and the bar's scroll arrows
//! work at the bottom (`..._clicks_...`, `..._scroll_arrows_...`); dragging a tab to another group's
//! bar and to a terminal half (the shade is drawn there, the drop splits) works
//! (`..._a_tab_drags_...`); toggling the file and Reload Settings moves the bars without
//! restarting the shells (`..._reload_...`); a bad value is a notice with its line and the default
//! applies (`..._a_bad_value_...`).

use std::num::NonZeroU16;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event as Input, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, Config};
use crate::command::Command;
use crate::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};

const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    app: App,
    events: UnboundedReceiver<AppEvent>,
    conn_rx: Receiver<ServerMsg>,
    terminal: Terminal<StreamBackend>,
    screen: vt100::Parser,
    cols: u16,
    dir: tempfile::TempDir,
}

impl Harness {
    /// A started session (one tab) at `rows` x `cols` whose settings file holds `text`.
    async fn new(rows: u16, cols: u16, text: &str) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(crate::settings::FILE_NAME);
        std::fs::write(&path, text).unwrap();
        let (settings, settings_errors) = crate::settings::load(&path);
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let config = Config {
            name: "work".into(),
            socket: PathBuf::new(),
            program: "/bin/sh".into(),
            cwd: std::env::temp_dir(),
            open_url: |_| Ok(()),
            dirs: None,
            settings,
            settings_path: Some(path),
            settings_errors,
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
            rows,
            cols,
            truecolor: true,
            term: "xterm".into(),
        };
        app.handle(AppEvent::Client { conn: 1, msg });
        let mut h = Harness {
            app,
            events,
            conn_rx,
            terminal: Terminal::new(StreamBackend::new(cols, rows)).unwrap(),
            screen: vt100::Parser::new(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap(), 0),
            cols,
            dir,
        };
        h.settle("a drawn frame", |h| !h.app.hit.groups.is_empty()).await;
        h
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
                self.text()
            );
            self.step().await;
        }
    }

    async fn frame(&mut self) {
        self.settle("a frame", |h| !h.app.needs_redraw).await;
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

    fn input(&mut self, input: Input) {
        self.app.handle(AppEvent::Client {
            conn: 1,
            msg: ClientMsg::Input(input),
        });
    }

    fn command(&mut self, command: Command) {
        let combo = self.app.keymap.binding_for(command).expect("a default key");
        self.input(Input::Key(KeyEvent::new(combo.code, combo.mods)));
    }

    fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        self.input(Input::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }));
    }

    fn click(&mut self, button: MouseButton, column: u16, row: u16) {
        self.mouse(MouseEventKind::Down(button), column, row);
    }

    fn shell_size(&self, group: u64) -> (u16, u16) {
        let tab = self.app.groups.get(group).unwrap().active_tab().unwrap();
        let (rows, cols) = tab.run.screen().screen().size();
        (rows.get(), cols.get())
    }

    fn shells_fit(&self) -> bool {
        let fit = |g: &crate::hit::GroupHit| self.shell_size(g.id) == (g.text.height, g.text.width);
        self.app.hit.groups.iter().all(fit)
    }

    fn tab_count(&self) -> usize {
        self.app.groups.tab_count()
    }

    fn active(&self) -> usize {
        self.app.groups.active_group().active
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

    fn run_ids(&self) -> Vec<jterm::run::RunId> {
        self.app.groups.tabs().map(|t| t.run.id).collect()
    }

    fn bg_at(&self, x: u16, y: u16) -> vt100::Color {
        self.screen.screen().cell(y, x).unwrap().bgcolor()
    }

    /// The cell inside tab `index` of `group`'s bar.
    fn tab_cell(&self, group: u64, index: usize) -> (u16, u16) {
        let g = self.app.hit.group(group).unwrap();
        let t = g.tabs.iter().find(|t| t.index == index).unwrap();
        (t.x0 + 1, g.bar.y)
    }

    async fn press_and_move(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
        self.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
        self.frame().await;
    }

    /// Opens tabs until there are `n`, waiting until the shells fit.
    async fn tabs(&mut self, n: usize) {
        for i in 1..n {
            self.command(Command::NewTerminal);
            self.settle("a new tab", |h| h.tab_count() == i + 1 && h.shells_fit())
                .await;
        }
    }

    /// Rewrites the settings file, reloads and waits for the notice.
    async fn reload(&mut self, text: &str) -> String {
        std::fs::write(self.dir.path().join(crate::settings::FILE_NAME), text).unwrap();
        self.app.notice = None;
        self.app.execute(Command::ReloadSettings);
        self.settle("the reload's notice", |h| h.app.notice.is_some()).await;
        self.frame().await;
        self.app.notice.clone().unwrap()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.app.shutdown();
    }
}

const BOTTOM: &str = "tabs_at_bottom = true\n";

#[tokio::test(flavor = "multi_thread")]
async fn jmux_tabs_at_bottom_the_bar_is_on_the_groups_last_row_and_the_shell_starts_on_the_first() {
    let mut h = Harness::new(12, 60, BOTTOM).await;
    // 12 rows: the status bar is the last, so the group's last row is 10.
    h.settle("the bar on row 10 and the prompt on row 0", |h| {
        h.row(10).contains("sh") && h.row(0).trim_end().ends_with('$')
    })
    .await;
    assert!(!h.row(0).contains('×'), "{}", h.text());
    let g = h.app.hit.groups[0].clone();
    assert_eq!((g.bar.y, g.bar.height, g.term.y, g.term.height), (10, 1, 0, 10));
    assert_eq!((g.text.y, g.scroll.track.y), (0, 0));
    h.settle("the shell fits", |h| h.shells_fit()).await;
    // The same height as with the bar on top; a new shell starts at the drawn size.
    let top = Harness::new(12, 60, "").await;
    assert_eq!(
        (top.app.hit.groups[0].bar.y, top.app.hit.groups[0].term.height),
        (0, 10)
    );
    h.command(Command::NewTerminal);
    h.settle("a second tab at the drawn size", |h| {
        h.tab_count() == 2 && h.shells_fit()
    })
    .await;
    assert_eq!(h.shell_size(g.id), (10, 59));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_tabs_at_bottom_each_group_of_a_split_has_its_bar_on_its_own_last_row() {
    let mut h = Harness::new(16, 61, BOTTOM).await;
    h.command(Command::SplitDown);
    h.settle("two stacked groups", |h| h.app.hit.groups.len() == 2 && h.shells_fit())
        .await;
    for g in &h.app.hit.groups {
        assert_eq!(g.bar.y, g.term.y + g.term.height, "the bar is below its terminal");
        assert_eq!(g.bar.width, g.term.width);
    }
    let (a, b) = (h.app.hit.groups[0].clone(), h.app.hit.groups[1].clone());
    assert!(a.bar.y < b.term.y, "the top group's bar is above the lower group");
    h.settle("a bar on each group's last row", |h| {
        h.row(a.bar.y).contains("sh") && h.row(b.bar.y).contains("sh")
    })
    .await;
    assert_eq!(b.bar.y, 14);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_tabs_at_bottom_clicks_on_the_bar_pick_and_close_tabs() {
    let mut h = Harness::new(12, 60, BOTTOM).await;
    h.tabs(3).await;
    h.settle("three tabs on the bottom bar", |h| h.row(10).contains("sh (3)"))
        .await;
    let tabs = h.app.hit.groups[0].tabs.clone();
    assert_eq!((tabs.len(), h.active()), (3, 2));
    h.click(MouseButton::Left, tabs[0].x0 + 1, 10);
    assert_eq!(h.active(), 0);
    h.mouse(MouseEventKind::Up(MouseButton::Left), tabs[0].x0 + 1, 10);
    // The top row is the terminal now: the same x there picks nothing.
    h.click(MouseButton::Left, tabs[1].x0 + 1, 0);
    h.mouse(MouseEventKind::Up(MouseButton::Left), tabs[1].x0 + 1, 0);
    assert_eq!(h.active(), 0);
    // The x of the second closes it.
    h.click(MouseButton::Left, tabs[1].close_x, 10);
    assert_eq!(h.titles(), ["sh ○", "sh (3) ○"]);
    h.settle("a redrawn bar", |h| !h.row(10).contains("sh (2)")).await;
    let tabs = h.app.hit.groups[0].tabs.clone();
    h.click(MouseButton::Middle, tabs[1].x0 + 1, 10);
    assert_eq!(h.titles(), ["sh ○"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_tabs_at_bottom_the_bars_scroll_arrows_work() {
    let mut h = Harness::new(10, 30, BOTTOM).await;
    h.tabs(5).await;
    // The group has 9 rows (the status bar is the tenth): its last is 8.
    h.settle("scroll arrows on the last row", |h| {
        h.row(8).starts_with('‹') && h.row(8).trim_end().ends_with('›')
    })
    .await;
    let arrows = h.app.hit.groups[0].arrows.expect("arrows");
    let before = h.app.groups.active_group().scroll;
    assert!(before > 0);
    h.click(MouseButton::Left, arrows.back, 8);
    assert_eq!(h.app.groups.active_group().scroll, before - 1);
    h.mouse(MouseEventKind::ScrollDown, 10, 8);
    assert_eq!(h.app.groups.active_group().scroll, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_tabs_at_bottom_a_tab_drags_to_another_bar_and_to_a_terminal_half() {
    let mut h = Harness::new(14, 61, BOTTOM).await;
    h.command(Command::SplitRight);
    h.settle("two groups", |h| h.app.hit.groups.len() == 2 && h.shells_fit())
        .await;
    let (left, right) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    h.command(Command::FocusSide(crate::group::Side::Left));
    h.command(Command::NewTerminal);
    h.settle("two tabs on the left", |h| {
        h.app.groups.get(left).unwrap().tabs.len() == 2 && h.shells_fit()
    })
    .await;
    // Left tab 0 onto the right bar's far end.
    let rbar = h.app.hit.group(right).unwrap().bar;
    assert_eq!(rbar.y, 12);
    let from = h.tab_cell(left, 0);
    assert_eq!(from.1, 12, "the tab to drag is on the bottom row");
    let end = (rbar.x + rbar.width - 1, rbar.y);
    h.press_and_move(from, end).await;
    h.mouse(MouseEventKind::Up(MouseButton::Left), end.0, end.1);
    h.frame().await;
    let counts = |h: &Harness| {
        (
            h.app.groups.get(left).unwrap().tabs.len(),
            h.app.groups.get(right).unwrap().tabs.len(),
        )
    };
    assert_eq!(counts(&h), (1, 2));
    h.settle("shells fit", |h| h.shells_fit()).await;

    // The right group's tab 0 onto the left terminal's right half: shaded, then a split.
    let term = h.app.hit.group(left).unwrap().term;
    let (x, y) = (term.x + term.width - 1, term.y + 3);
    let before = h.bg_at(x - 1, y);
    let from = h.tab_cell(right, 0);
    h.press_and_move(from, (x, y)).await;
    let shade = h.bg_at(x - 1, y);
    assert_ne!(shade, before);
    assert_eq!(
        h.bg_at(term.x + term.width / 2, term.y),
        shade,
        "from the middle, at the top"
    );
    assert_eq!(
        h.bg_at(x, term.y + term.height - 1),
        shade,
        "down to the terminal's last row"
    );
    assert_ne!(h.bg_at(term.x + term.width / 2 - 1, term.y), shade, "not the left half");
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    h.frame().await;
    h.settle("three groups, each shell sized", |h| {
        h.app.hit.groups.len() == 3 && h.shells_fit()
    })
    .await;
    assert!(h.app.hit.groups.iter().all(|g| g.bar.y == 12 && g.term.y == 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_tabs_at_bottom_reload_moves_the_bars_and_keeps_the_shells() {
    let mut h = Harness::new(12, 60, "").await;
    h.tabs(2).await;
    h.settle("the bar on top", |h| h.row(0).contains("sh (2)")).await;
    let ids = h.run_ids();
    let id = h.app.hit.groups[0].id;
    let size = h.shell_size(id);
    assert_eq!(h.app.hit.groups[0].bar.y, 0);
    h.reload(BOTTOM).await;
    h.settle("the bar on the last row", |h| {
        h.row(10).contains("sh (2)") && !h.row(0).contains("sh (2)")
    })
    .await;
    let g = h.app.hit.groups[0].clone();
    assert_eq!((g.bar.y, g.term.y), (10, 0));
    h.settle("shells fit", |h| h.shells_fit()).await;
    assert_eq!(h.shell_size(id), size, "the height is the same");
    assert_eq!(h.run_ids(), ids);
    // And back.
    h.reload("tabs_at_bottom = false\n").await;
    h.settle("the bar on top again", |h| h.row(0).contains("sh (2)")).await;
    assert_eq!(h.app.hit.groups[0].bar.y, 0);
    assert_eq!(h.run_ids(), ids);
    assert_eq!(h.shell_size(id), size);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_tabs_at_bottom_a_bad_value_is_reported_and_the_default_applies() {
    let mut h = Harness::new(12, 60, "scrollback = 10\ntabs_at_bottom = \"yes\"\n").await;
    assert!(!h.app.config.settings.tabs_at_bottom);
    assert_eq!(h.app.hit.groups[0].bar.y, 0, "the default: the bar on top");
    let notice = h.app.notice.clone().unwrap_or_default();
    assert!(
        notice.contains("line 2") && notice.contains("tabs_at_bottom"),
        "{notice:?}"
    );
    // A reload with a bad value applies the default again.
    h.reload(BOTTOM).await;
    assert_eq!(h.app.hit.groups[0].bar.y, 10);
    let notice = h.reload("tabs_at_bottom = 1\n").await;
    assert!(
        notice.contains("line 1") && notice.contains("true or false"),
        "{notice:?}"
    );
    assert_eq!(h.app.hit.groups[0].bar.y, 0);
}
