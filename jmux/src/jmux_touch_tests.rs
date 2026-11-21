//! The headless harness for the touch layout's tests (`jmux attach --touch`): the server `App`
//! driven with the fake shell and a `Hello` carrying `touch`, used by `jmux_touch_*` tests. Its own
//! tests cover the layout's handshake: a plain client taking the session over gets the tab bars
//! back and a touch client the full-height shells again (`..._a_plain_client_taking_over_...`), a
//! `Hello` without `touch` is accepted (`..._a_hello_without_touch_...`). The menu that replaces
//! the bars is `jmux_touch_menu_tests`.

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

pub(crate) struct Harness {
    pub(crate) app: App,
    pub(crate) events: UnboundedReceiver<AppEvent>,
    pub(crate) conn_rx: Receiver<ServerMsg>,
    pub(crate) terminal: Terminal<StreamBackend>,
    pub(crate) screen: vt100::Parser,
    pub(crate) rows: u16,
    pub(crate) cols: u16,
    pub(crate) _dir: tempfile::TempDir,
}

impl Harness {
    /// A started session (one tab) at `rows` x `cols` whose settings file holds `text`.
    pub(crate) async fn new(rows: u16, cols: u16, text: &str, touch: bool) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(crate::settings::FILE_NAME);
        std::fs::write(&path, text).unwrap();
        let (settings, settings_errors) = crate::settings::load(&path);
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let config = Config {
            name: "work".into(),
            socket: PathBuf::new(),
            program: jterm::testing::fake_shell(),
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

    pub(crate) async fn step(&mut self) {
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

    pub(crate) async fn settle(&mut self, what: &str, done: impl Fn(&Harness) -> bool) {
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

    pub(crate) async fn frame(&mut self) {
        self.settle("a frame", |h| !h.app.needs_redraw).await;
    }

    pub(crate) fn text(&self) -> String {
        self.screen.screen().contents()
    }

    pub(crate) fn row(&self, row: u16) -> String {
        self.screen
            .screen()
            .rows(0, self.cols)
            .nth(usize::from(row))
            .unwrap_or_default()
    }

    pub(crate) fn click(&mut self, button: MouseButton, column: u16, row: u16) {
        self.app.send_mouse(MouseEventKind::Down(button), column, row);
    }

    /// Another client connects and says hello, taking the session over.
    pub(crate) async fn take_over(&mut self, conn: u64, touch: bool) {
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

    pub(crate) fn shell_size(&self, group: u64) -> (u16, u16) {
        let tab = self.app.groups.get(group).unwrap().active_tab().unwrap();
        let (rows, cols) = tab.run.screen().screen().size();
        (rows.get(), cols.get())
    }

    pub(crate) fn shells_fit(&self) -> bool {
        let fit = |g: &crate::hit::GroupHit| self.shell_size(g.id) == (g.text.height, g.text.width);
        self.app.hit.groups.iter().all(fit)
    }

    pub(crate) fn tab_count(&self) -> usize {
        self.app.groups.tab_count()
    }

    pub(crate) fn active(&self) -> usize {
        self.app.groups.active_group().active
    }

    pub(crate) fn bg_at(&self, x: u16, y: u16) -> vt100::Color {
        self.screen.screen().cell(y, x).unwrap().bgcolor()
    }

    /// Opens tabs until there are `n`, waiting until the shells fit.
    pub(crate) async fn tabs(&mut self, n: usize) {
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

pub(crate) const BOTTOM: &str = "tabs_at_bottom = true\n";

/// A tap (press and release) at the cell.
pub(crate) fn tap(h: &mut Harness, x: u16, y: u16) {
    h.click(MouseButton::Left, x, y);
    h.app.send_mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_touch_a_plain_client_taking_over_gets_the_normal_layout() {
    let mut h = Harness::new(14, 60, "", true).await;
    h.settle("the full-height shell", |h| {
        h.app.hit.groups[0].bar.height == 0 && h.shells_fit()
    })
    .await;
    let id = h.app.hit.groups[0].id;
    let tall = h.shell_size(id);
    assert!(h.app.touch);
    h.take_over(2, false).await;
    h.settle("a one-row bar", |h| {
        h.app.hit.groups[0].bar.height == 1 && h.shells_fit()
    })
    .await;
    assert!(!h.app.touch);
    assert_eq!(
        h.shell_size(id).0,
        tall.0,
        "the tab bar takes the row the second status row gave back"
    );
    assert_eq!(h.row(0).trim_end(), " fakesh ○ ×");
    // And a touch client taking over again.
    h.take_over(3, true).await;
    h.settle("no bar", |h| h.app.hit.groups[0].bar.height == 0 && h.shells_fit())
        .await;
    assert_eq!(h.shell_size(id), tall);
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
