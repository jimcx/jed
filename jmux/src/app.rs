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

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use jterm::run::{EnvNames, Event, RunId, SCROLLBACK, Sink, StartOptions, Status};
use jterm::shell::{self, ShellTab};
use jterm::{keys, mouse};
use jtui::stream_backend::StreamBackend;
use jtui::theme::Theme;
use ratatui::Terminal;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Sender, UnboundedSender};
use tokio::task::JoinHandle;
use vt100::MouseProtocolMode;

use crate::protocol::{ByeReason, ClientMsg, PROTOCOL_VERSION, ServerMsg};
use crate::{theme, ui};

/// Shown at the right of the status bar.
pub const DETACH_HINT: &str = "Ctrl+Shift+D detach";

/// Messages a connection may have queued for its writer. A client that falls this far behind is
/// dropped (its terminal can catch up on the next attach) rather than letting memory grow.
pub const OUT_QUEUE: usize = 256;

/// The shell's size until a client says its own.
const INITIAL_SIZE: (u16, u16) = (24, 80);

/// Rows a mouse wheel notch scrolls the scrollback by.
const WHEEL_LINES: isize = 3;

/// The variables a shell gets: the socket path and its terminal's id (jterm sets the second).
const ENV: EnvNames = EnvNames {
    ipc: "JMUX",
    terminal: "JMUX_TERMINAL",
};

/// What a server is started with.
#[derive(Debug, Clone)]
pub struct Config {
    pub name: String,
    /// The session's socket (the shell's `$JMUX`).
    pub socket: PathBuf,
    /// The shell to run: `$SHELL`, else `/bin/sh`.
    pub program: PathBuf,
    /// Its working directory.
    pub cwd: PathBuf,
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
}

struct Conn {
    tx: Sender<ServerMsg>,
    writer: JoinHandle<()>,
    /// Its `Hello` was accepted.
    hello: bool,
}

pub struct App {
    pub name: String,
    config: Config,
    events: UnboundedSender<AppEvent>,
    pub shell: Option<ShellTab>,
    pub theme: Theme,
    conns: HashMap<u64, Conn>,
    /// Writers of connections told `Bye`: `shutdown` waits for them so the message is delivered.
    closing: Vec<JoinHandle<()>>,
    attached: Option<u64>,
    /// The attached client's screen: rows, columns.
    size: (u16, u16),
    /// The next draw starts from a cleared screen at `size` (an attach or a resize).
    resync: bool,
    /// Unix seconds this session started (`jmux ls`).
    created: u64,
    pub needs_redraw: bool,
    pub should_quit: bool,
}

