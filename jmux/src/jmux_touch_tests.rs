//! End-to-end tests of jmux_touch (`jmux attach --touch`: the touch layout of the tab bars), driving
//! the server `App` headlessly with real shells and a `Hello` carrying `touch`.
//!
//! Covered by `jmux_touch_*`: the bar is three rows with padded tabs, the title on the middle row
//! and the tab's background on all three, at the top and with `tabs_at_bottom`
//! (`..._the_bar_is_three_rows_...`); a tap on any row of a tab, padding included, focuses it
//! (`..._a_tap_on_any_row_...`); the three cells of `×` close it, the cell beside them does not,
//! and a running command still asks first (`..._the_close_target_...`, `..._closing_a_running_...`);
//! the three-cell arrows scroll an overflowing bar (`..._the_arrows_...`); a tab drags to another
//! bar and to a terminal half (`..._a_tab_drags_...`); the shells are two rows shorter than for a
//! plain client (`..._shells_are_two_rows_shorter_...`); a plain client taking the session over
//! gets the normal bar (`..._a_plain_client_taking_over_...`); a `Hello` without `touch` is
//! accepted (`..._a_hello_without_touch_...`); a tap on any row of the `+` at the bar's right end
//! opens a tab in that bar's group, and a plain client has no `+` (`..._the_plus_...`).

use std::num::NonZeroU16;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEventKind};
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
    rows: u16,
    cols: u16,
    _dir: tempfile::TempDir,
}

