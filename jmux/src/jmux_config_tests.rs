//! End-to-end tests of jmux_config-a (`settings.toml`), driving the server `App` headlessly with
//! real shells and a settings file in a temporary directory (never `XDG_CONFIG_HOME`).
//!
//! Covered by `jmux_config_a_*`: `shell` and `scrollback` from the file the server started with
//! apply to the first terminal (`..._shell_and_scrollback_apply_to_the_first_terminal`); a bad key is
//! a notice with its line and the rest applies (`..._a_bad_key_...`); "Reload Settings" re-reads the
//! file, changes the next terminal only and says so in the status bar (`..._reload_...`), reports a
//! bad key the same way, and without a settings path says there is nothing to reload.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::Event as Input;
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, Config};
use crate::command::Command;
use crate::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use crate::settings;

const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    app: App,
    events: UnboundedReceiver<AppEvent>,
    _conn_rx: Receiver<ServerMsg>,
    terminal: Terminal<StreamBackend>,
    dir: tempfile::TempDir,
}

/// A shell script that says `marker` and then becomes `/bin/sh`.
fn script(dir: &Path, name: &str, marker: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\necho {marker}\nexec /bin/sh\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

impl Harness {
    /// A started session whose settings file holds `text` (read the way `run_server` does).
    async fn new(text: &str) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, text).unwrap();
        let (settings, settings_errors) = settings::load(&path);
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
            _conn_rx: conn_rx,
            terminal: Terminal::new(StreamBackend::new(100, 24)).unwrap(),
            dir,
        }
    }

    fn settings_file(&self) -> PathBuf {
        self.dir.path().join("settings.toml")
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars().chain(['\r']) {
            let code = if ch == '\r' {
                crossterm::event::KeyCode::Enter
            } else {
                crossterm::event::KeyCode::Char(ch)
            };
            let key = crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE);
            self.app.handle(AppEvent::Client {
                conn: 1,
                msg: ClientMsg::Input(Input::Key(key)),
            });
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
    }

    async fn settle(&mut self, what: &str, done: impl Fn(&Harness) -> bool) {
        let deadline = Instant::now() + WAIT;
        self.step().await;
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.step().await;
        }
    }

    /// The active tab's screen text, scrollback excluded.
    fn screen_text(&self) -> String {
        self.app
            .groups
            .tabs()
            .last()
            .map(|t| t.run.screen().screen().contents())
            .unwrap_or_default()
    }

    /// `(base, scrollback lines kept)` of the tab opened last.
    fn last_tab(&self) -> (String, usize) {
        let t = self.app.groups.tabs().last().unwrap();
        let kept = t.run.screen().screen().scrollback_len();
        (t.base.clone(), kept)
    }

    /// Reloads and waits for the notice it ends with.
    async fn reload(&mut self) -> String {
        self.app.notice = None;
        self.app.execute(Command::ReloadSettings);
        self.settle("the reload's notice", |h| h.app.notice.is_some()).await;
        self.app.notice.clone().unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_a_shell_and_scrollback_apply_to_the_first_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let shell = script(tmp.path(), "mysh", "marker-first");
    let mut h = Harness::new(&format!("shell = \"{}\"\nscrollback = 20\n", shell.display())).await;
    h.settle("the script's marker", |h| h.screen_text().contains("marker-first"))
        .await;
    assert_eq!(h.last_tab().0, "mysh");
    h.type_text("seq 1 200; echo done-$((2+3))");
    h.settle("the output", |h| h.screen_text().contains("done-5")).await;
    let (_, kept) = h.last_tab();
    // 200+ lines went by: the limit is what was kept, not merely an upper bound.
    assert_eq!(
        kept, 20,
        "scrollback should hold exactly the 20 lines the setting allows"
    );
    assert!(h.app.notice.is_none(), "{:?}", h.app.notice);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_a_a_bad_key_is_a_notice_with_its_line_and_the_rest_applies() {
    let h = Harness::new("scrollback = 7\nbogus = 1\ncommand_history = \"many\"\n").await;
    let notice = h.app.notice.clone().unwrap();
    assert!(
        notice.contains("line 2") && notice.contains("bogus") && notice.contains("1 more"),
        "{notice}"
    );
    assert_eq!(h.app.config.settings.scrollback, 7);
    assert_eq!(h.app.config.settings.command_history, 500);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_a_reload_changes_the_next_terminal_only() {
    let tmp = tempfile::tempdir().unwrap();
    let shell = script(tmp.path(), "second", "marker-second");
    let mut h = Harness::new("scrollback = 5000\n").await;
    h.settle("the first shell", |h| h.app.groups.tabs().count() == 1).await;
    assert_eq!(h.last_tab().0, "sh");
    std::fs::write(
        h.settings_file(),
        format!(
            "shell = \"{}\"\nscrollback = 20\nside_bar_width = 40\n",
            shell.display()
        ),
    )
    .unwrap();
    assert_eq!(h.reload().await, "Settings reloaded");
    assert_eq!(h.app.side.width, 40);
    assert_eq!(h.last_tab().0, "sh", "the running terminal keeps its shell");
    h.app.execute(Command::NewTerminal);
    h.settle("the second shell's marker", |h| {
        h.screen_text().contains("marker-second")
    })
    .await;
    assert_eq!(h.last_tab().0, "second");
    h.type_text("seq 1 200; echo done-$((2+3))");
    h.settle("the output", |h| h.screen_text().contains("done-5")).await;
    let kept = h.last_tab().1;
    assert_eq!(kept, 20, "the new terminal uses the reloaded scrollback");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_a_reload_reports_a_bad_key_and_applies_the_rest() {
    let mut h = Harness::new("").await;
    std::fs::write(h.settings_file(), "scrollback = 9\n\nnope = true\n").unwrap();
    let notice = h.reload().await;
    assert!(notice.contains("line 3") && notice.contains("nope"), "{notice}");
    assert_eq!(h.app.config.settings.scrollback, 9);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_a_reload_without_a_settings_path_says_so() {
    let mut h = Harness::new("").await;
    h.app.config.settings_path = None;
    h.app.execute(Command::ReloadSettings);
    assert!(h.app.notice.as_deref().unwrap().contains("No settings file"));
}
