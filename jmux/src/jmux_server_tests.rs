//! End-to-end tests for jmux_server-b (the server, part of the task's "done when"): a real server
//! (its own thread and runtime, a real `/bin/sh` on a PTY) in a temp socket dir, driven by a test
//! client over the socket that feeds the server's output into a `vt100` screen. Covers: the shell
//! shown above the status bar, keys and paste reaching it, resize, take-over keeping the screen,
//! detach keeping the server, the session ending with the shell (socket removed), a version
//! mismatch, stale and live sockets. Every wait is a deadline loop; nothing sleeps for a fixed time.

use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tokio::net::UnixStream;

use crate::app::Config;
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
    c.wait_text("29 90").await;
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

#[tokio::test(flavor = "multi_thread")]
async fn jmux_server_replaces_a_stale_socket_but_not_a_live_one() {
    let mut server = TestServer::start();
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
    let c = Client::attach(&server.socket, 20, 60).await;
    c.end(&mut server).await;

    // A leftover file nobody listens on is replaced.
    drop(std::os::unix::net::UnixListener::bind(&server.socket).unwrap());
    assert!(server.socket.exists());
    server.thread = Some(spawn_thread(server.dir.clone()));
    server::wait_for_socket(&server.socket, WAIT, || true).unwrap();
    let c = Client::attach(&server.socket, 20, 60).await;
    c.end(&mut server).await;
}
