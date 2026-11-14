//! End-to-end tests of jmux_config-a (`settings.toml`), driving the server `App` headlessly with
//! real shells and a settings file in a temporary directory (never `XDG_CONFIG_HOME`).
//!
//! Covered by `jmux_config_a_*`: `shell` and `scrollback` from the file the server started with
//! apply to the first terminal (`..._shell_and_scrollback_apply_to_the_first_terminal`); a bad key is
//! a notice with its line and the rest applies (`..._a_bad_key_...`); "Reload Settings" re-reads the
//! file, changes the next terminal only and says so in the status bar (`..._reload_...`), reports a
//! bad key the same way, and without a settings path says there is nothing to reload.
//!
//! Covered by `jmux_config_b_*` (`keybindings.toml` and the status bar): a user binding for split
//! right works and the default key stops (`..._a_user_binding_...`); a bad binding is a notice and
//! the rest applies; Reload Settings re-reads the file, and a file that does not parse keeps the old
//! keymap; the bar shows the terminal and running counts and they update, and a rebound detach key
//! shows.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::Event as Input;
use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use crate::app::{App, AppEvent, Config};
use crate::command::Command;
use crate::keymap;
use crate::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use crate::settings;

const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    app: App,
    events: UnboundedReceiver<AppEvent>,
    conn_rx: Receiver<ServerMsg>,
    terminal: Terminal<StreamBackend>,
    /// What the client would show (the frames' bytes).
    screen: vt100::Parser,
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
        Harness::with_keys(text, None).await
    }

    /// Like `new`, with a `keybindings.toml` holding `keys` (read the way `run_server` does).
    async fn with_keys(text: &str, keys: Option<&str>) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let keys_path = dir.path().join(keymap::FILE_NAME);
        if let Some(keys) = keys {
            std::fs::write(&keys_path, keys).unwrap();
        }
        let (keymap, keymap_errors) = keymap::load(&keys_path);
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, text).unwrap();
        let (settings, mut settings_errors) = settings::load(&path);
        settings_errors.extend(keymap_errors);
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
            keybindings_path: Some(keys_path),
            keymap: Some(keymap),
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
            screen: vt100::Parser::new(
                std::num::NonZeroU16::new(24).unwrap(),
                std::num::NonZeroU16::new(100).unwrap(),
                0,
            ),
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
        while let Ok(msg) = self.conn_rx.try_recv() {
            if let ServerMsg::Output(bytes) = msg {
                self.screen.process(&bytes);
            }
        }
    }

    fn keys_file(&self) -> PathBuf {
        self.dir.path().join(keymap::FILE_NAME)
    }

    /// The status bar's text.
    fn bar(&self) -> String {
        self.screen.screen().rows(0, 100).nth(23).unwrap_or_default()
    }

    fn press(&mut self, code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) {
        let key = crossterm::event::KeyEvent::new(code, mods);
        self.app.handle(AppEvent::Client {
            conn: 1,
            msg: ClientMsg::Input(Input::Key(key)),
        });
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
    h.app.config.keybindings_path = None;
    h.app.execute(Command::ReloadSettings);
    assert!(h.app.notice.as_deref().unwrap().contains("No settings file"));
}

const SPLIT_RIGHT_ON_CTRL_ALT_K: &str = "[[binding]]\nkey = \"ctrl+alt+k\"\ncommand = \"terminal.splitRight\"\n";

