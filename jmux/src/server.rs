//! The server process: one per session, detached from the terminal that started it.
//!
//! [`spawn_server`] (what `jmux new` calls) re-executes the binary as `jmux server --session
//! <name> --socket-dir <dir>` in a new session, stdio closed, working directory kept for the first
//! shell, and waits for the socket. [`run`] is that process: it listens on `<dir>/<name>.sock`
//! and drives the [`App`]; each connection is a reader task (messages become `AppEvent`s) and a
//! writer task fed by a channel, so the app never waits for a slow client. The socket file is
//! removed on every way out (a guard, plus after the loop).

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use jtui::stream_backend::StreamBackend;
use ratatui::Terminal;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

use crate::app::{App, AppEvent, Config};
use crate::protocol::{ClientMsg, ServerMsg, read_msg, write_all_msg};
use crate::session;

/// How long `spawn_server` waits for the socket.
pub const START_TIMEOUT: Duration = Duration::from_secs(5);
const START_POLL: Duration = Duration::from_millis(10);
/// How long the end of a session waits for the last messages to reach the client.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// The arguments `jmux` re-executes itself with.
pub fn server_args(name: &str, socket_dir: &Path) -> Vec<OsString> {
    vec![
        "server".into(),
        "--session".into(),
        name.into(),
        "--socket-dir".into(),
        socket_dir.into(),
    ]
}

/// Starts the server of session `name` as a detached process of `exe` (new session id, stdio
/// closed, the current directory kept for the first shell) and returns once its socket accepts
/// connections. An error if the process exits first or the socket doesn't appear in
/// [`START_TIMEOUT`]. Forks nothing else: the server's runtime starts in the new process.
///
/// Blocking: call from spawn_blocking
pub fn spawn_server(exe: &Path, socket_dir: &Path, name: &str) -> io::Result<()> {
    let mut command = Command::new(exe);
    command
        .args(server_args(name, socket_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and the closure does nothing else.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let socket = session::socket_path(socket_dir, name);
    wait_for_socket(&socket, START_TIMEOUT, || matches!(child.try_wait(), Ok(None)))
}

/// Polls until `socket` accepts a connection. `alive` says whether the server process still
/// runs (an early exit is an error at once).
///
/// Blocking: call from spawn_blocking
pub fn wait_for_socket(socket: &Path, timeout: Duration, mut alive: impl FnMut() -> bool) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        if !alive() {
            return Err(io::Error::other("the server exited before it was ready (see its log)"));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the server did not start in time",
            ));
        }
        std::thread::sleep(START_POLL);
    }
}

/// Removes the socket file when dropped.
struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        jtui::socket::remove_socket(&self.0);
    }
}

/// Binds the session's socket in `dir`: an error if a live server has it, a stale file is replaced.
fn bind(dir: &Path, name: &str) -> io::Result<(UnixListener, SocketGuard)> {
    jtui::socket::prepare_dir(dir)?;
    let path = session::socket_path(dir, name);
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("session {name} is already running"),
        ));
    }
    jtui::socket::remove_socket(&path);
    let listener = UnixListener::bind(&path)?;
    Ok((listener, SocketGuard(path)))
}

/// Runs the session until its shell ends. Needs a tokio runtime.
pub async fn run(socket_dir: &Path, mut config: Config) -> io::Result<()> {
    let (listener, _guard) = bind(socket_dir, &config.name)?;
    config.socket = session::socket_path(socket_dir, &config.name);
    let (events, mut rx) = mpsc::unbounded_channel();
    let accept = tokio::spawn(accept_loop(listener, events.clone()));
    let mut app = App::new(config, events);
    let mut terminal = Terminal::new(StreamBackend::new(80, 24))?;
    app.start();
    let result = loop {
        if app.needs_redraw
            && let Err(e) = app.redraw(&mut terminal)
        {
            break Err(e);
        }
        if app.should_quit {
            break Ok(());
        }
        let Some(event) = rx.recv().await else {
            break Ok(());
        };
        app.handle(event);
        // Whatever else is queued becomes the same frame.
        while let Ok(event) = rx.try_recv() {
            app.handle(event);
        }
        if app.should_quit {
            break Ok(());
        }
    };
    accept.abort();
    if let Err(e) = &result {
        tracing::error!(error = %e, "server failed");
    }
    // The shell dies with the app; the writers get the time to deliver `Bye`.
    let writers = app.shutdown();
    let _ = tokio::time::timeout(FLUSH_TIMEOUT, async {
        for writer in writers {
            let _ = writer.await;
        }
    })
    .await;
    drop(_guard);
    result
}

async fn accept_loop(listener: UnixListener, events: mpsc::UnboundedSender<AppEvent>) {
    static NEXT_CONN: AtomicU64 = AtomicU64::new(1);
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
                connection(conn, stream, events.clone());
            }
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                break;
            }
        }
    }
}

/// Starts a connection's tasks: the writer (ends once the app drops its sender and the queue is
/// sent, or a write fails) and the reader (ends at EOF, a bad frame or the writer's end).
fn connection(conn: u64, stream: UnixStream, events: mpsc::UnboundedSender<AppEvent>) {
    let (mut read, mut write) = stream.into_split();
    let (tx, mut out) = mpsc::channel::<ServerMsg>(crate::app::OUT_QUEUE);
    let (done, mut done_rx) = oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out.recv().await {
            if let Err(e) = write_all_msg(&mut write, &msg).await {
                tracing::debug!(conn, error = %e, "write failed");
                break;
            }
        }
        let _ = write.shutdown().await;
        let _ = done.send(());
    });
    if events.send(AppEvent::Connected { conn, tx, writer }).is_err() {
        return;
    }
    tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                msg = read_msg::<ClientMsg, _>(&mut read) => msg,
                _ = &mut done_rx => break,
            };
            match msg {
                Ok(Some(msg)) => {
                    if events.send(AppEvent::Client { conn, msg }).is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(conn, error = %e, "bad message");
                    break;
                }
            }
        }
        let _ = events.send(AppEvent::Closed { conn });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_is_started_with_its_session_and_socket_dir() {
        let args = server_args("work", Path::new("/run/jmux"));
        let args: Vec<_> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["server", "--session", "work", "--socket-dir", "/run/jmux"]);
    }

    #[test]
    fn waiting_for_a_socket_gives_up_when_the_process_died() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("x.sock");
        let err = wait_for_socket(&socket, Duration::from_secs(5), || false).unwrap_err();
        assert!(err.to_string().contains("exited"), "{err}");
    }

    #[test]
    fn waiting_for_a_socket_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("x.sock");
        let err = wait_for_socket(&socket, Duration::from_millis(50), || true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn waiting_for_a_socket_returns_once_it_listens() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("x.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        wait_for_socket(&socket, Duration::from_secs(5), || true).unwrap();
    }
}
