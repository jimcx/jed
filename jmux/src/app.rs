//! The server's `App`: all state of one session (jed's shape: it owns everything, handlers
//! mutate it, `needs_redraw` says a frame is due, nothing blocks). So far one shell filling the
//! screen above a status bar; at most one client is attached and the shell is sized by it.
//!
//! Events arrive as [`AppEvent`]s from the connection tasks (`server`) and from the shell's run
//! (through a `Sink`). While no client is attached nothing is drawn; the run keeps the screen and
//! scrollback, and the next `Hello` redraws everything for its size.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::settings::Settings;
use crossterm::event::{Event as Input, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use jterm::run::{EnvNames, Event, RunId, Sink, StartOptions, Status};
use jterm::shell::{self, ShellTab};
use jterm::{keys, mouse, select};
use jtui::borders::Borders;
use jtui::clicks::ClickTracker;
use jtui::clipboard::osc52_sequence;
use jtui::scrollbar::Scrollable;
use jtui::split::{Divider, GroupId, SplitDir};
use jtui::stream_backend::StreamBackend;
use jtui::tabs::TabMetrics;
use jtui::terminal::{Pointer, set_pointer};
use jtui::theme::Theme;
use ratatui::Terminal;
use ratatui::layout::Rect;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Sender, UnboundedSender};
use tokio::task::JoinHandle;
use vt100::MouseProtocolMode;

use crate::command::{Command, Context, SideCommand};
use crate::group::Groups;
use crate::history::History;
use crate::hit::{Hit, HitMap};
use crate::keymap::{self, JmuxKeymap};
use crate::protocol::{ByeReason, ClientMsg, PROTOCOL_VERSION, ServerMsg};
use crate::side::SideBar;
use crate::tab_drag::TabDrag;
use crate::{integration, theme, ui};
use jterm::search::TermSearch;

/// Messages a connection may have queued for its writer. A client that falls this far behind is
/// dropped (its terminal can catch up on the next attach) rather than letting memory grow.
pub const OUT_QUEUE: usize = 256;

/// The shell's size until a client says its own.
const INITIAL_SIZE: (u16, u16) = (24, 80);

/// Rows a mouse wheel notch scrolls the scrollback by.
const WHEEL_LINES: isize = 3;

/// A step through a shell's scrollback.
enum Scroll {
    Lines(isize),
    Page(isize),
    /// The oldest output (true) or the live screen.
    Edge(bool),
}

/// The variables a shell gets: the socket path and its terminal's id (jterm sets the second).
const ENV: EnvNames = EnvNames {
    ipc: "JMUX",
    terminal: "JMUX_TERMINAL",
};

/// What a server is started with.
pub struct Config {
    pub name: String,
    /// The session's socket (the shell's `$JMUX`).
    pub socket: PathBuf,
    /// The shell to run: `$SHELL`, else `/bin/sh`.
    pub program: PathBuf,
    /// Its working directory.
    pub cwd: PathBuf,
    /// Opens a clicked URL (blocking: it runs in `spawn_blocking`); `jtui::url::open`, a recorder
    /// in the tests.
    pub open_url: fn(&str) -> Result<(), String>,
    /// Where the shell integration lives (`None`: no check, no install).
    pub dirs: Option<integration::Dirs>,
    /// `settings.toml` as the server last read it (`apply_settings` replaces it).
    pub settings: Settings,
    /// Where it is read from on `Command::ReloadSettings` (`None`: no reload).
    pub settings_path: Option<PathBuf>,
    /// What was wrong in the first read: the first notice.
    pub settings_errors: Vec<String>,
    /// `keybindings.toml`, re-read on `Command::ReloadSettings` (`None`: no reload).
    pub keybindings_path: Option<PathBuf>,
    /// The keymap the first read built (`None`: the defaults); `App::new` takes it. Its problems are in
    /// `settings_errors`.
    pub keymap: Option<JmuxKeymap>,
}

/// The notice for a settings file's problems: the first, and how many more.
fn settings_notice(errors: &[String]) -> Option<String> {
    let (first, rest) = errors.split_first()?;
    Some(match rest.len() {
        0 => first.clone(),
        n => format!("{first} (and {n} more)"),
    })
}

/// What the event loop hears about.
pub enum AppEvent {
    /// A client connected: where its messages go, and the task that writes them.
    Connected {
        conn: u64,
        tx: Sender<ServerMsg>,
        writer: JoinHandle<()>,
    },
    Client {
        conn: u64,
        msg: ClientMsg,
    },
    /// The client's connection ended (EOF or a read error).
    Closed {
        conn: u64,
    },
    Run(Event),
    /// A blocking shell-integration job finished (`integration_event`).
    Integration(IntegrationEvent),
    /// `Command::ReloadSettings`'s read finished: the new settings and keymap (`None`: no file to
    /// read, or an unreadable keybindings file: keep the old) and what was wrong (`apply_settings`).
    Settings(Option<Settings>, Option<JmuxKeymap>, Vec<String>),
    /// Once a second (`App::start`'s timer): the side bar's durations and ages move on.
    Tick,
}

/// What the off-thread integration jobs report.
pub enum IntegrationEvent {
    /// The first shell's check: its base name and the state on disk.
    Checked {
        shell: String,
        status: Option<jterm::shell_integration::Status>,
    },
    /// An install finished.
    Installed(Result<integration::Installed, String>),
}

/// The open question "close this terminal? a command is running".
pub struct Confirm {
    /// The tab's run (the tab may move or go before the answer).
    pub run: RunId,
    /// What runs: the tab's title without its state glyph.
    pub name: String,
}

struct Conn {
    tx: Sender<ServerMsg>,
    writer: JoinHandle<()>,
    /// Its `Hello` was accepted.
    hello: bool,
}

mod commands;
mod search;
mod side;

