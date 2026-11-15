//! End-to-end tests for jmux_server-b (the server, part of the task's "done when"): a real server
//! (its own thread and runtime, a real `/bin/sh` on a PTY) in a temp socket dir, driven by a test
//! client over the socket that feeds the server's output into a `vt100` screen. Covers: the shell
//! shown above the status bar, keys and paste reaching it, resize, take-over keeping the screen,
//! detach keeping the server, the session ending with the shell (socket removed), a version
//! mismatch, stale and live sockets. Every wait is a deadline loop; nothing sleeps for a fixed time.
//!
//! jmux_server-c (client and CLI, the rest of "done when"): `client::attach` driven with a channel of
//! terminal events and a `vt100` screen as its output against the same real server: a shell shown,
//! detach and its line, `ls`, re-attach at another size with scrollback, take-over, `exit` ending the
//! session, a vanishing server, `$JMUX` refusing `new` / `attach`, `kill-session`, `rename-session`.
//! (`kill -9` of a real server is in `tests/cli.rs`.)

use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tokio::net::UnixStream;

use crate::app::Config;
use crate::client::{self, Outcome};
use crate::protocol::{ByeReason, ClientMsg, PROTOCOL_VERSION, ServerMsg, read_msg, write_all_msg};
use crate::{server, session};

const WAIT: Duration = Duration::from_secs(10);

struct TestServer {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    socket: PathBuf,
    thread: Option<std::thread::JoinHandle<std::io::Result<()>>>,
}

impl TestServer {
    /// Starts the server of session `work` on its own runtime and waits for its socket.
    fn start() -> TestServer {
        let tmp = tempfile::tempdir().unwrap();
        // `prepare_dir` creates this one with mode 0700 (the temp dir itself may be open to others).
        let dir = tmp.path().join("run");
        let socket = session::socket_path(&dir, "work");
        let thread = spawn_thread(dir.clone());
        if let Err(e) = server::wait_for_socket(&socket, WAIT, || !thread.is_finished()) {
            let result = if thread.is_finished() {
                Some(thread.join())
            } else {
                None
            };
            panic!("{e}: {result:?}");
        }
        TestServer {
            _dir: tmp,
            dir,
            socket,
            thread: Some(thread),
        }
    }

    /// Waits for the server to exit (the session ended) and returns its result.
    fn join(&mut self) -> std::io::Result<()> {
        let thread = self.thread.take().unwrap();
        let deadline = Instant::now() + WAIT;
        while !thread.is_finished() {
            assert!(Instant::now() < deadline, "server did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        thread.join().unwrap()
    }
}

fn config(dir: &Path) -> Config {
    Config {
        name: "work".into(),
        socket: session::socket_path(dir, "work"),
        program: "/bin/sh".into(),
        cwd: std::env::temp_dir(),
        open_url: |_| Ok(()),
        dirs: None,
        settings: Default::default(),
        settings_path: None,
        settings_errors: Vec::new(),
        keybindings_path: None,
        keymap: None,
    }
}

fn spawn_thread(dir: PathBuf) -> std::thread::JoinHandle<std::io::Result<()>> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(server::run(&dir, config(&dir)));
        rt.shutdown_timeout(Duration::from_secs(1));
        result
    })
}

fn parser(rows: u16, cols: u16) -> vt100::Parser {
    vt100::Parser::new(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap(), 0)
}

struct Client {
    stream: UnixStream,
    screen: vt100::Parser,
}

fn hello(rows: u16, cols: u16) -> ClientMsg {
    ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        rows,
        cols,
        truecolor: true,
        term: "xterm-256color".into(),
    }
}

impl Client {
    async fn connect(socket: &Path) -> Client {
        let stream = UnixStream::connect(socket).await.unwrap();
        Client {
            stream,
            screen: parser(1, 1),
        }
    }

    /// Connects and attaches with a `rows` × `cols` screen; `Welcome` is checked.
    async fn attach(socket: &Path, rows: u16, cols: u16) -> Client {
        let mut c = Client::connect(socket).await;
        c.screen = parser(rows, cols);
        c.send(hello(rows, cols)).await;
        match c.next().await {
            ServerMsg::Welcome { session } => assert_eq!(session, "work"),
            other => panic!("expected Welcome, got {other:?}"),
        }
        c
    }

    async fn send(&mut self, msg: ClientMsg) {
        write_all_msg(&mut self.stream, &msg).await.unwrap();
    }