impl Harness {
    /// A started session (one tab) at `rows` x `cols` whose settings file holds `text`.
    async fn new(rows: u16, cols: u16, text: &str, touch: bool) -> Harness {
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
            touch,
        };
        app.handle(AppEvent::Client { conn: 1, msg });
        let mut h = Harness {
            app,
            events,
            conn_rx,
            terminal: Terminal::new(StreamBackend::new(cols, rows)).unwrap(),
            screen: vt100::Parser::new(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap(), 0),
            rows,
            cols,
            _dir: dir,
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

    fn click(&mut self, button: MouseButton, column: u16, row: u16) {
        self.app.send_mouse(MouseEventKind::Down(button), column, row);
    }

    /// Another client connects and says hello, taking the session over.
    async fn take_over(&mut self, conn: u64, touch: bool) {
        let (ctx, rx) = tokio::sync::mpsc::channel(256);
        let writer = tokio::spawn(std::future::pending::<()>());
        self.app.handle(AppEvent::Connected { conn, tx: ctx, writer });
        let msg = ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            rows: self.rows,
            cols: self.cols,
            truecolor: true,
            term: "xterm".into(),
            touch,
        };
        self.app.handle(AppEvent::Client { conn, msg });
        // The frames go to the new client now.
        self.conn_rx = rx;
        let (rows, cols) = (NonZeroU16::new(self.rows).unwrap(), NonZeroU16::new(self.cols).unwrap());
        self.screen = vt100::Parser::new(rows, cols, 0);
        self.settle("the new client's frame", |h| h.text().contains("sh")).await;
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

    fn bg_at(&self, x: u16, y: u16) -> vt100::Color {
        self.screen.screen().cell(y, x).unwrap().bgcolor()
    }

    async fn press_and_move(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.app
            .send_mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
        self.app.send_mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
        self.frame().await;
    }

    /// Opens tabs until there are `n`, waiting until the shells fit.
    async fn tabs(&mut self, n: usize) {
        for i in 1..n {
            self.app.run(Command::NewTerminal);
            self.settle("a new tab", |h| h.tab_count() == i + 1 && h.shells_fit())
                .await;
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.app.shutdown();
    }
}

const BOTTOM: &str = "tabs_at_bottom = true\n";

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_the_bar_is_three_rows_with_padded_tabs_at_the_top() {
    let mut h = Harness::new(14, 60, "", true).await;
    h.settle("a padded tab on the middle row", |h| h.row(1).contains("sh"))
        .await;
    let g = h.app.hit.groups[0].clone();
    assert_eq!((g.bar.y, g.bar.height, g.term.y, g.term.height), (0, 3, 3, 10));
    assert_eq!(
        h.row(1).trim_end(),
        format!("{:<58}+", "  sh ○  ×"),
        "two cells of padding at each end and round the x; the + in the middle of the bar's last 3"
    );
    assert_eq!(
        (h.row(0).trim(), h.row(2).trim()),
        ("", ""),
        "the rows above and below carry no text"
    );
    let t = g.tabs[0];
    assert_eq!((t.x0, t.x1), (0, 11));
    assert_eq!((t.close_x0, t.close_x1), (7, 10), "the x and the cell each side of it");
    let bg = h.bg_at(t.x0, 1);
    assert_ne!(bg, h.bg_at(40, 1), "the tab's background differs from the bar's");
    for y in 0..3 {
        for x in t.x0..t.x1 {
            assert_eq!(h.bg_at(x, y), bg, "({x}, {y}) is part of the tab's block");
        }
        assert_ne!(h.bg_at(t.x1, y), bg, "the block ends with the tab");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_the_bar_is_three_rows_with_padded_tabs_at_the_bottom() {
    let mut h = Harness::new(14, 60, BOTTOM, true).await;
    // 13 rows for the group (the status bar is the last): the bar takes 10..13.
    h.settle("a padded tab on the middle row", |h| h.row(11).contains("sh"))
        .await;
    let g = h.app.hit.groups[0].clone();
    assert_eq!((g.bar.y, g.bar.height, g.term.y, g.term.height), (10, 3, 0, 10));
    assert_eq!(h.row(11).trim_end(), format!("{:<58}+", "  sh ○  ×"));
    assert_eq!((h.row(10).trim(), h.row(12).trim()), ("", ""));
    let t = g.tabs[0];
    let bg = h.bg_at(t.x0, 11);
    for y in 10..13 {
        assert_eq!(h.bg_at(t.x0 + 5, y), bg, "row {y} is the tab's");
    }
}

/// A tap (press and release) at the cell.
fn tap(h: &mut Harness, x: u16, y: u16) {
    h.click(MouseButton::Left, x, y);
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

async fn taps_on_every_row_focus(bottom: bool) {
    let mut h = Harness::new(16, 70, if bottom { BOTTOM } else { "" }, true).await;
    h.tabs(3).await;
    h.settle("three tabs", |h| h.app.hit.groups[0].tabs.len() == 3).await;
    let g = h.app.hit.groups[0].clone();
    assert_eq!(g.bar.height, 3);
    for y in g.bar.y..g.bar.y + 3 {
        // The left padding, the title and the right padding (short of the close target).
        for dx in [0, 2, 3, 6] {
            tap(&mut h, g.tabs[0].x0 + dx, y);
            assert_eq!(h.active(), 0, "tap at +{dx} on row {y}");
            tap(&mut h, g.tabs[2].x0 + dx, y);
            assert_eq!(h.active(), 2, "tap on the third tab at +{dx} on row {y}");
        }
    }
    assert_eq!(h.tab_count(), 3, "none of them closed a tab");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_a_tap_on_any_row_of_a_tab_focuses_it_at_the_top() {
    taps_on_every_row_focus(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_a_tap_on_any_row_of_a_tab_focuses_it_at_the_bottom() {
    taps_on_every_row_focus(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_the_close_target_is_three_cells_on_all_three_rows() {
    let mut h = Harness::new(16, 100, "", true).await;
    h.tabs(5).await;
    h.settle("five tabs", |h| h.app.hit.groups[0].tabs.len() == 5).await;
    // Beside the target focuses; the target closes, at each of its cells and rows.
    let t = h.app.hit.groups[0].tabs[1];
    assert_eq!(t.close_x1 - t.close_x0, 3);
    for x in [t.close_x0 - 1, t.close_x1] {
        tap(&mut h, x, 1);
        assert_eq!((h.tab_count(), h.active()), (5, 1), "x {x} is beside the target");
    }
    for (i, (dx, y)) in [(0, 0), (1, 1), (2, 2), (0, 2)].into_iter().enumerate() {
        let t = h.app.hit.groups[0].tabs[1];
        tap(&mut h, t.close_x0 + dx, y);
        assert_eq!(h.tab_count(), 4 - i, "closed at +{dx} on row {y}");
        h.settle("the bar redrawn", |h| {
            h.app.hit.groups[0].tabs.len() == 4 - i && !h.app.needs_redraw
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_closing_a_running_command_asks_first() {
    let mut h = Harness::new(16, 70, "", true).await;
    h.tabs(2).await;
    h.app.type_text("printf '\\033]133;B\\a'; sleep 30\n");
    h.settle("a running command", |h| {
        h.app.groups.active_group().active_tab().is_some_and(|t| !t.idle)
    })
    .await;
    h.frame().await;
    let t = h.app.hit.groups[0].tabs[1];
    tap(&mut h, t.close_x1 - 1, 2);
    assert!(h.app.confirm.is_some(), "the end of the target, on the bottom pad row");
    assert_eq!(h.tab_count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_the_arrows_are_three_cells_and_scroll_an_overflowing_bar() {
    let mut h = Harness::new(14, 40, "", true).await;
    h.tabs(5).await;
    h.settle("arrows on the middle row", |h| {
        h.row(1).contains('‹') && h.row(1).contains('›') && h.app.hit.groups[0].arrows.is_some()
    })
    .await;
    assert_eq!((h.row(0).trim(), h.row(2).trim()), ("", ""));
    let arrows = h.app.hit.groups[0].arrows.clone().unwrap();
    assert_eq!(
        (arrows.back.clone(), arrows.forward.clone()),
        (0..3, 34..37),
        "the + keeps the last 3 cells"
    );
    assert_eq!(
        h.row(1).chars().nth(1),
        Some('‹'),
        "the glyph is in the middle of its target"
    );
    let before = h.app.groups.active_group().scroll;
    assert!(before >= 3, "scrolled to the last tab: {before}");
    for (n, (x, y)) in [(0, 0), (1, 1), (2, 2)].into_iter().enumerate() {
        tap(&mut h, x, y);
        assert_eq!(h.app.groups.active_group().scroll, before - 1 - n, "back at ({x}, {y})");
    }
    let scrolled = h.app.groups.active_group().scroll;
    tap(&mut h, 36, 0);
    assert_eq!(
        h.app.groups.active_group().scroll,
        scrolled + 1,
        "forward at its far cell"
    );
    h.app.send_mouse(MouseEventKind::ScrollUp, 10, 2);
    assert_eq!(
        h.app.groups.active_group().scroll,
        scrolled,
        "the wheel on the bottom pad row"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_a_tab_drags_to_another_bar_and_to_a_terminal_half() {
    let mut h = Harness::new(20, 61, "", true).await;
    h.app.run(Command::SplitRight);
    h.settle("two groups", |h| h.app.hit.groups.len() == 2 && h.shells_fit())
        .await;
    let (left, right) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    h.app.run(Command::FocusSide(crate::group::Side::Left));
    h.app.run(Command::NewTerminal);
    h.settle("two tabs on the left", |h| {
        h.app.groups.get(left).unwrap().tabs.len() == 2 && h.shells_fit()
    })
    .await;
    // Pressed on the bottom pad row of the left tab 0, dropped on the right bar's far end.
    let rbar = h.app.hit.group(right).unwrap().bar;
    let t = h.app.hit.group(left).unwrap().tabs[0];
    let from = (t.x0 + 1, 2);
    let end = (rbar.x + rbar.width - 1, rbar.y + 2);
    h.press_and_move(from, end).await;
    // The drop marker (`tab.dropMarker`'s blue) spans the bar's three rows.
    let marker = vt100::Color::Rgb(0x00, 0x7A, 0xCC);
    // Dropped past the last tab, the marker sits in the gap at that tab's end.
    let gap = h.app.hit.group(right).unwrap().tabs.last().unwrap().x1;
    for y in 0..3 {
        assert_eq!(h.bg_at(gap, rbar.y + y), marker, "the marker on bar row {y}");
    }
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), end.0, end.1);
    h.frame().await;
    assert_eq!(
        (
            h.app.groups.get(left).unwrap().tabs.len(),
            h.app.groups.get(right).unwrap().tabs.len()
        ),
        (1, 2)
    );
    h.settle("shells fit", |h| h.shells_fit()).await;

    // The right group's tab 0 onto the left terminal's right half: shaded, then a split.
    let term = h.app.hit.group(left).unwrap().term;
    let (x, y) = (term.x + term.width - 1, term.y + 3);
    let before = h.bg_at(x - 1, y);
    let from = {
        let g = h.app.hit.group(right).unwrap();
        (g.tabs[0].x0 + 1, g.bar.y + 1)
    };
    h.press_and_move(from, (x, y)).await;
    assert_ne!(h.bg_at(x - 1, y), before, "the half is shaded");
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    h.frame().await;
    h.settle("three groups, each shell sized", |h| {
        h.app.hit.groups.len() == 3 && h.shells_fit()
    })
    .await;
    assert!(h.app.hit.groups.iter().all(|g| g.bar.height == 3));
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_the_shells_are_two_rows_shorter_and_told_so() {
    let plain = Harness::new(14, 60, "", false).await;
    let mut h = Harness::new(14, 60, "", true).await;
    h.settle("the shell fits", |h| h.shells_fit()).await;
    let id = h.app.hit.groups[0].id;
    assert_eq!(h.shell_size(id).0 + 2, plain.shell_size(plain.app.hit.groups[0].id).0);
    // `shell_size` is what the program in the shell was told, not only the drawn area.
    let want = h.shell_size(id);
    assert_eq!(want, (10, 59));
    h.app.run(Command::NewTerminal);
    // Before any frame is drawn, the new shell has started at the touch size already.
    assert_eq!(h.app.groups.get(id).unwrap().tabs.len(), 2);
    assert_eq!(h.shell_size(id), want, "the size the new shell was started at");
    h.settle("a new shell at the touch size", |h| {
        h.tab_count() == 2 && h.shells_fit()
    })
    .await;
    assert_eq!(h.shell_size(id), want, "a new shell starts at the size the frame gives");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_a_plain_client_taking_over_gets_the_normal_layout() {
    let mut h = Harness::new(14, 60, "", true).await;
    h.settle("the touch bar", |h| {
        h.app.hit.groups[0].bar.height == 3 && h.shells_fit()
    })
    .await;
    let id = h.app.hit.groups[0].id;
    let small = h.shell_size(id);
    assert!(h.app.touch);
    h.take_over(2, false).await;
    h.settle("a one-row bar", |h| {
        h.app.hit.groups[0].bar.height == 1 && h.shells_fit()
    })
    .await;
    assert!(!h.app.touch);
    assert_eq!(h.shell_size(id).0, small.0 + 2, "the shells grow back");
    assert_eq!(h.row(0).trim_end(), " sh ○ ×");
    // And a touch client taking over again.
    h.take_over(3, true).await;
    h.settle("a three-row bar", |h| {
        h.app.hit.groups[0].bar.height == 3 && h.shells_fit()
    })
    .await;
    assert_eq!(h.shell_size(id), small);
    // Detaching clears it.
    h.app.handle(AppEvent::Client {
        conn: 3,
        msg: ClientMsg::Detach,
    });
    assert!(!h.app.touch);
    let mut bye = None;
    while let Ok(msg) = h.conn_rx.try_recv() {
        if let ServerMsg::Bye { reason } = msg {
            bye = Some(reason);
        }
    }
    assert!(matches!(bye, Some(crate::protocol::ByeReason::Detached)), "{bye:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_a_group_shorter_than_the_bar_gets_a_shorter_bar() {
    // Window rows 4, 3 and 2 leave the group 3, 2 and 1 rows after the status bar.
    for (rows, bar) in [(4u16, 3u16), (3, 2), (2, 1)] {
        for text in ["", BOTTOM] {
            let mut h = Harness::new(rows, 40, text, true).await;
            h.frame().await;
            let g = h.app.hit.groups[0].clone();
            assert_eq!(
                (g.bar.height, g.term.height),
                (bar, rows - 1 - bar),
                "rows {rows} {text:?}"
            );
            assert_eq!(g.bar.height + g.term.height, rows - 1);
            // Each tab is still hit across the bar's rows, and the bar's last row is drawn.
            let t = g.tabs[0];
            tap(&mut h, t.x0, g.bar.y + bar - 1);
            assert_eq!(h.active(), 0);
            let last = g.bar.y + bar - 1;
            assert_ne!(h.bg_at(t.x0, last), h.bg_at(30, last), "tab drawn on row {last}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_a_hello_without_touch_is_accepted() {
    // What an old client sends: no `touch` field.
    let json = r#"{"Hello":{"version":1,"rows":14,"cols":60,"truecolor":true,"term":"xterm"}}"#;
    let msg: ClientMsg = serde_json::from_str(json).unwrap();
    assert!(matches!(&msg, ClientMsg::Hello { touch: false, .. }), "{msg:?}");
    let mut h = Harness::new(14, 60, "", true).await;
    let (ctx, mut rx) = tokio::sync::mpsc::channel(256);
    let writer = tokio::spawn(std::future::pending::<()>());
    h.app.handle(AppEvent::Connected {
        conn: 3,
        tx: ctx,
        writer,
    });
    h.app.handle(AppEvent::Client { conn: 3, msg });
    assert!(matches!(rx.try_recv(), Ok(ServerMsg::Welcome { .. })));
    assert!(!h.app.touch, "the old client's layout is the normal one");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_the_plus_opens_a_tab_in_its_group() {
    let mut h = Harness::new(20, 61, "", true).await;
    h.app.run(Command::SplitRight);
    h.settle("two groups", |h| h.app.hit.groups.len() == 2 && h.shells_fit())
        .await;
    let (left, right) = (h.app.hit.groups[0].id, h.app.hit.groups[1].id);
    h.app.run(Command::FocusSide(crate::group::Side::Left));
    let g = h.app.hit.group(right).unwrap().clone();
    let plus = g.new_tab.clone().expect("a + on the right bar");
    assert_eq!(
        plus,
        g.bar.x + g.bar.width - 3..g.bar.x + g.bar.width,
        "the bar's last 3 cells"
    );
    h.frame().await;
    assert_eq!(h.row(1).chars().nth(usize::from(plus.start + 1)), Some('+'));
    let tabs = |h: &Harness, id| h.app.groups.get(id).unwrap().tabs.len();
    for (n, (x, y)) in [(plus.start, 0), (plus.start + 1, 1), (plus.end - 1, 2)]
        .into_iter()
        .enumerate()
    {
        tap(&mut h, x, y);
        assert_eq!(tabs(&h, right), n + 2, "a tab opened by the tap at ({x}, {y})");
        assert_eq!(h.app.groups.active, right, "the right group is focused");
        assert_eq!(h.app.groups.get(right).unwrap().active, n + 1, "the new tab is active");
        h.settle("the new shell sized", |h| h.shells_fit()).await;
    }
    assert_eq!(tabs(&h, left), 1, "the left group is untouched");
    tap(&mut h, plus.start - 1, 1);
    assert_eq!(tabs(&h, right), 4, "the cell before the + opens nothing");

    let mut plain = Harness::new(14, 60, "", false).await;
    plain.frame().await;
    assert!(plain.app.hit.groups[0].new_tab.is_none());
    assert!(!plain.row(0).contains('+'), "no + in the normal bar");
}