pub struct App {
    pub name: String,
    pub(crate) config: Config,
    events: UnboundedSender<AppEvent>,
    pub groups: Groups,
    pub keymap: JmuxKeymap,
    /// What the last frame drew where (`ui::render`), for the mouse.
    pub hit: HitMap,
    pub confirm: Option<Confirm>,
    /// A one-line message in place of the status bar's title, until the next key.
    pub notice: Option<String>,
    /// The commands of every terminal, running and finished (`history`).
    pub history: History,
    /// The first shell's integration check has been started.
    integration_checked: bool,
    /// The check found the first shell's integration not installed (the side bar's empty text says so).
    pub integration_missing: bool,
    /// The commands side bar.
    pub side: SideBar,
    /// The divider or tab being dragged: it owns the mouse until the release.
    pub(crate) drag: Option<Drag>,
    /// Where the pointer last was, and the shape the client was last told to show.
    mouse_at: Option<(u16, u16)>,
    pointer: Pointer,
    /// Counts clicks in a terminal (a double click selects a word, a triple one the line).
    clicks: ClickTracker,
    /// The search bar, over one run (shown while that tab is active).
    pub search: Option<TermSearch>,
    /// Escape output for the client (OSC 52 from a copy), sent with the next frame.
    out: Vec<u8>,
    /// The dividers' lines (`ui::render`).
    pub borders: Borders,
    /// The id of the next run (they stay unique for the session: `$JMUX_TERMINAL`).
    next_run: u64,
    pub theme: Theme,
    conns: HashMap<u64, Conn>,
    /// Writers of connections told `Bye`: `shutdown` waits for them so the message is delivered.
    closing: Vec<JoinHandle<()>>,
    attached: Option<u64>,
    /// The attached client asked for the touch layout (`--touch`); false while none is attached.
    pub touch: bool,
    /// The attached client's screen: rows, columns.
    size: (u16, u16),
    /// The next draw starts from a cleared screen at `size` (an attach or a resize).
    resync: bool,
    /// Unix seconds this session started (`jmux ls`).
    created: u64,
    pub needs_redraw: bool,
    pub should_quit: bool,
}

/// What a held mouse button is dragging.
#[derive(Debug, Clone)]
pub enum Drag {
    Divider(Divider),
    Tab(TabDrag),
    /// Selecting text in a terminal; `anchor` is the pressed range (a word or line for a double /
    /// triple click), `text` the shell's cells on screen.
    Terminal {
        run: RunId,
        anchor: (select::Cell, select::Cell),
        word: bool,
        text: Rect,
    },
    /// A scroll bar's thumb, held by its half cell `grab`.
    ScrollBar {
        group: GroupId,
        grab: u16,
    },
    /// The side bar's border, moving its width.
    SideBorder,
    /// The side bar's scroll bar thumb, held by its half cell `grab`.
    SideScroll {
        grab: u16,
    },
}

impl App {
    /// The tab drag in flight, if that is what is being dragged.
    pub(crate) fn tab_drag_mut(&mut self) -> Option<&mut TabDrag> {
        match &mut self.drag {
            Some(Drag::Tab(t)) => Some(t),
            _ => None,
        }
    }

    /// Ends the drag if it is a tab's, handing it over.
    pub(crate) fn take_tab_drag(&mut self) -> Option<TabDrag> {
        match self.drag.take() {
            Some(Drag::Tab(t)) => Some(t),
            other => {
                self.drag = other;
                None
            }
        }
    }

    /// The shell a new terminal runs: the `shell` setting, else the config's (`$SHELL`, `/bin/sh`).
    pub(crate) fn program(&self) -> PathBuf {
        self.config
            .settings
            .shell
            .clone()
            .unwrap_or_else(|| self.config.program.clone())
    }

    /// Spawns nothing (`start` does).
    pub fn new(mut config: Config, events: UnboundedSender<AppEvent>) -> App {
        let notice = settings_notice(&config.settings_errors);
        let mut side = SideBar::new();
        side.width = config.settings.side_bar_width;
        let keymap = config.keymap.take().unwrap_or_else(keymap::keymap);
        App {
            name: config.name.clone(),
            history: History::new(config.settings.command_history),
            config,
            events,
            groups: Groups::new(),
            keymap,
            hit: HitMap::default(),
            confirm: None,
            notice,
            integration_checked: false,
            integration_missing: false,
            side,
            drag: None,
            mouse_at: None,
            pointer: Pointer::Default,
            clicks: ClickTracker::default(),
            search: None,
            out: Vec::new(),
            borders: Borders::default(),
            next_run: 1,
            theme: theme::dark_plus(true),
            conns: HashMap::new(),
            closing: Vec::new(),
            attached: None,
            touch: false,
            size: INITIAL_SIZE,
            resync: false,
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            needs_redraw: false,
            should_quit: false,
        }
    }

    /// The session's socket file (it changes with `rename-session`).
    pub fn socket(&self) -> &std::path::Path {
        &self.config.socket
    }