fn ctrl_alt() -> crossterm::event::KeyModifiers {
    crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_b_a_user_binding_for_split_right_works_and_the_default_key_stops() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut h = Harness::with_keys("", Some(SPLIT_RIGHT_ON_CTRL_ALT_K)).await;
    assert!(h.app.notice.is_none(), "{:?}", h.app.notice);
    h.settle("the first shell", |h| h.app.groups.tabs().count() == 1).await;
    assert_eq!(h.app.groups.order().len(), 1);
    // The default key (Ctrl+\\) no longer splits.
    h.press(KeyCode::Char('\\'), KeyModifiers::CONTROL);
    assert_eq!(h.app.groups.order().len(), 1);
    h.press(KeyCode::Char('k'), ctrl_alt());
    h.settle("a second group", |h| h.app.groups.order().len() == 2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_b_a_bad_binding_is_reported_and_the_others_apply() {
    use crossterm::event::KeyCode;
    let keys = format!("{SPLIT_RIGHT_ON_CTRL_ALT_K}\n[[binding]]\nkey = \"ctrl+alt+j\"\ncommand = \"nope.nothing\"\n");
    let mut h = Harness::with_keys("", Some(&keys)).await;
    let notice = h.app.notice.clone().unwrap();
    assert!(
        notice.contains("keybindings.toml") && notice.contains("nope.nothing"),
        "{notice}"
    );
    h.press(KeyCode::Char('k'), ctrl_alt());
    h.settle("a second group", |h| h.app.groups.order().len() == 2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_b_reload_rereads_the_file_and_a_broken_file_keeps_the_old_keymap() {
    use crossterm::event::KeyCode;
    let mut h = Harness::new("").await;
    h.press(KeyCode::Char('k'), ctrl_alt());
    assert_eq!(h.app.groups.order().len(), 1, "unbound before the reload");
    std::fs::write(h.keys_file(), SPLIT_RIGHT_ON_CTRL_ALT_K).unwrap();
    assert_eq!(h.reload().await, "Settings reloaded");
    h.press(KeyCode::Char('k'), ctrl_alt());
    h.settle("a second group", |h| h.app.groups.order().len() == 2).await;
    // A file that does not parse: reported, the keymap from before stays.
    std::fs::write(h.keys_file(), "[[binding\n").unwrap();
    let notice = h.reload().await;
    assert!(notice.contains("keybindings.toml"), "{notice}");
    h.press(KeyCode::Char('k'), ctrl_alt());
    h.settle("a third group", |h| h.app.groups.order().len() == 3).await;
    // One bad binding in a parsable file: reported on a reload too, the good one still applies.
    let bad = format!("{SPLIT_RIGHT_ON_CTRL_ALT_K}\n[[binding]]\nkey = \"ctrl+alt+j\"\ncommand = \"nope.nothing\"\n");
    std::fs::write(h.keys_file(), bad).unwrap();
    let notice = h.reload().await;
    assert!(notice.contains("nope.nothing"), "{notice}");
    h.press(KeyCode::Char('k'), ctrl_alt());
    h.settle("a fourth group", |h| h.app.groups.order().len() == 4).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_b_reload_works_with_only_a_keybindings_path() {
    let mut h = Harness::new("").await;
    h.app.config.settings_path = None;
    assert_eq!(h.reload().await, "Settings reloaded");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_b_the_status_bar_shows_the_counts_and_they_update() {
    let mut h = Harness::new("").await;
    h.settle("1 terminal", |h| {
        h.bar().contains("work") && h.bar().contains("1 terminal ")
    })
    .await;
    assert!(h.bar().contains("detach"), "{}", h.bar());
    h.app.execute(Command::NewTerminal);
    h.settle("2 terminals", |h| h.bar().contains("2 terminals")).await;
    // A running command (the codes the shell integration prints) shows, then goes.
    h.type_text("printf '\\033]133;B\\007'; sleep 2; printf '\\033]133;D;0\\007'");
    h.settle("1 running", |h| h.bar().contains("1 running")).await;
    h.settle("no longer running", |h| !h.bar().contains("running")).await;
    assert!(h.bar().contains("2 terminals"), "{}", h.bar());
    h.app.execute(Command::CloseTerminal);
    h.settle("1 terminal again", |h| h.bar().contains("1 terminal ")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_config_b_a_rebound_detach_key_shows_in_the_bar() {
    let keys = "[[binding]]\nkey = \"ctrl+alt+q\"\ncommand = \"session.detach\"\n";
    let mut h = Harness::with_keys("", Some(keys)).await;
    h.settle("the hint", |h| h.bar().contains("detach")).await;
    assert!(h.bar().contains("Ctrl+Alt+Q detach"), "{}", h.bar());
}