impl App {
    /// Spawns nothing (`start` does).
    pub fn new(config: Config, events: UnboundedSender<AppEvent>) -> App {
        App {
            name: config.name.clone(),
            config,
            events,
            shell: None,
            theme: theme::dark_plus(true),
            conns: HashMap::new(),
            closing: Vec::new(),
            attached: None,
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

    /// Starts the shell at 24×80 (no client has said its size yet); an attach resizes it. Needs a
    /// tokio runtime.
    pub fn start(&mut self) {
        let sink: Sink = {
            let events = self.events.clone();
            Arc::new(move |ev| {
                let _ = events.send(AppEvent::Run(ev));
            })
        };
        let spec = shell::spec(
            &self.config.program,
            &self.config.cwd,
            Some((ENV.ipc, &self.config.socket)),
        );
        let (rows, cols) = INITIAL_SIZE;
        let options = StartOptions {
            env: ENV,
            command_ring: 500,
        };
        self.shell = Some(ShellTab::new(
            RunId(1),
            spec,
            rows - ui::STATUS_ROWS,
            cols,
            sink,
            SCROLLBACK,
            options,
            1,
        ));
        tracing::info!(session = %self.name, program = %self.config.program.display(), "session started");
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
                } => self.hello(conn, version, rows, cols, truecolor, &term),
                ClientMsg::Query => {
                    let info = self.info();
                    self.answer(conn, info);
                }
                ClientMsg::Kill => {
                    tracing::info!(conn, "session killed");
                    self.answer(conn, ServerMsg::Done);
                    self.end_session();
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
    fn hello(&mut self, conn: u64, version: u32, rows: u16, cols: u16, truecolor: bool, term: &str) {
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
        self.size = (rows.max(1), cols.max(1));
        self.resync = true;
        self.needs_redraw = true;
    }

    fn info(&self) -> ServerMsg {
        let running = self
            .shell
            .as_ref()
            .is_some_and(|s| !s.idle && s.run.status().is_running());
        ServerMsg::Info {
            terminals: u32::from(self.shell.is_some()),
            running: u32::from(running),
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
        }
        false
    }

    fn detach(&mut self) {
        if let Some(conn) = self.attached.take() {
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
            Input::Paste(text) => {
                if let Some(shell) = &self.shell {
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
        if is_detach(&key) {
            if key.kind == KeyEventKind::Press {
                self.detach();
            }
            return;
        }
        if let Some(shell) = &self.shell {
            let application_cursor = shell.run.screen().screen().application_cursor();
            if let Some(bytes) = keys::encode(&key, application_cursor)
                && shell.run.write(&bytes)
            {
                shell.run.scroll_to(false);
                self.needs_redraw = true;
            }
        }
    }

    /// A pointer event over the shell: reported to a program that asked for the mouse, else the
    /// wheel scrolls the scrollback.
    fn mouse(&mut self, m: MouseEvent) {
        let Some(shell) = &self.shell else {
            return;
        };
        let area = ui::shell_area(self.size.0, self.size.1);
        if m.row >= area.height || m.column >= area.width || !shell.run.status().is_running() {
            return;
        }
        let bytes = {
            let screen = shell.run.screen();
            let s = screen.screen();
            if s.mouse_protocol_mode() == MouseProtocolMode::None {
                None
            } else if s.scrollback() != 0 {
                return;
            } else {
                Some(mouse::encode(&m, s.mouse_protocol_mode(), s.mouse_protocol_encoding()))
            }
        };
        match bytes {
            Some(Some(bytes)) => {
                let motion = matches!(m.kind, MouseEventKind::Drag(_) | MouseEventKind::Moved);
                shell.run.write_mouse(bytes, motion);
            }
            Some(None) => {}
            None => {
                match m.kind {
                    MouseEventKind::ScrollUp => shell.run.scroll(-WHEEL_LINES),
                    MouseEventKind::ScrollDown => shell.run.scroll(WHEEL_LINES),
                    _ => return,
                }
                self.needs_redraw = true;
            }
        }
    }

    fn run_event(&mut self, ev: Event) {
        match ev {
            Event::Output(_) | Event::Started(_) => self.needs_redraw = true,
            Event::Foreground { name, idle, error, .. } => {
                if let Some(shell) = &mut self.shell {
                    shell.title = shell::title(&shell.base, shell.number, Some((&name, idle)));
                    shell.idle = idle;
                    shell.error = error;
                    self.needs_redraw = true;
                }
            }
            Event::Ended(_) => self.end_session(),
        }
    }

    /// The shell ended (the last terminal closed): the client is told and the server quits.
    fn end_session(&mut self) {
        let reason = match self.shell.as_ref().map(|s| s.run.status()) {
            Some(Status::Failed(e)) => {
                tracing::error!(error = %e, "the shell failed to start");
                ByeReason::ServerError(e)
            }
            _ => ByeReason::SessionEnded,
        };
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
            return Ok(());
        };
        if self.resync {
            self.resync = false;
            let (rows, cols) = self.size;
            let backend = terminal.backend_mut();
            backend.take_output();
            backend.set_size(cols, rows);
            terminal.clear()?;
        }
        terminal.draw(|f| ui::render(f, self))?;
        let bytes = terminal.backend_mut().take_output();
        if !bytes.is_empty() {
            self.send(conn, ServerMsg::Output(bytes));
        }
        Ok(())
    }

    /// Hangs up: the shell is killed and the connections' queues close. Returns the writer tasks
    /// to wait for, so queued messages (`Bye`) reach their clients.
    pub fn shutdown(&mut self) -> Vec<JoinHandle<()>> {
        self.shell = None;
        let mut writers = std::mem::take(&mut self.closing);
        writers.extend(self.conns.drain().map(|(_, c)| c.writer));
        writers
    }
}

/// Ctrl+Shift+D, as a terminal reports it with or without the kitty keyboard protocol's case.
fn is_detach(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('d' | 'D')) && key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn detach_key_is_ctrl_shift_d_only() {
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert!(is_detach(&key(KeyCode::Char('d'), cs)));
        assert!(is_detach(&key(KeyCode::Char('D'), cs)));
        assert!(!is_detach(&key(KeyCode::Char('d'), KeyModifiers::CONTROL)));
        assert!(!is_detach(&key(KeyCode::Char('d'), KeyModifiers::SHIFT)));
        assert!(!is_detach(&key(KeyCode::Char('x'), cs)));
    }
}