    /// Starts the first shell at 24×80 (no client has said its size yet); an attach resizes it.
    /// Needs a tokio runtime.
    pub fn start(&mut self) {
        self.new_terminal(self.groups.active);
        // The side bar's durations and ages: a frame a second while it shows (`tick`), not per frame.
        let events = self.events.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(std::time::Duration::from_secs(1));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            timer.tick().await;
            loop {
                timer.tick().await;
                if events.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
        });
        tracing::info!(session = %self.name, program = %self.config.program.display(), "session started");
    }

    /// The key that runs `command`, as a label (`Ctrl+Shift+D`).
    pub(crate) fn key_label(&self, command: Command) -> String {
        self.keymap.binding_for(command).map_or_else(String::new, |c| c.label())
    }

    /// Shown at the right of the status bar.
    pub fn detach_hint(&self) -> String {
        format!("{} detach", self.key_label(Command::Detach))
    }

    /// The tab bar's layout for the attached client: touch if it attached with `--touch`.
    pub fn tab_metrics(&self) -> TabMetrics {
        if self.touch {
            TabMetrics::TOUCH
        } else {
            TabMetrics::NORMAL
        }
    }

    /// The size a new shell in `group` starts at (the next frame resizes it to its area).
    fn term_size(&self, group: GroupId) -> (u16, u16) {
        let (rows, cols) = if self.attached.is_some() {
            self.size
        } else {
            INITIAL_SIZE
        };
        let body = ui::body(rows, cols, &self.side);
        let (cells, _) = self.groups.layout(body);
        let area = cells.iter().find(|(id, _)| *id == group).map_or(body, |c| c.1);
        let (text, _) = ui::shell_area(ui::term_area(
            area,
            self.config.settings.tabs_at_bottom,
            &self.tab_metrics(),
        ));
        (text.height, text.width)
    }

    /// Opens a shell tab after the active one in `group` and makes it the active tab: the `number`th
    /// of its program, the lowest not taken (`zsh`, `zsh (2)`).
    fn new_terminal(&mut self, group: GroupId) {
        let sink: Sink = {
            let events = self.events.clone();
            Arc::new(move |ev| {
                let _ = events.send(AppEvent::Run(ev));
            })
        };
        let program = self.program();
        let spec = shell::spec(&program, &self.config.cwd, Some((ENV.ipc, &self.config.socket)));
        let base = shell::base(&program);
        let number = (1..)
            .find(|n| !self.groups.tabs().any(|t| t.base == base && t.number == *n))
            .unwrap_or(1);
        let id = RunId(self.next_run);
        self.next_run += 1;
        let (rows, cols) = self.term_size(group);
        let options = StartOptions {
            env: ENV,
            command_ring: 500,
        };
        let tab = ShellTab::new(
            id,
            spec,
            rows,
            cols,
            sink,
            self.config.settings.scrollback(),
            options,
            number,
        );
        if let Some(g) = self.groups.get_mut(group) {
            g.open(tab);
        }
        self.check_integration();
        self.needs_redraw = true;
    }

    /// Splits the active group in `dir`; the new group holds one new shell and has the focus.
    fn split(&mut self, dir: SplitDir) {
        let group = self.groups.split(dir);
        self.new_terminal(group);
    }

    /// The last layout's group areas, for the directional focus.
    fn cells(&self) -> Vec<(GroupId, Rect)> {
        let (rows, cols) = if self.attached.is_some() {
            self.size
        } else {
            INITIAL_SIZE
        };
        self.groups.layout(ui::body(rows, cols, &self.side)).0
    }

    /// Runs a command. Commands never look at keys.
    pub fn execute(&mut self, command: Command) {
        match command {
            Command::NewTerminal => self.new_terminal(self.groups.active),
            Command::SplitRight => self.split(SplitDir::Right),
            Command::SplitDown => self.split(SplitDir::Down),
            Command::FocusSide(side) => {
                let cells = self.cells();
                self.groups.focus_side(&cells, side);
            }
            Command::FocusNextGroup => self.groups.focus_step(1),
            Command::FocusPreviousGroup => self.groups.focus_step(-1),
            Command::FocusGroup(n) => self.groups.focus_nth(usize::from(n)),
            Command::CloseTerminal => {
                let group = self.groups.active;
                let index = self.groups.active_group().active;
                self.request_close(group, index);
            }
            Command::NextTab => self.groups.active_group_mut().cycle(1),
            Command::PrevTab => self.groups.active_group_mut().cycle(-1),
            Command::GoToTab(n) => self.groups.active_group_mut().go_to(usize::from(n)),
            Command::Copy => {
                if let Some(text) = self
                    .groups
                    .active_group()
                    .active_tab()
                    .and_then(|s| s.run.copy_selection())
                    && !text.is_empty()
                {
                    self.out.extend_from_slice(osc52_sequence(&text).as_bytes());
                }
            }
            Command::ScrollUp => self.scroll_active(Scroll::Lines(-1)),
            Command::ScrollDown => self.scroll_active(Scroll::Lines(1)),
            Command::PageUp => self.scroll_active(Scroll::Page(-1)),
            Command::PageDown => self.scroll_active(Scroll::Page(1)),
            Command::ScrollTop => self.scroll_active(Scroll::Edge(true)),
            Command::ScrollBottom => self.scroll_active(Scroll::Edge(false)),
            Command::Search(c) => self.search_command(c),
            Command::Side(c) => self.side_command(c),
            Command::Field(c) if self.side.focused => self.side_edit(c),
            Command::Field(c) => self.search_edit(c),
            Command::Detach => self.detach(),
            Command::ConfirmYes => {
                if let Some(confirm) = self.confirm.take()
                    && let Some((group, index)) = self.groups.find(confirm.run)
                {
                    self.close_tab(group, index);
                }
            }
            Command::ConfirmNo => self.confirm = None,
            Command::InstallShellIntegration => self.install_integration(),
            Command::ReloadSettings => self.reload_settings(),
        }
        self.needs_redraw = true;
    }

    /// Scrolls the active shell's scrollback.
    fn scroll_active(&mut self, by: Scroll) {
        let Some(shell) = self.groups.active_group().active_tab() else {
            return;
        };
        let run = &shell.run;
        match by {
            Scroll::Lines(n) => run.scroll(n),
            // A page is the screen's rows less one.
            Scroll::Page(n) => {
                // The rows first: the screen's lock is not held across the scroll, which takes it.
                let rows = run.screen().screen().size().0.get() as isize;
                run.scroll(n * (rows - 1).max(1));
            }
            Scroll::Edge(top) => run.scroll_to(top),
        }
    }

    /// Closes a tab, asking first while a command runs in it (known from the shell integration).
    fn request_close(&mut self, group: GroupId, index: usize) {
        let Some(tab) = self.groups.get(group).and_then(|g| g.tabs.get(index)) else {
            return;
        };
        if !tab.idle && tab.run.status().is_running() {
            let name = tab.title.trim_end_matches(['●', '○', ' ']).to_string();
            self.confirm = Some(Confirm { run: tab.run.id, name });
            self.needs_redraw = true;
        } else {
            self.close_tab(group, index);
        }
    }

    /// Drops the tab (which hangs its shell up); the last one closing ends the session.
    fn close_tab(&mut self, group: GroupId, index: usize) {
        let Some(tab) = self.groups.close_tab(group, index) else {
            return;
        };
        if self.confirm.as_ref().is_some_and(|c| c.run == tab.run.id) {
            self.confirm = None;
        }
        if self.search.as_ref().is_some_and(|s| s.run == tab.run.id) {
            self.search = None;
        }
        let failed = match tab.run.status() {
            Status::Failed(e) => Some(e),
            _ => None,
        };
        // A finish the shell reported since the last `Foreground` event isn't lost.
        self.feed_history_from(&tab);
        self.history.close(tab.run.id, Instant::now());
        self.needs_redraw = true;
        if self.groups.tab_count() == 0 {
            let reason = match failed {
                Some(e) => {
                    tracing::error!(error = %e, "the shell failed to start");
                    ByeReason::ServerError(e)
                }
                None => ByeReason::SessionEnded,
            };
            self.end_session(reason);
        } else if let Some(e) = failed {
            tracing::error!(error = %e, "a shell failed to start");
        }
    }

    pub fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Connected { conn, tx, writer } => {
                self.conns.insert(
                    conn,
                    Conn {
                        tx,
                        writer,
                        hello: false,
                    },
                );
            }
            AppEvent::Client { conn, msg } => self.client_msg(conn, msg),
            AppEvent::Closed { conn } => {
                self.conns.remove(&conn);
                if self.attached == Some(conn) {
                    self.attached = None;
                    tracing::info!(conn, "client gone");
                }
            }
            AppEvent::Run(ev) => self.run_event(ev),
            AppEvent::Integration(ev) => self.integration_event(ev),
            AppEvent::Settings(settings, keymap, errors) => self.apply_settings(settings, keymap, errors),
            AppEvent::Tick => self.tick(),
        }
    }

    fn client_msg(&mut self, conn: u64, msg: ClientMsg) {
        let Some(c) = self.conns.get(&conn) else {
            return;
        };
        if !c.hello {
            match msg {
                ClientMsg::Hello {
                    version,
                    rows,
                    cols,
                    truecolor,
                    term,
                    touch,
                } => self.hello(conn, version, rows, cols, truecolor, &term, touch),
                ClientMsg::Query => {
                    let info = self.info();
                    self.answer(conn, info);
                }
                ClientMsg::Kill => {
                    tracing::info!(conn, "session killed");
                    self.answer(conn, ServerMsg::Done);
                    self.end_session(ByeReason::SessionEnded);
                }
                ClientMsg::Rename { name } => {
                    let reply = match self.rename(&name) {
                        Ok(()) => ServerMsg::Done,
                        Err(message) => ServerMsg::Error { message },
                    };
                    self.answer(conn, reply);
                }
                other => {
                    tracing::warn!(conn, ?other, "first message was not Hello");
                    self.conns.remove(&conn);
                }
            }
            return;
        }
        if self.attached != Some(conn) {
            return;
        }
        match msg {
            ClientMsg::Hello { .. } | ClientMsg::Query | ClientMsg::Kill | ClientMsg::Rename { .. } => {
                tracing::warn!(conn, "unexpected message from an attached client ignored")
            }
            ClientMsg::Input(input) => self.input(input),
            ClientMsg::Resize { rows, cols } => self.resize(rows, cols),
            ClientMsg::Detach => self.detach(),
        }
    }

    /// A client says hello: it replaces the attached one (told `TakenOver`), gets `Welcome`, and a
    /// redraw from scratch at its size.
    #[allow(clippy::too_many_arguments)]
    fn hello(&mut self, conn: u64, version: u32, rows: u16, cols: u16, truecolor: bool, term: &str, touch: bool) {
        if version != PROTOCOL_VERSION {
            tracing::warn!(conn, version, "client speaks another protocol");
            self.bye(
                conn,
                ByeReason::VersionMismatch {
                    server: PROTOCOL_VERSION,
                },
            );
            return;
        }
        if let Some(old) = self.attached.take() {
            tracing::info!(old, conn, "session taken over");
            self.bye(old, ByeReason::TakenOver);
        }
        tracing::info!(conn, rows, cols, truecolor, term, "client attached");
        if let Some(c) = self.conns.get_mut(&conn) {
            c.hello = true;
        }
        self.send(
            conn,
            ServerMsg::Welcome {
                session: self.name.clone(),
            },
        );
        if truecolor != self.theme.truecolor() {
            self.theme = theme::dark_plus(truecolor);
        }
        crate::session::touch(&self.config.socket);
        self.attached = Some(conn);
        self.touch = touch;
        // A drag or pointer position belongs to the client that had them.
        self.drag = None;
        self.mouse_at = None;
        self.size = (rows.max(1), cols.max(1));
        self.resync = true;
        self.needs_redraw = true;
    }

    /// How many terminals there are and how many of them run a command.
    pub fn counts(&self) -> (usize, usize) {
        let running = self
            .groups
            .tabs()
            .filter(|s| !s.idle && s.run.status().is_running())
            .count();
        (self.groups.tab_count(), running)
    }

    fn info(&self) -> ServerMsg {
        let (terminals, running) = self.counts();
        ServerMsg::Info {
            terminals: terminals as u32,
            running: running as u32,
            created: self.created,
            attached: self.attached.is_some(),
        }
    }

    /// Sends a management request's answer and forgets the connection (its writer ends after the
    /// answer).
    fn answer(&mut self, conn: u64, msg: ServerMsg) {
        self.send(conn, msg);
        if let Some(c) = self.conns.remove(&conn) {
            self.closing.retain(|w| !w.is_finished());
            self.closing.push(c.writer);
        }
    }

    /// Renames the session: its socket file moves in the same directory (running shells keep the
    /// old path in `$JMUX`). An error text when the name is invalid or another live session has it.
    fn rename(&mut self, name: &str) -> Result<(), String> {
        if !crate::session::valid_name(name) {
            return Err(format!("invalid session name {name}"));
        }
        if name == self.name {
            return Ok(());
        }
        let dir = self
            .config
            .socket
            .parent()
            .ok_or("the session has no socket directory")?;
        let new = crate::session::socket_path(dir, name);
        if std::os::unix::net::UnixStream::connect(&new).is_ok() {
            return Err(format!("session {name} already exists"));
        }
        std::fs::rename(&self.config.socket, &new).map_err(|e| format!("cannot rename: {e}"))?;
        tracing::info!(from = %self.name, to = name, "session renamed");
        self.config.socket = new;
        self.name = name.to_string();
        self.config.name = self.name.clone();
        self.needs_redraw = true;
        Ok(())
    }

    /// Tells `conn` why, then forgets it (its writer ends after sending the message).
    fn bye(&mut self, conn: u64, reason: ByeReason) {
        self.send(conn, ServerMsg::Bye { reason });
        if let Some(c) = self.conns.remove(&conn) {
            self.closing.retain(|w| !w.is_finished());
            self.closing.push(c.writer);
        }
    }

    /// Queues `msg` for `conn` without waiting. A connection whose queue is full (a stalled
    /// client) or already closed is dropped: its writer is aborted, which also ends its reader.
    fn send(&mut self, conn: u64, msg: ServerMsg) -> bool {
        let Some(c) = self.conns.get(&conn) else {
            return false;
        };
        let full = match c.tx.try_send(msg) {
            Ok(()) => return true,
            Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Closed(_)) => false,
        };
        tracing::warn!(conn, full, "dropping a client that cannot take messages");
        if let Some(c) = self.conns.remove(&conn) {
            c.writer.abort();
        }
        if self.attached == Some(conn) {
            self.attached = None;
            self.touch = false;
        }
        false
    }

    fn detach(&mut self) {
        if let Some(conn) = self.attached.take() {
            self.touch = false;
            tracing::info!(conn, "client detached");
            self.bye(conn, ByeReason::Detached);
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.size = (rows.max(1), cols.max(1));
        self.resync = true;
        self.needs_redraw = true;
    }

    fn input(&mut self, input: Input) {
        match input {
            Input::Key(key) => self.key(key),
            Input::Paste(text) if self.confirm.is_none() && self.side.focused => {
                self.side_paste(&text);
                self.needs_redraw = true;
            }
            Input::Paste(text) if self.confirm.is_none() && self.search_active() => self.search_paste(&text),
            Input::Paste(text) => {
                if let Some(shell) = self.groups.active_group().active_tab() {
                    let bracketed = shell.run.screen().screen().bracketed_paste();
                    if shell.run.write(&keys::paste(&text, bracketed)) {
                        shell.run.scroll_to(false);
                    }
                }
            }
            Input::Mouse(m) => self.mouse(m),
            // The client sends `Resize` itself; focus changes mean nothing yet.
            Input::Resize(..) | Input::FocusGained | Input::FocusLost => {}
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if self.notice.take().is_some() {
            self.needs_redraw = true;
        }
        let context = if self.confirm.is_some() {
            Context::Confirm
        } else if self.side.focused {
            Context::Side
        } else if self.search_active() {
            Context::Search
        } else {
            Context::Terminal
        };
        if let Some(command) = self.keymap.lookup(&key, context) {
            // A held key repeats nothing but the tab cycling.
            let repeats = matches!(
                command,
                Command::NextTab
                    | Command::PrevTab
                    | Command::Field(_)
                    | Command::Side(SideCommand::Up | SideCommand::Down | SideCommand::PageUp | SideCommand::PageDown)
            );
            if key.kind == KeyEventKind::Press || repeats {
                // A terminal command (new tab, split, scrolling, ...) takes the keys back from the bar.
                if context == Context::Side
                    && !matches!(
                        command,
                        Command::Side(_)
                            | Command::Field(_)
                            | Command::Detach
                            | Command::InstallShellIntegration
                            | Command::ReloadSettings
                    )
                {
                    self.side.focused = false;
                }
                self.execute(command);
            }
            return;
        }
        if context == Context::Confirm {
            return;
        }
        if context == Context::Side {
            self.side_type(key);
            self.needs_redraw = true;
            return;
        }
        if context == Context::Search {
            self.search_type(key);
            self.needs_redraw = true;
            return;
        }
        if let Some(shell) = self.groups.active_group().active_tab() {
            let application_cursor = shell.run.screen().screen().application_cursor();
            if let Some(bytes) = keys::encode(&key, application_cursor)
                && shell.run.write(&bytes)
            {
                shell.run.scroll_to(false);
                self.needs_redraw = true;
            }
        }
    }

    /// A pointer event: on a tab bar it picks, closes or scrolls tabs; over a terminal it focuses
    /// the group and goes to its shell. Nothing happens while the close question is open.
    fn mouse(&mut self, m: MouseEvent) {
        self.mouse_at = Some((m.column, m.row));
        self.pointer_event(m);
        if self.desired_pointer() != self.pointer {
            self.needs_redraw = true;
        }
    }

    /// A divider drag takes every event until the button is released.
    fn pointer_event(&mut self, m: MouseEvent) {
        if self.confirm.is_some() {
            return;
        }
        if let Some(drag) = &self.drag {
            match (drag, m.kind) {
                (Drag::Divider(d), MouseEventKind::Drag(MouseButton::Left)) => {
                    let ratio = d.ratio_at(m.column, m.row);
                    let d = d.clone();
                    self.groups.set_ratio(&d, ratio);
                }
                (Drag::Tab(_), MouseEventKind::Drag(MouseButton::Left)) => self.tab_drag_to(m.column, m.row),
                (Drag::Terminal { .. }, MouseEventKind::Drag(MouseButton::Left)) => self.select_drag(m),
                (Drag::Terminal { .. }, MouseEventKind::Up(_)) => self.select_release(),
                (Drag::ScrollBar { group, grab }, MouseEventKind::Drag(MouseButton::Left)) => {
                    let (group, grab) = (*group, *grab);
                    self.scrollbar_drag(group, m.row, grab);
                }
                (Drag::ScrollBar { .. }, MouseEventKind::Up(_)) => self.drag = None,
                (Drag::Divider(_), MouseEventKind::Up(_)) => self.drag = None,
                (Drag::SideBorder, MouseEventKind::Drag(MouseButton::Left)) => self.side_resize(m.column),
                (Drag::SideScroll { grab }, MouseEventKind::Drag(MouseButton::Left)) => {
                    let grab = *grab;
                    self.side_scroll_drag(m.row, grab);
                }
                (Drag::SideBorder | Drag::SideScroll { .. }, MouseEventKind::Up(_)) => self.drag = None,
                (Drag::Tab(_), MouseEventKind::Up(_)) => self.finish_tab_drag(m.column, m.row),
                _ => {}
            }
            self.needs_redraw = true;
            return;
        }
        let down = |button| m.kind == MouseEventKind::Down(button);
        let at = self.hit.at(m.column, m.row);
        if let Some(hit) = at
            && matches!(
                hit,
                Hit::SideBorder | Hit::SideFilter | Hit::SideRow(_) | Hit::SideScrollBar | Hit::Side
            )
        {
            if self.side_mouse(hit, m) {
                self.needs_redraw = true;
            }
            return;
        }
        // A press anywhere else takes the keys back from the bar.
        if matches!(m.kind, MouseEventKind::Down(_)) && self.side.focused {
            self.side.focused = false;
        }
        match at {
            Some(Hit::Tab { group, index }) if down(MouseButton::Left) => {
                self.groups.active = group;
                if let Some(g) = self.groups.get_mut(group) {
                    g.active = index;
                    // A drag that never leaves the tab is just a click.
                    self.drag = g.tabs.get(index).map(|t| {
                        Drag::Tab(TabDrag {
                            from: group,
                            index,
                            run: t.run.id,
                            target: None,
                        })
                    });
                }
            }
            Some(Hit::Tab { group, index } | Hit::Close { group, index }) if down(MouseButton::Middle) => {
                self.request_close(group, index)
            }
            Some(Hit::Close { group, index }) if down(MouseButton::Left) => self.request_close(group, index),
            Some(Hit::Arrow { group, forward }) if down(MouseButton::Left) => {
                if let Some(g) = self.groups.get_mut(group) {
                    g.scroll_tabs(if forward { 1 } else { -1 });
                }
            }
            Some(Hit::Tab { group, .. } | Hit::Close { group, .. } | Hit::Arrow { group, .. } | Hit::Bar { group }) => {
                // The wheel over a bar scrolls its tabs; a click elsewhere on it focuses the group.
                match m.kind {
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft => {
                        if let Some(g) = self.groups.get_mut(group) {
                            g.scroll_tabs(-1);
                        }
                    }
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollRight => {
                        if let Some(g) = self.groups.get_mut(group) {
                            g.scroll_tabs(1);
                        }
                    }
                    MouseEventKind::Down(_) => self.groups.active = group,
                    _ => return,
                }
            }
            Some(Hit::Divider(i)) => {
                if down(MouseButton::Left) {
                    self.drag = self.hit.dividers.get(i).cloned().map(Drag::Divider);
                }
            }
            Some(Hit::Terminal { group }) => {
                if matches!(m.kind, MouseEventKind::Down(_)) {
                    self.groups.active = group;
                }
                let text = self.hit.group(group).map(|g| g.text);
                if !text.is_some_and(|text| self.link_click(group, text, m)) {
                    self.terminal_mouse(group, m);
                }
            }
            Some(Hit::ScrollBar { group }) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    self.groups.active = group;
                    self.scrollbar_click(group, m.row);
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => self.wheel(group, m.kind),
                MouseEventKind::Down(_) => self.groups.active = group,
                _ => return,
            },
            _ => return,
        }
        self.needs_redraw = true;
    }

    /// A pointer event over `group`'s terminal: reported to a program that asked for the mouse
    /// (Shift keeps it for jmux's own selection), else it selects and the wheel scrolls the
    /// scrollback. A program's events are not reported while the view is scrolled back.
    fn terminal_mouse(&mut self, group: GroupId, mut m: MouseEvent) {
        let Some(text) = self.hit.group(group).map(|g| g.text) else {
            return;
        };
        let Some(shell) = self.groups.get(group).and_then(|g| g.active_tab()) else {
            return;
        };
        if !shell.run.status().is_running() {
            return;
        }
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
        let reported = {
            let screen = shell.run.screen();
            let s = screen.screen();
            let mode = s.mouse_protocol_mode();
            (mode != MouseProtocolMode::None && s.scrollback() == 0 && !shift).then(|| {
                // Positions are relative to the shell's own corner.
                m.row -= text.y;
                m.column -= text.x;
                mouse::encode(&m, mode, s.mouse_protocol_encoding())
            })
        };
        match reported {
            Some(Some(bytes)) => {
                let motion = matches!(m.kind, MouseEventKind::Drag(_) | MouseEventKind::Moved);
                shell.run.write_mouse(bytes, motion);
            }
            Some(None) => {}
            None => match m.kind {
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => self.wheel(group, m.kind),
                MouseEventKind::Down(MouseButton::Left) => self.select_press(group, text, m),
                _ => {}
            },
        }
    }

    /// The wheel over `group`'s terminal: three lines of scrollback.
    fn wheel(&mut self, group: GroupId, kind: MouseEventKind) {
        let Some(shell) = self.groups.get(group).and_then(|g| g.active_tab()) else {
            return;
        };
        shell.run.scroll(if kind == MouseEventKind::ScrollUp {
            -WHEEL_LINES
        } else {
            WHEEL_LINES
        });
    }

    /// A left press in a terminal's text (its program has no mouse reporting, or Shift is held):
    /// an empty selection at the cell (a double click: the path-like run under it, a triple
    /// click: the row; Shift on a single click extends the selection) and a drag that owns the
    /// mouse until the release.
    fn select_press(&mut self, group: GroupId, text: Rect, m: MouseEvent) {
        let count = self.clicks.click(Instant::now(), m.column, m.row);
        let Some(run) = self.groups.get(group).and_then(|g| g.active_tab()).map(|s| &s.run) else {
            return;
        };
        let (anchor, word) = {
            let screen = run.screen();
            let s = screen.screen();
            let (row, col) = select::screen_cell(s, text, m.column, m.row);
            let cell = select::cell_at(row, col, s.scrollback());
            let seed = match count {
                2 => select::path_run(s, (row, col)),
                3 => {
                    let line = row as isize - s.scrollback() as isize;
                    Some(((line, 0), (line, select::size(s).1)))
                }
                _ => None,
            };
            let extend = m.modifiers.contains(KeyModifiers::SHIFT) && count == 1;
            let (anchor, head) = match run.selection() {
                Some(old) if extend => ((old.anchor, old.anchor), cell),
                _ => {
                    let (a, h) = seed.unwrap_or((cell, cell));
                    ((a, h), h)
                }
            };
            // Set while the screen is held, so no output lands between the cell and the drag.
            run.set_dragging(true);
            run.set_selection(select::Selection { anchor: anchor.0, head });
            (anchor, count == 2 && seed.is_some())
        };
        self.drag = Some(Drag::Terminal {
            run: run.id,
            anchor,
            word,
            text,
        });
    }

    /// The pointer moved with the selection drag held: the head follows it (the cell clamps into
    /// the text, also with the pointer outside), by words after a double click.
    fn select_drag(&mut self, m: MouseEvent) {
        let Some(Drag::Terminal {
            run,
            anchor,
            word,
            text,
        }) = self.drag.clone()
        else {
            return;
        };
        let Some(run) = self.run_by_id(run) else {
            return;
        };
        // The screen is held from taking the lines output scrolled to setting the selection, so
        // no chunk's shift of the anchor is lost in between.
        let screen = run.screen();
        // Dropped meanwhile (a resize, output that changed its text): the anchor is stale, so
        // the drag ends instead of rebuilding a selection from it.
        if run.selection().is_none() {
            drop(screen);
            run.set_dragging(false);
            self.drag = None;
            return;
        }
        // Output scrolled meanwhile: the anchor followed its text, so must our copy.
        let moved = run.take_drag_lines() as isize;
        let anchor = (
            select::shift_drag_cell(anchor.0, moved),
            select::shift_drag_cell(anchor.1, moved),
        );
        let s = screen.screen();
        let (row, col) = select::screen_cell(s, text, m.column, m.row);
        let head = select::cell_at(row, col, s.scrollback());
        let (start, head) = if word {
            select::round_run_drag(anchor, head, select::path_run(s, (row, col)))
        } else {
            (anchor.0, head)
        };
        run.set_selection(select::Selection { anchor: start, head });
        drop(screen);
        if moved != 0
            && let Some(Drag::Terminal { anchor: stored, .. }) = &mut self.drag
        {
            *stored = anchor;
        }
    }

    /// The button went up: a press without a drag selected no cell, so nothing stays to copy.
    fn select_release(&mut self) {
        if let Some(Drag::Terminal { run, .. }) = self.drag.take()
            && let Some(run) = self.run_by_id(run)
        {
            run.set_dragging(false);
            if run.selection().is_some_and(|s| s.anchor == s.head) {
                run.clear_selection();
            }
        }
    }

    fn run_by_id(&self, id: RunId) -> Option<&jterm::run::Run> {
        self.groups.tabs().find(|t| t.run.id == id).map(|t| &t.run)
    }

    /// A left press on `group`'s scroll bar: on the thumb it is grabbed where it was pressed, off
    /// it the thumb jumps there (centred on the pointer) and is grabbed. The thumb moves in half
    /// cells.
    fn scrollbar_click(&mut self, group: GroupId, y: u16) {
        let Some(hit) = self.hit.group(group).map(|g| g.scroll) else {
            return;
        };
        let Some(thumb) = hit.thumb else {
            return;
        };
        let Some(run) = self.groups.get(group).and_then(|g| g.active_tab()).map(|s| &s.run) else {
            return;
        };
        let scroll = run.scrollable().scroll();
        let cell = y.saturating_sub(hit.track.y);
        let (pos, len) = (cell * 2, hit.track.height * 2);
        let grab = if thumb.contains(cell) {
            pos.saturating_sub(scroll.thumb(len).map_or(0, |t| t.start))
        } else {
            let grab = scroll.thumb(len).map_or(0, |t| t.len / 2);
            run.set_scrollback_from_top(scroll.top_at(len, i32::from(pos) - i32::from(grab)));
            grab
        };
        self.drag = Some(Drag::ScrollBar { group, grab });
    }

    /// The pointer at row `y` with the thumb held by `grab`.
    fn scrollbar_drag(&mut self, group: GroupId, y: u16, grab: u16) {
        let Some(hit) = self.hit.group(group).map(|g| g.scroll) else {
            return;
        };
        let Some(run) = self.groups.get(group).and_then(|g| g.active_tab()).map(|s| &s.run) else {
            return;
        };
        let scroll = run.scrollable().scroll();
        let len = hit.track.height * 2;
        let start = 2 * (i32::from(y) - i32::from(hit.track.y)) - i32::from(grab);
        run.set_scrollback_from_top(scroll.top_at(len, start));
    }

    /// The pointer's shape: a resize arrow over a divider (and while dragging one), the I-beam over
    /// a terminal unless its program takes the mouse itself, else the default.
    fn desired_pointer(&self) -> Pointer {
        let resize = |d: &Divider| match d.dir {
            SplitDir::Right => Pointer::ColResize,
            SplitDir::Down => Pointer::RowResize,
        };
        match &self.drag {
            Some(Drag::Divider(d)) => return resize(d),
            Some(Drag::SideBorder) => return Pointer::ColResize,
            Some(Drag::Tab(_) | Drag::ScrollBar { .. } | Drag::SideScroll { .. }) => return Pointer::Default,
            // Held for the whole selection, as jed holds the I-beam during a text drag.
            Some(Drag::Terminal { .. }) => return Pointer::Text,
            None => {}
        }
        let Some((x, y)) = self.mouse_at else {
            return Pointer::Default;
        };
        match self.hit.at(x, y) {
            Some(Hit::Divider(i)) => self.hit.dividers.get(i).map_or(Pointer::Default, resize),
            Some(Hit::SideBorder) => Pointer::ColResize,
            Some(Hit::Terminal { group }) => {
                let reporting = self
                    .groups
                    .get(group)
                    .and_then(|g| g.active_tab())
                    .is_some_and(|s| s.run.screen().screen().mouse_protocol_mode() != MouseProtocolMode::None);
                if reporting { Pointer::Default } else { Pointer::Text }
            }
            _ => Pointer::Default,
        }
    }

    fn run_event(&mut self, ev: Event) {
        match ev {
            Event::Output(_) | Event::Started(_) => self.needs_redraw = true,
            Event::Foreground { id, name, idle, error } => {
                if let Some((group, index)) = self.groups.find(id)
                    && let Some(shell) = self.groups.get_mut(group).and_then(|g| g.tabs.get_mut(index))
                {
                    shell.title = shell::title(&shell.base, shell.number, Some((&name, idle)));
                    shell.idle = idle;
                    shell.error = error;
                    self.needs_redraw = true;
                }
                self.feed_history(id);
            }
            // The shell exited (or never started): its tab goes, with no question.
            Event::Ended(id) => {
                // A last finish may not have sent a `Foreground`.
                self.feed_history(id);
                if let Some((group, index)) = self.groups.find(id) {
                    self.close_tab(group, index);
                }
            }
        }
    }

    /// The last terminal closed (or the session was killed): the client is told and the server quits.
    fn end_session(&mut self, reason: ByeReason) {
        tracing::info!(session = %self.name, ?reason, "session ended");
        if let Some(conn) = self.attached.take() {
            self.bye(conn, reason);
        }
        self.should_quit = true;
    }

    /// Draws for the attached client (nothing while there is none) and sends what the stream
    /// backend wrote.
    pub fn redraw(&mut self, terminal: &mut Terminal<StreamBackend>) -> io::Result<()> {
        self.needs_redraw = false;
        let Some(conn) = self.attached else {
            self.out.clear();
            return Ok(());
        };
        if self.resync {
            self.resync = false;
            // A new client starts with the default pointer.
            self.pointer = Pointer::Default;
            let (rows, cols) = self.size;
            let backend = terminal.backend_mut();
            backend.take_output();
            backend.set_size(cols, rows);
            terminal.clear()?;
        }
        terminal.draw(|f| ui::render(f, self))?;
        let mut bytes = terminal.backend_mut().take_output();
        // After the frame: a copy's OSC 52.
        bytes.append(&mut self.out);
        // The client's pointer follows what the last frame drew.
        let pointer = self.desired_pointer();
        if pointer != self.pointer {
            self.pointer = pointer;
            let _ = set_pointer(&mut bytes, pointer);
        }
        if !bytes.is_empty() {
            self.send(conn, ServerMsg::Output(bytes));
        }
        Ok(())
    }

    /// Hangs up: the shell is killed and the connections' queues close. Returns the writer tasks
    /// to wait for, so queued messages (`Bye`) reach their clients.
    pub fn shutdown(&mut self) -> Vec<JoinHandle<()>> {
        self.groups = Groups::new();
        self.confirm = None;
        let mut writers = std::mem::take(&mut self.closing);
        writers.extend(self.conns.drain().map(|(_, c)| c.writer));
        writers
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// An app with no shell and a connection 1 that said hello with a queue of `cap`; the receiver
    /// plays the writer task's queue, and the writer itself never finishes by itself.
    async fn attached_app(cap: usize) -> (App, tokio::sync::mpsc::Receiver<ServerMsg>, Terminal<StreamBackend>) {
        let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
        let config = Config {
            name: "t".into(),
            socket: PathBuf::new(),
            program: "/bin/sh".into(),
            cwd: PathBuf::from("/"),
            open_url: |_| Ok(()),
            dirs: None,
            settings: Settings::default(),
            settings_path: None,
            settings_errors: Vec::new(),
            keybindings_path: None,
            keymap: None,
        };
        let mut app = App::new(config, events);
        let rx = connect(&mut app, 1, cap);
        assert_eq!(app.attached, Some(1));
        (app, rx, Terminal::new(StreamBackend::new(40, 10)).unwrap())
    }

    fn connect(app: &mut App, conn: u64, cap: usize) -> tokio::sync::mpsc::Receiver<ServerMsg> {
        let (tx, rx) = tokio::sync::mpsc::channel(cap);
        let writer = tokio::spawn(std::future::pending::<()>());
        app.handle(AppEvent::Connected { conn, tx, writer });
        let msg = ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            rows: 10,
            cols: 40,
            truecolor: true,
            term: "xterm".into(),
            touch: false,
        };
        app.handle(AppEvent::Client { conn, msg });
        rx
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        let msg = ClientMsg::Input(Input::Key(key(code, modifiers)));
        app.handle(AppEvent::Client { conn: 1, msg });
    }

    #[tokio::test]
    async fn ctrl_shift_d_detaches_and_plain_ctrl_d_does_not() {
        let (mut app, mut rx, _t) = attached_app(8).await;
        assert!(matches!(rx.try_recv(), Ok(ServerMsg::Welcome { .. })));
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.attached, Some(1));
        assert!(rx.try_recv().is_err(), "Ctrl+D must not detach");
        press(
            &mut app,
            KeyCode::Char('D'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(app.attached, None);
        assert!(matches!(
            rx.try_recv(),
            Ok(ServerMsg::Bye {
                reason: ByeReason::Detached
            })
        ));
        assert!(!app.should_quit, "a detach leaves the session running");
    }

    #[tokio::test]
    async fn nothing_is_drawn_or_sent_while_detached() {
        let (mut app, mut rx, mut terminal) = attached_app(8).await;
        assert!(matches!(rx.try_recv(), Ok(ServerMsg::Welcome { .. })));
        app.redraw(&mut terminal).unwrap();
        assert!(
            matches!(rx.try_recv(), Ok(ServerMsg::Output(b)) if !b.is_empty()),
            "an attached client gets a frame"
        );
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let _ = rx.try_recv();
        app.resize(12, 50);
        app.redraw(&mut terminal).unwrap();
        assert!(terminal.backend_mut().take_output().is_empty(), "drew for nobody");
        // Nothing was drawn: the backend was not even resized to the detached client's size.
        let size = terminal.size().unwrap();
        assert_eq!(
            (size.width, size.height),
            (40, 10),
            "the screen was touched while detached"
        );
        assert!(rx.try_recv().is_err());
        // A closed connection is detached too.
        let (mut app, mut rx, mut terminal) = attached_app(8).await;
        app.handle(AppEvent::Closed { conn: 1 });
        assert_eq!(app.attached, None);
        app.needs_redraw = true;
        app.redraw(&mut terminal).unwrap();
        assert!(terminal.backend_mut().take_output().is_empty());
        let _ = rx.try_recv();
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_client_that_stops_reading_is_dropped_not_buffered() {
        let (mut app, mut rx, mut terminal) = attached_app(2).await;
        // Welcome is queued and nobody reads: frames fill the queue, then the client goes.
        for _ in 0..4 {
            app.resync = true;
            app.redraw(&mut terminal).unwrap();
        }
        assert_eq!(app.attached, None, "the stalled client must be dropped");
        assert!(app.conns.is_empty());
        assert!(!app.should_quit, "the session itself goes on");
        let mut queued = 0;
        while rx.try_recv().is_ok() {
            queued += 1;
        }
        assert_eq!(queued, 2, "the queue stays at its capacity");
        // A later client attaches normally.
        let mut rx2 = connect(&mut app, 2, 8);
        assert!(matches!(rx2.try_recv(), Ok(ServerMsg::Welcome { .. })));
        assert_eq!(app.attached, Some(2));
    }

    #[tokio::test]
    async fn shutdown_waits_for_the_writer_of_a_client_that_was_told_bye() {
        let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
        let config = Config {
            name: "t".into(),
            socket: PathBuf::new(),
            program: "/bin/sh".into(),
            cwd: PathBuf::from("/"),
            open_url: |_| Ok(()),
            dirs: None,
            settings: Settings::default(),
            settings_path: None,
            settings_errors: Vec::new(),
            keybindings_path: None,
            keymap: None,
        };
        let mut app = App::new(config, events);
        // A writer that does nothing until the gate opens, then reports what it was given.
        let (tx, mut queue) = tokio::sync::mpsc::channel(8);
        let (gate, gated) = tokio::sync::oneshot::channel::<()>();
        let (delivered_tx, delivered) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            let _ = gated.await;
            let mut got = Vec::new();
            while let Some(msg) = queue.recv().await {
                got.push(msg);
            }
            let _ = delivered_tx.send(got);
        });
        app.handle(AppEvent::Connected { conn: 1, tx, writer });
        let msg = ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            rows: 10,
            cols: 40,
            truecolor: true,
            term: "x".into(),
            touch: false,
        };
        app.handle(AppEvent::Client { conn: 1, msg });
        press(
            &mut app,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let writers = app.shutdown();
        assert_eq!(
            writers.len(),
            1,
            "shutdown must hand back the writer still holding the Bye"
        );
        let _ = gate.send(());
        for w in writers {
            w.await.unwrap();
        }
        let got = delivered.await.unwrap();
        assert!(matches!(
            got.last(),
            Some(ServerMsg::Bye {
                reason: ByeReason::Detached
            })
        ));
    }
}
