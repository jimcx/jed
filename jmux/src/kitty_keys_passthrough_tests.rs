//! kitty_keys_passthrough-b "Done when" (jmux's half): a program in a jmux pane that pushes the
//! kitty keyboard flags (`fakesh`'s `keyecho`) has the query answered with them, gets Ctrl+Shift+A,
//! Esc and Alt+X as `CSI .. u`, gets a key release only with flag 2 (`:3`), and after it pops the
//! flags gets exactly the bytes a terminal without the protocol sends.
//! (Hand check, not tested: jed started in a pane finds the protocol without delay.)

use std::time::{Duration, Instant};

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, Config};
use crate::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};

const WAIT: Duration = Duration::from_secs(10);
const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);

struct Harness {
    app: App,
    events: UnboundedReceiver<AppEvent>,
    conn_rx: Receiver<ServerMsg>,
    terminal: Terminal<StreamBackend>,
}

impl Harness {
    async fn new() -> Harness {
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let config = Config {
            name: "work".into(),
            socket: std::path::PathBuf::new(),
            program: jterm::testing::fake_shell(),
            cwd: std::env::temp_dir(),
            open_url: |_| Ok(()),
            dirs: None,
            settings: crate::settings::Settings::default(),
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
        }
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
        while self.conn_rx.try_recv().is_ok() {}
    }

    /// The focused pane's screen text.
    fn text(&self) -> String {
        let tab = self.app.groups.active_group().active_tab().unwrap();
        tab.run.screen().screen().contents()
    }

    fn flags(&self) -> u8 {
        self.app.groups.active_group().active_tab().unwrap().run.kitty_flags()
    }

    /// The `hex: ..` lines `keyecho` has printed.
    fn hex_lines(&self) -> Vec<String> {
        self.text()
            .lines()
            .filter_map(|l| l.strip_prefix("hex: "))
            .map(|l| l.trim_end().to_string())
            .collect()
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

    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) {
        self.app.send_input(Input::Key(KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }));
    }

    /// Sends a key, then waits for `lines` printed lines (one key per read: a later key never
    /// joins an earlier one's bytes).
    async fn send(&mut self, lines: usize, code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) {
        self.key(code, modifiers, kind);
        self.settle("the key's bytes", |h| h.hex_lines().len() >= lines).await;
    }

    /// Starts `keyecho flags` in the pane and waits for the query's answer.
    async fn keyecho(&mut self, flags: &str) {
        self.app.type_text(&format!("keyecho {flags}\n"));
        let reply = format!("flags={flags}");
        self.settle("the query's answer", |h| h.text().contains(&reply)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_pane_gets_csi_u_keys_without_releases() {
    let mut h = Harness::new().await;
    h.keyecho("1").await;
    assert_eq!(h.flags(), 1);
    let (press, release) = (KeyEventKind::Press, KeyEventKind::Release);
    h.send(1, KeyCode::Char('a'), CTRL_SHIFT, press).await;
    // Flag 1 has no releases: this one reaches nothing, so Esc is the next line.
    h.key(KeyCode::Char('a'), CTRL_SHIFT, release);
    h.send(2, KeyCode::Esc, KeyModifiers::NONE, press).await;
    h.send(3, KeyCode::Char('x'), KeyModifiers::ALT, press).await;
    assert_eq!(
        h.hex_lines(),
        ["1b 5b 39 37 3b 36 75", "1b 5b 32 37 75", "1b 5b 31 32 30 3b 33 75"],
        "{}",
        h.text()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_pane_release_with_flag_2() {
    let mut h = Harness::new().await;
    h.keyecho("3").await;
    let (press, release) = (KeyEventKind::Press, KeyEventKind::Release);
    h.send(1, KeyCode::Char('a'), CTRL_SHIFT, press).await;
    h.send(2, KeyCode::Char('a'), CTRL_SHIFT, release).await;
    assert_eq!(
        h.hex_lines(),
        ["1b 5b 39 37 3b 36 75", "1b 5b 39 37 3b 36 3a 33 75"],
        "{}",
        h.text()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_pane_back_to_xterm_after_the_pop() {
    let mut h = Harness::new().await;
    h.keyecho("3").await;
    h.key(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Press);
    h.settle("the pop", |h| h.flags() == 0 && h.text().contains("bye"))
        .await;
    // Flags 0 pushed again: a program that asked for nothing gets xterm's bytes, releases none.
    h.keyecho("0").await;
    h.send(1, KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press).await;
    h.key(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Release);
    h.send(2, KeyCode::Char('x'), KeyModifiers::ALT, KeyEventKind::Press)
        .await;
    assert_eq!(h.hex_lines(), ["1b", "1b 78"], "{}", h.text());
}

/// A release of a key bound in the pane's context (its press ran the command) is not sent to a
/// program that asked for releases.
#[tokio::test(flavor = "multi_thread")]
async fn kitty_keys_passthrough_pane_bound_key_release_stays_out() {
    let mut h = Harness::new().await;
    h.keyecho("3").await;
    let context = h.app.context();
    let combo = h
        .app
        .keymap
        .bindings_with_source()
        .map(|(combo, ..)| combo)
        .find(|c| {
            h.app
                .keymap
                .lookup(
                    &KeyEvent {
                        code: c.code,
                        modifiers: c.mods,
                        kind: KeyEventKind::Press,
                        state: KeyEventState::NONE,
                    },
                    context,
                )
                .is_some()
        })
        .expect("a bound key");
    h.key(combo.code, combo.mods, KeyEventKind::Release);
    h.send(1, KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press).await;
    assert_eq!(h.hex_lines(), ["1b 5b 32 37 75"], "{}", h.text());
}