    /// The next message, or a panic after `WAIT`. `Output` is applied to the screen too.
    async fn next(&mut self) -> ServerMsg {
        let msg = tokio::time::timeout(WAIT, read_msg::<ServerMsg, _>(&mut self.stream))
            .await
            .expect("no message from the server in time")
            .expect("bad frame")
            .expect("the server closed the connection");
        if let ServerMsg::Output(bytes) = &msg {
            self.screen.process(bytes);
        }
        msg
    }

    /// Reads until `done(screen)`; a `Bye` meanwhile is a failure.
    async fn wait_screen(&mut self, what: &str, done: impl Fn(&vt100::Screen) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(self.screen.screen()) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}:\n{}",
                self.text()
            );
            match self.next().await {
                ServerMsg::Output(_) => {}
                other => panic!("unexpected {other:?} while waiting for {what}:\n{}", self.text()),
            }
        }
    }

    async fn wait_text(&mut self, needle: &str) {
        let needle = needle.to_string();
        self.wait_screen(&format!("{needle:?}"), |s| s.contents().contains(&needle))
            .await;
    }

    /// Reads until `Bye`, returning its reason.
    async fn wait_bye(&mut self) -> ByeReason {
        loop {
            if let ServerMsg::Bye { reason } = self.next().await {
                return reason;
            }
        }
    }

    async fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.send(ClientMsg::Input(Event::Key(KeyEvent::new(code, modifiers))))
            .await;
    }

    /// Types `text` as key presses (`\r` is Enter).
    async fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            let code = if ch == '\r' { KeyCode::Enter } else { KeyCode::Char(ch) };
            self.key(code, KeyModifiers::NONE).await;
        }
    }

    fn text(&self) -> String {
        self.screen.screen().contents()
    }

    fn row(&self, row: u16) -> String {
        let cols = self.screen.screen().size().1.get();
        self.screen.screen().rows(0, cols).nth(row as usize).unwrap_or_default()
    }

    /// Ends the session: `exit` in the shell, the `Bye`, the server gone.
    async fn end(mut self, server: &mut TestServer) {
        self.type_text("exit\r").await;
        assert_eq!(self.wait_bye().await, ByeReason::SessionEnded);
        server.join().unwrap();
        assert!(!server.socket.exists(), "the socket outlived the session");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_shows_a_shell_above_the_status_bar_and_runs_keys() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 60).await;
    c.wait_screen("the status bar", |s| {
        s.contents().contains("work") && s.contents().contains("detach")
    })
    .await;
    assert!(
        c.row(19).contains(" work"),
        "status bar on the last row: {:?}",
        c.row(19)
    );
    assert!(c.row(19).trim_end().ends_with("Ctrl+Shift+D detach"), "{:?}", c.row(19));
    c.type_text("echo hi-$((1+2))\r").await;
    c.wait_text("hi-3").await;
    assert!(!c.row(19).contains("hi-3"));
    c.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_gives_the_shell_its_environment() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 80).await;
    c.type_text("echo id=$JMUX_TERMINAL sock=$JMUX\r").await;
    let socket = server.socket.display().to_string();
    c.wait_text(&format!("id=1 sock={socket}")).await;
    c.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_pastes_into_the_shell() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 60).await;
    c.send(ClientMsg::Input(Event::Paste("echo pasted-$((2+2))\r".into())))
        .await;
    c.wait_text("pasted-4").await;
    c.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_follows_the_clients_size() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    c.screen
        .screen_mut()
        .set_size(NonZeroU16::new(30).unwrap(), NonZeroU16::new(90).unwrap());
    c.send(ClientMsg::Resize { rows: 30, cols: 90 }).await;
    c.wait_screen("the status bar on the new last row", |s| {
        s.rows(0, 90)
            .nth(29)
            .is_some_and(|r| r.contains(" work") && r.contains("detach"))
    })
    .await;
    c.type_text("stty size\r").await;
    // The group's tab bar takes a row above the shell, the status bar one below.
    c.wait_text("28 89").await;
    c.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_take_over_redraws_everything_and_tells_the_first_client() {
    let mut server = TestServer::start();
    let mut first = Client::attach(&server.socket, 20, 60).await;
    first.type_text("echo before-$((3+4))\r").await;
    first.wait_text("before-7").await;
    let mut second = Client::attach(&server.socket, 25, 70).await;
    assert_eq!(first.wait_bye().await, ByeReason::TakenOver);
    second.wait_text("before-7").await;
    second.wait_text("detach").await;
    assert!(second.row(24).contains(" work"), "{:?}", second.row(24));
    // The new client drives the shell.
    second.type_text("echo after-$((1+1))\r").await;
    second.wait_text("after-2").await;
    second.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_keeps_running_after_a_detach_and_keeps_the_screen() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 60).await;
    c.type_text("echo kept-$((5+5))\r").await;
    c.wait_text("kept-10").await;
    c.key(KeyCode::Char('D'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        .await;
    assert_eq!(c.wait_bye().await, ByeReason::Detached);
    // Still listening; the same screen comes back, and output while away is not lost.
    let mut again = Client::attach(&server.socket, 20, 60).await;
    again.wait_text("kept-10").await;
    again.type_text("echo while-$((6+1))\r").await;
    again.wait_text("while-7").await;
    again.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_a_client_closing_is_a_detach() {
    let mut server = TestServer::start();
    let c = Client::attach(&server.socket, 20, 60).await;
    drop(c);
    let mut again = Client::attach(&server.socket, 20, 60).await;
    again.wait_text("detach").await;
    again.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_refuses_another_protocol_version() {
    let mut server = TestServer::start();
    let mut c = Client::connect(&server.socket).await;
    c.send(ClientMsg::Hello {
        version: PROTOCOL_VERSION + 1,
        rows: 20,
        cols: 60,
        truecolor: false,
        term: "xterm".into(),
    })
    .await;
    assert_eq!(
        c.wait_bye().await,
        ByeReason::VersionMismatch {
            server: PROTOCOL_VERSION
        }
    );
    // The refused client did not disturb the session.
    let c = Client::attach(&server.socket, 20, 60).await;
    c.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_drops_a_client_that_does_not_start_with_hello() {
    let mut server = TestServer::start();
    let mut c = Client::connect(&server.socket).await;
    c.send(ClientMsg::Detach).await;
    let closed = tokio::time::timeout(WAIT, read_msg::<ServerMsg, _>(&mut c.stream))
        .await
        .expect("not closed");
    assert!(matches!(closed, Ok(None)), "{closed:?}");
    let c = Client::attach(&server.socket, 20, 60).await;
    c.end(&mut server).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_ends_when_the_shell_exits_and_removes_its_socket() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    assert!(server.socket.exists());
    c.end(&mut server).await;
    assert!(server.dir.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_ctrl_d_without_shift_goes_to_the_shell() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 60).await;
    // The shell reads input once it has run a command.
    c.type_text("echo ready-$((1+1))\r").await;
    c.wait_text("ready-2").await;
    // End of input at an empty prompt makes `sh` exit: the session ends, nobody detached.
    c.key(KeyCode::Char('d'), KeyModifiers::CONTROL).await;
    assert_eq!(c.wait_bye().await, ByeReason::SessionEnded);
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_puts_the_cursor_where_the_shells_is() {
    let mut server = TestServer::start();
    let mut c = Client::attach(&server.socket, 20, 60).await;
    c.type_text("abc").await;
    c.wait_screen("the typed text", |s| {
        s.rows(0, 60).any(|r| r.trim_end().ends_with("abc"))
    })
    .await;
    let (row, text) = (0..20u16)
        .map(|r| (r, c.row(r)))
        .find(|(_, t)| t.trim_end().ends_with("abc"))
        .unwrap();
    let screen = c.screen.screen();
    assert!(!screen.hide_cursor(), "the cursor must be shown");
    assert_eq!(screen.cursor_position(), (row, text.trim_end().chars().count() as u16));
    c.type_text("\x15exit\r").await;
    assert_eq!(c.wait_bye().await, ByeReason::SessionEnded);
    server.join().unwrap();
}

/// What the stale-socket test prints at each step (shown only when it fails), so a failure says
/// which step it died in and what the socket looked like.
fn flake_step(t0: Instant, step: &str, socket: &Path) {
    eprintln!(
        "FLAKE jmux_server_replaces_a_stale_socket_but_not_a_live_one: step `{step}` at {:?}, socket exists {}, is a socket file {}",
        t0.elapsed(),
        socket.exists(),
        std::fs::metadata(socket).is_ok_and(|m| std::os::unix::fs::FileTypeExt::is_socket(&m.file_type())),
    );
}

// FLAKE: 2026-11-14, failed once with bad frame: Io(ConnectionReset) at line 141 (`Client::next`)
#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_replaces_a_stale_socket_but_not_a_live_one() {
    let t0 = Instant::now();
    let mut server = TestServer::start();
    flake_step(t0, "server started", &server.socket);
    // A second server for the same name finds the live socket and refuses.
    let second = spawn_thread(server.dir.clone());
    let deadline = Instant::now() + WAIT;
    while !second.is_finished() {
        assert!(
            Instant::now() < deadline,
            "a second server kept running on a live socket instead of refusing"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let err = second.join().unwrap().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    assert!(
        server.socket.exists(),
        "the refused server must not remove the live socket"
    );
    flake_step(t0, "second server refused; attaching to the first", &server.socket);
    let c = Client::attach(&server.socket, 20, 60).await;
    flake_step(t0, "attached to the first; ending it", &server.socket);
    c.end(&mut server).await;
    flake_step(t0, "first server ended", &server.socket);

    // A leftover file nobody listens on is replaced.
    drop(std::os::unix::net::UnixListener::bind(&server.socket).unwrap());
    assert!(server.socket.exists());
    server.thread = Some(spawn_thread(server.dir.clone()));
    server::wait_for_socket(&server.socket, WAIT, || true).unwrap();
    flake_step(t0, "replacement server's socket is up; attaching", &server.socket);
    let c = Client::attach(&server.socket, 20, 60).await;
    flake_step(t0, "attached to the replacement; ending it", &server.socket);
    c.end(&mut server).await;
    flake_step(t0, "done", &server.socket);
}

// ---- jmux_server-c: the client and the CLI ----

use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crossterm::event::{MouseEvent, MouseEventKind};
use tokio::sync::{mpsc, oneshot};

/// The terminal's events as a stream (what `EventStream` is in the real client).
struct Events(mpsc::UnboundedReceiver<std::io::Result<Event>>);

impl futures_util::Stream for Events {
    type Item = std::io::Result<Event>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

/// The client's "terminal": what it writes goes through a `vt100` parser.
#[derive(Clone)]
struct Screen(Arc<Mutex<vt100::Parser>>);

impl Write for Screen {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().process(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A running `client::attach` with the test holding its terminal's ends.
struct Attached {
    events: mpsc::UnboundedSender<std::io::Result<Event>>,
    screen: Screen,
    hangup: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Outcome>,
}

impl Attached {
    async fn start(socket: &Path, rows: u16, cols: u16) -> Attached {
        let stream = UnixStream::connect(socket).await.unwrap();
        let (events, rx) = mpsc::unbounded_channel();
        let (hangup, hangup_rx) = oneshot::channel::<()>();
        let screen = Screen(Arc::new(Mutex::new(parser(rows, cols))));
        let mut out = screen.clone();
        let terminal = client::Terminal {
            rows,
            cols,
            truecolor: true,
            term: "xterm-256color".into(),
        };
        let task = tokio::spawn(async move {
            let signal = async move {
                if hangup_rx.await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            client::attach(stream, &terminal, Events(rx), &mut out, signal)
                .await
                .unwrap()
        });
        Attached {
            events,
            screen,
            hangup: Some(hangup),
            task,
        }
    }

    fn send(&self, event: Event) {
        self.events.send(Ok(event)).unwrap();
    }

    fn key(&self, code: KeyCode, modifiers: KeyModifiers) {
        self.send(Event::Key(KeyEvent::new(code, modifiers)));
    }

    fn type_text(&self, text: &str) {
        for ch in text.chars() {
            let code = if ch == '\r' { KeyCode::Enter } else { KeyCode::Char(ch) };
            self.key(code, KeyModifiers::NONE);
        }
    }

    fn rows(&self) -> Vec<String> {
        let p = self.screen.0.lock().unwrap();
        let cols = p.screen().size().1.get();
        p.screen().rows(0, cols).collect()
    }

    fn text(&self) -> String {
        self.screen.0.lock().unwrap().screen().contents()
    }

    async fn wait_rows(&self, what: &str, done: impl Fn(&[String]) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.rows()) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}:\n{}",
                self.text()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn wait_text(&self, needle: &str) {
        self.wait_rows(&format!("{needle:?}"), |rows| rows.iter().any(|r| r.contains(needle)))
            .await;
    }

    /// The client's return value (it must come within `WAIT`).
    async fn outcome(self) -> Outcome {
        tokio::time::timeout(WAIT, self.task)
            .await
            .expect("the client did not return")
            .unwrap()
    }
}

fn detach_key() -> Event {
    Event::Key(KeyEvent::new(
        KeyCode::Char('D'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_new_shows_a_shell_with_the_session_in_the_status_bar() {
    let mut server = TestServer::start();
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_rows("the status bar", |r| r.last().is_some_and(|l| l.contains(" work")))
        .await;
    c.type_text("echo hi-$((1+2))\r");
    c.wait_text("hi-3").await;
    c.type_text("exit\r");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_the_detach_key_returns_with_the_detached_line_and_ls_lists_the_session() {
    let mut server = TestServer::start();
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    // While attached, `ls` says so.
    let dir = server.dir.clone();
    let lines = tokio::task::spawn_blocking(move || crate::cli::ls(&dir)).await.unwrap();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("work: 1 terminal, "), "{lines:?}");
    assert!(lines[0].ends_with(", attached"), "{lines:?}");
    c.send(detach_key());
    let outcome = c.outcome().await;
    assert_eq!(outcome, Outcome::Bye(ByeReason::Detached));
    assert_eq!(outcome.message("work"), "[detached from work]");
    assert_eq!(outcome.exit_code(), 0);
    // The server keeps running, listed as not attached.
    let dir = server.dir.clone();
    let lines = tokio::task::spawn_blocking(move || crate::cli::ls(&dir)).await.unwrap();
    assert!(
        lines[0].starts_with("work: ") && lines[0].ends_with(", detached"),
        "{lines:?}"
    );
    let c = Attached::start(&server.socket, 20, 60).await;
    c.type_text("exit\r");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_attach_shows_the_same_screen_and_scrollback_at_the_new_size() {
    let mut server = TestServer::start();
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    c.type_text("seq 1 100\r");
    c.wait_rows("the end of the output", |r| r.iter().any(|l| l.trim() == "100"))
        .await;
    c.send(detach_key());
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::Detached));

    let c = Attached::start(&server.socket, 12, 40).await;
    c.wait_rows("the old screen at 12 rows", |r| {
        r.len() == 12 && r.iter().any(|l| l.trim() == "100") && r[11].contains(" work")
    })
    .await;
    // Scrollback: the wheel scrolls back to the first lines of the output.
    for _ in 0..50 {
        c.send(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 5,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }));
    }
    c.wait_rows("an early line in the scrollback", |r| r.iter().any(|l| l.trim() == "3"))
        .await;
    c.type_text("exit\r");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_a_second_attach_takes_over_and_the_first_client_says_so() {
    let mut server = TestServer::start();
    let first = Attached::start(&server.socket, 20, 60).await;
    first.wait_text("detach").await;
    first.type_text("echo mine\r");
    first
        .wait_rows("the echo", |r| r.iter().any(|l| l.trim() == "mine"))
        .await;
    let second = Attached::start(&server.socket, 20, 60).await;
    let outcome = first.outcome().await;
    assert_eq!(outcome, Outcome::Bye(ByeReason::TakenOver));
    assert_eq!(outcome.message("work"), "[work taken over by another client]");
    second
        .wait_rows("the same screen", |r| r.iter().any(|l| l.trim() == "mine"))
        .await;
    second.type_text("exit\r");
    assert_eq!(second.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_exit_ends_the_session_removes_the_socket_and_says_so() {
    let mut server = TestServer::start();
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    c.type_text("exit\r");
    let outcome = c.outcome().await;
    assert_eq!(outcome.message("work"), "[session work ended]");
    assert_eq!(outcome.exit_code(), 0);
    server.join().unwrap();
    assert!(!server.socket.exists(), "the socket outlived the session");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_a_vanishing_server_is_reported_as_gone() {
    // A stand-in server that accepts, reads the hello and drops the connection (as `kill -9` of
    // the real one does; that is in tests/cli.rs).
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("gone.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let fake = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let _ = read_msg::<ClientMsg, _>(&mut s).await;
        write_all_msg(&mut s, &ServerMsg::Output(b"hello".to_vec()))
            .await
            .unwrap();
    });
    let c = Attached::start(&socket, 5, 20).await;
    let outcome = c.outcome().await;
    fake.await.unwrap();
    assert_eq!(outcome, Outcome::ServerGone);
    assert_eq!(outcome.message("work"), "[server of work went away]");
    assert_eq!(outcome.exit_code(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_a_hangup_or_a_closed_terminal_detaches() {
    let mut server = TestServer::start();
    let mut c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    c.hangup.take().unwrap().send(()).unwrap();
    assert_eq!(c.outcome().await, Outcome::Hangup);
    // The input ending (the terminal closing) is the same, and the session survives both.
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    let Attached { events, task, .. } = c;
    drop(events);
    assert_eq!(
        tokio::time::timeout(WAIT, task).await.unwrap().unwrap(),
        Outcome::Hangup
    );
    let c = Attached::start(&server.socket, 20, 60).await;
    c.type_text("exit\r");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_resize_events_reach_the_server() {
    let mut server = TestServer::start();
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    // The client's own terminal here is a fixed 20x60 parser: a 10 row screen's status bar is on row 9.
    c.send(Event::Resize(60, 10));
    c.wait_rows("the status bar on row 9", |r| {
        r.get(9).is_some_and(|l| l.contains(" work"))
    })
    .await;
    c.type_text("exit\r");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_jmux_inside_a_jmux_shell_is_refused() {
    let mut server = TestServer::start();
    let inside = server.socket.to_string_lossy().into_owned();
    let dir = server.dir.clone();
    let codes = tokio::task::spawn_blocking(move || {
        use crate::cli::Cmd;
        let new = Cmd::New {
            name: Some("nested".into()),
            dir: None,
        };
        let new = crate::run_cli(new, &dir, Some(&inside), true);
        let attach = crate::run_cli(Cmd::Attach { target: None }, &dir, Some(&inside), true);
        let bare = crate::run_cli(Cmd::Default, &dir, Some(&inside), true);
        (new, attach, bare, session::list(&dir).len())
    })
    .await
    .unwrap();
    assert_eq!(codes, (1, 1, 1, 1), "refused without starting or attaching to anything");
    use crate::cli::Cmd;
    let want = "jmux: already inside session work (unset JMUX to nest anyway)";
    let env = Some(server.socket.to_str().unwrap());
    for cmd in [
        Cmd::New { name: None, dir: None },
        Cmd::Attach { target: None },
        Cmd::Default,
    ] {
        assert_eq!(crate::refusal(&cmd, env, true).as_deref(), Some(want), "{cmd:?}");
    }
    for cmd in [Cmd::Ls, Cmd::Kill { target: "x".into() }, Cmd::Help] {
        assert_eq!(crate::refusal(&cmd, env, true), None, "{cmd:?}");
    }
    assert_eq!(crate::refusal(&Cmd::Default, None, true), None);
    // No terminal: refused with its own message, after the nesting one.
    for cmd in [
        Cmd::New { name: None, dir: None },
        Cmd::Attach { target: None },
        Cmd::Default,
    ] {
        assert_eq!(
            crate::refusal(&cmd, None, false).as_deref(),
            Some("jmux: not a terminal")
        );
        assert_eq!(crate::refusal(&cmd, env, false).as_deref(), Some(want));
    }
    assert_eq!(crate::refusal(&Cmd::Ls, None, false), None);
    let refusal = crate::cli::nested(Some(&server.socket.to_string_lossy())).unwrap();
    assert!(refusal.contains("already inside session work"), "{refusal}");
    let c = Attached::start(&server.socket, 10, 40).await;
    c.type_text("exit\r");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_kill_session_ends_the_session_and_tells_the_client() {
    let mut server = TestServer::start();
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    let dir = server.dir.clone();
    let line = tokio::task::spawn_blocking(move || crate::cli::kill(&dir, "work"))
        .await
        .unwrap();
    assert_eq!(line.unwrap(), "[session work killed]");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
    assert!(!server.socket.exists());
    let dir = server.dir.clone();
    let err = tokio::task::spawn_blocking(move || crate::cli::kill(&dir, "work"))
        .await
        .unwrap();
    assert_eq!(err.unwrap_err(), "jmux: no session work");
}

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_rename_session_moves_the_socket_and_refuses_a_taken_name() {
    let mut server = TestServer::start();
    let c = Attached::start(&server.socket, 20, 60).await;
    c.wait_text("detach").await;
    let dir = server.dir.clone();
    let other = std::os::unix::net::UnixListener::bind(session::socket_path(&dir, "other")).unwrap();
    let d = dir.clone();
    let results = tokio::task::spawn_blocking(move || {
        (
            crate::cli::rename(&d, Some("work"), "other", None),
            crate::cli::rename(&d, Some("work"), "a/b", None),
            crate::cli::rename(&d, None, "play", Some("/x/work.sock")),
        )
    })
    .await
    .unwrap();
    assert_eq!(results.0.unwrap_err(), "jmux: session other already exists");
    assert!(results.1.unwrap_err().contains("invalid session name"));
    assert_eq!(results.2.unwrap(), "[session work renamed to play]");
    drop(other);
    assert!(!server.socket.exists());
    assert!(session::socket_path(&dir, "play").exists());
    c.wait_rows("the new name in the status bar", |r| {
        r.last().is_some_and(|l| l.contains(" play"))
    })
    .await;
    // The renamed session is still the same one, and killing it by its new name works.
    let d = dir.clone();
    let line = tokio::task::spawn_blocking(move || crate::cli::kill(&d, "play"))
        .await
        .unwrap();
    assert!(line.is_ok(), "{line:?}");
    assert_eq!(c.outcome().await, Outcome::Bye(ByeReason::SessionEnded));
    server.join().unwrap();
    assert!(
        !session::socket_path(&dir, "play").exists(),
        "the guard removes the renamed socket"
    );
}

/// The `jmux` binary next to the test binary (cargo builds it for `tests/cli.rs`).
fn jmux_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let path = exe.parent().unwrap().parent().unwrap().join("jmux");
    assert!(path.exists(), "{} is not built", path.display());
    path
}

/// A script in `dir` that runs the jmux binary with `SHELL=/bin/sh`, so servers started through
/// `cli::start` don't run the developer's own shell (zsh's first-run menu would swallow input).
fn sh_pinned_exe(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("jmux-sh");
    let body = format!("#!/bin/sh\nSHELL=/bin/sh exec '{}' \"$@\"\n", jmux_exe().display());
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// Kills a server started through `cli::start` when dropped, also on a panic: `Kill`, then SIGKILL
/// of the pid the socket's peer credentials name.
struct StartedServer {
    socket: PathBuf,
    pid: Option<i32>,
}

impl StartedServer {
    fn new(socket: &Path) -> StartedServer {
        let pid = std::os::unix::net::UnixStream::connect(socket)
            .ok()
            .and_then(|s| peer_pid(&s));
        StartedServer {
            socket: socket.to_path_buf(),
            pid,
        }
    }
}

/// The pid of the process at the other end of a connected Unix socket.
#[cfg(target_os = "linux")]
fn peer_pid(s: &std::os::unix::net::UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt with a ucred buffer of the stated length.
    let rc = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    };
    (rc == 0).then_some(cred.pid)
}

/// The pid of the process at the other end of a connected Unix socket.
#[cfg(target_os = "macos")]
fn peer_pid(s: &std::os::unix::net::UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: getsockopt with a pid_t buffer of the stated length.
    let rc = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut len,
        )
    };
    (rc == 0).then_some(pid)
}

impl Drop for StartedServer {
    fn drop(&mut self) {
        let asked = crate::cli::request(&self.socket, &ClientMsg::Kill);
        if asked.is_err() && !self.socket.exists() {
            return; // already ended by the test
        }
        if let Some(pid) = self.pid {
            let deadline = Instant::now() + Duration::from_secs(2);
            // SAFETY: signal 0 probes the pid; SIGKILL goes to the server this guard started.
            while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if unsafe { libc::kill(pid, 0) } == 0 {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

// FLAKE: 2026-11-14, timed out waiting for the shell's directory; the typed pwd landed in the sh prompt under load
// FLAKE: 2026-11-14, failed once under a parallel release build (which step is unknown)
#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run");
    let cwd = tmp.path().join("project-dir");
    std::fs::create_dir(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    jtui::socket::prepare_dir(&dir).unwrap();
    // A leftover socket nobody listens on.
    drop(std::os::unix::net::UnixListener::bind(session::socket_path(&dir, "work")).unwrap());

    let sh_exe = sh_pinned_exe(tmp.path());
    let (d, c, exe) = (dir.clone(), cwd.clone(), sh_exe.clone());
    let started = tokio::task::spawn_blocking(move || crate::cli::start(&exe, &d, Some("work"), Some(&c)))
        .await
        .unwrap();
    let (name, socket) = started.unwrap_or_else(|e| {
        eprintln!("FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: first start failed: {e:?}; socket exists {}", session::socket_path(&dir, "work").exists());
        panic!("first start failed: {e:?}")
    });
    let _work = StartedServer::new(&socket);
    assert_eq!(name, "work");
    let c = Attached::start(&socket, 20, 80).await;
    c.type_text("pwd\r");
    let want = cwd.display().to_string();
    let (begun, deadline) = (Instant::now(), Instant::now() + WAIT);
    while !c.rows().iter().any(|l| l.trim() == want) {
        if Instant::now() >= deadline {
            eprintln!(
                "FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: \
                 no row equal to {want:?} after {:?}; screen:\n{}",
                begun.elapsed(),
                c.text()
            );
            panic!("timed out waiting for the shell's directory:\n{}", c.text());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The same name again is refused and leaves the live session alone.
    let (d, exe) = (dir.clone(), sh_exe.clone());
    let again = tokio::task::spawn_blocking(move || crate::cli::start(&exe, &d, Some("work"), None))
        .await
        .unwrap();
    if again.as_ref().err().map(String::as_str) != Some("jmux: session work already exists") {
        eprintln!(
            "FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: second start of `work` gave {again:?}, wanted the refusal"
        );
    }
    assert_eq!(again.unwrap_err(), "jmux: session work already exists");
    // An unnamed start takes the next free number.
    let (d, exe) = (dir.clone(), sh_exe.clone());
    let numbered = tokio::task::spawn_blocking(move || crate::cli::start(&exe, &d, None, None))
        .await
        .unwrap();
    let (second, second_socket) = numbered.unwrap_or_else(|e| {
        eprintln!("FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: unnamed start failed: {e:?}");
        panic!("unnamed start failed: {e:?}")
    });
    let _second = StartedServer::new(&second_socket);
    assert_ne!(second, "work");
    // `ls` lists both, one line each; the attached one says so.
    let d = dir.clone();
    let lines = tokio::task::spawn_blocking(move || crate::cli::ls(&d)).await.unwrap();
    let dump = format!(
        "ls gave {lines:?}; second {second:?}; sockets {:?}",
        std::fs::read_dir(&dir).map(|r| r.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
    );
    if lines.len() != 2 {
        eprintln!("FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: {dump}");
    }
    assert_eq!(lines.len(), 2, "{dump}");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("work: ") && l.ends_with(", attached")),
        "{dump}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with(&format!("{second}: ")) && l.ends_with(", detached")),
        "{dump}"
    );
    // Kill the detached one: no client is involved, its socket goes.
    let (d, s2) = (dir.clone(), second.clone());
    let killed = tokio::task::spawn_blocking(move || crate::cli::kill(&d, &s2))
        .await
        .unwrap();
    if killed.as_ref().ok() != Some(&format!("[session {second} killed]")) {
        eprintln!(
            "FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: kill of {second} gave {killed:?}"
        );
    }
    assert_eq!(killed.unwrap(), format!("[session {second} killed]"));
    let deadline = Instant::now() + WAIT;
    while session::socket_path(&dir, &second).exists() {
        if Instant::now() >= deadline {
            eprintln!(
                "FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: the killed session {second}'s socket stayed after {WAIT:?}"
            );
            panic!("the killed session's socket stayed");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The attached client was not taken over by the queries and the kill above.
    c.type_text("exit\r");
    let outcome = c.outcome().await;
    if outcome != Outcome::Bye(ByeReason::SessionEnded) {
        eprintln!(
            "FLAKE jmux_server_new_replaces_a_stale_socket_keeps_the_cwd_and_refuses_a_live_name: attached client ended with {outcome:?}, wanted SessionEnded"
        );
    }
    assert_eq!(outcome, Outcome::Bye(ByeReason::SessionEnded));
}

// FLAKE: 2026-11-14, `ls` listed `dead: Connection reset by peer (os error 104)` instead of removing the
// socket (seen once in a full run); suspected cause: the closed listener's socket answers with ECONNRESET,
// which `remove_stale` doesn't count as stale
#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_ls_removes_stale_sockets_and_attach_ignores_them() {
    let began = Instant::now();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run");
    jtui::socket::prepare_dir(&dir).unwrap();
    let stale = || drop(std::os::unix::net::UnixListener::bind(session::socket_path(&dir, "dead")).unwrap());
    stale();
    let d = dir.clone();
    let found = tokio::task::spawn_blocking(move || crate::cli::find(&d, Some("dead")))
        .await
        .unwrap();
    assert_eq!(found.unwrap_err(), "jmux: no session dead");
    stale();
    let d = dir.clone();
    let lines = tokio::task::spawn_blocking(move || crate::cli::ls(&d)).await.unwrap();
    if lines != ["no sessions".to_string()] {
        let path = session::socket_path(&dir, "dead");
        eprintln!(
            "FLAKE jmux_server_ls_removes_stale_sockets_and_attach_ignores_them: ls gave {lines:?}; socket {path:?} \
             exists {}, connect now {:?}; the listener was dropped (closed, never killed) right before the second `ls`, \
             about {:?} after the test began",
            path.exists(),
            std::os::unix::net::UnixStream::connect(&path).map(|_| ()),
            began.elapsed()
        );
    }
    assert_eq!(lines, vec!["no sessions".to_string()]);
    assert!(!session::socket_path(&dir, "dead").exists());
}
