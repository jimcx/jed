//! Talking to a running jed over a Unix socket (PLAN.md, Git editor, "From a task terminal").
//!
//! Protocol: one JSON line each way. The client sends a [`Request`]
//! (`{"open": "/abs/path", "line": 3, "col": 1, "wait": true}`); the server answers with one
//! [`Reply`] line: `{"closed": true}` once the file's last tab is closed (with `wait`), or
//! `{"opened": true}` at once (without). A connection closed with no reply means jed quit first.
//!
//! The server side (`Server`, tokio) hands each request to a callback together with a
//! [`Responder`]; the app answers it when it is time, or drops it to close the connection. The
//! client side ([`client::open`]) is blocking std I/O for `main`, before logging, the runtime
//! and raw mode.
//!
//! The socket is `<socket_dir>/<pid>.sock` in a `0700` directory ([`prepare_dir`]); [`Server`]
//! removes it on drop and [`remove_socket`] does the same from the panic hook.

// The server half is wired into `App` by git_editor-f; until then only tests use it.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

/// Longest request line the server reads.
const MAX_LINE: u64 = 64 * 1024;

/// Name of the environment variable task processes find the socket path in.
pub const ENV: &str = "JED_IPC";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Absolute path of the file to open.
    pub open: PathBuf,
    /// 1-based position, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
    /// Answer only when the buffer's last tab closes.
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,
}

impl Reply {
    /// The wait is over: the file's last tab closed.
    pub fn closed() -> Reply {
        Reply {
            closed: true,
            opened: false,
        }
    }

    /// Opened, not waiting.
    pub fn opened() -> Reply {
        Reply {
            closed: false,
            opened: true,
        }
    }
}

/// `value` as one line of JSON, with the trailing newline.
pub fn encode<T: Serialize>(value: &T) -> String {
    let mut line = serde_json::to_string(value).unwrap_or_default();
    line.push('\n');
    line
}

/// One line of JSON; unknown fields are ignored.
pub fn decode<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, String> {
    serde_json::from_str(line.trim()).map_err(|e| e.to_string())
}

/// The socket directory from the environment's values: `$XDG_RUNTIME_DIR/jed` (absolute only),
/// else `$TMPDIR/jed-<uid>`, else `/tmp/jed-<uid>`.
pub fn socket_dir(xdg_runtime_dir: Option<&OsStr>, tmpdir: Option<&OsStr>, uid: u32) -> PathBuf {
    let absolute = |v: Option<&OsStr>| {
        v.filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(xdg) = absolute(xdg_runtime_dir) {
        return xdg.join("jed");
    }
    let base = absolute(tmpdir).unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(format!("jed-{uid}"))
}

/// [`socket_dir`] for this process's environment and user.
pub fn default_socket_dir() -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    socket_dir(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        std::env::var_os("TMPDIR").as_deref(),
        uid,
    )
}

/// The socket for process `pid` in `dir`.
pub fn socket_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.sock"))
}

/// Create `dir` with mode `0700`, or check an existing one is ours and closed to others.
///
/// Blocking: call from spawn_blocking
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() {
        return Err(io::Error::other(format!("{} is not a directory", dir.display())));
    }
    if meta.uid() != uid {
        return Err(io::Error::other(format!("{} is owned by another user", dir.display())));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(io::Error::other(format!("{} is accessible by others", dir.display())));
    }
    Ok(())
}

/// Delete the socket file; never panics, ignores errors (also for the panic hook).
pub fn remove_socket(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Answers one request: send a [`Reply`], or drop it to close the connection unanswered.
#[derive(Debug)]
pub struct Responder(oneshot::Sender<Reply>);

impl Responder {
    pub fn answer(self, reply: Reply) {
        let _ = self.0.send(reply);
    }
}

/// The listening socket; the file is removed and the accept loop stopped on drop.
pub struct Server {
    path: PathBuf,
    accept: tokio::task::JoinHandle<()>,
}

impl Server {
    /// Listen on `path` (a stale file there is replaced) and call `on_request` for every
    /// valid request, from the connection's task. Needs a tokio runtime; the directory must
    /// exist (see [`prepare_dir`]).
    pub fn start(path: &Path, on_request: impl Fn(Request, Responder) + Send + Sync + 'static) -> io::Result<Server> {
        remove_socket(path);
        let listener = UnixListener::bind(path)?;
        let on_request = std::sync::Arc::new(on_request);
        let accept = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let on_request = on_request.clone();
                        tokio::spawn(async move { serve_connection(stream, &*on_request).await });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "ipc accept failed");
                        break;
                    }
                }
            }
        });
        Ok(Server {
            path: path.to_path_buf(),
            accept,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.accept.abort();
        remove_socket(&self.path);
    }
}

async fn serve_connection(stream: UnixStream, on_request: &(dyn Fn(Request, Responder) + Send + Sync)) {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    // `take` caps the line; an overlong one has no newline and fails to decode.
    let mut reader = BufReader::new(tokio::io::AsyncReadExt::take(read, MAX_LINE));
    match reader.read_line(&mut line).await {
        Ok(n) if n > 0 && line.ends_with('\n') => {}
        other => {
            tracing::debug!(?other, "ipc: no complete request line");
            return;
        }
    }
    let request: Request = match decode(&line) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "ipc: bad request");
            return;
        }
    };
    let (tx, rx) = oneshot::channel();
    on_request(request, Responder(tx));
    // A dropped responder closes the connection.
    if let Ok(reply) = rx.await {
        let _ = write.write_all(encode(&reply).as_bytes()).await;
        let _ = write.shutdown().await;
    }
}

pub mod client {
    //! The blocking client, for `main` before logging, the runtime and raw mode.

    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;

    #[derive(Debug, PartialEq, Eq)]
    pub enum Outcome {
        /// The running jed answered (0) or went away first (1): exit with this code.
        Done(u8),
        /// Could not connect (jed gone, not a socket, not ours): start jed normally.
        Fallback(String),
    }

    /// How long a request without `wait` waits for its (immediate) answer before giving up
    /// with exit status 1: jed has it, so starting another would open the file twice.
    pub const NO_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

    /// Send `request` to the jed listening on `socket` and wait for its answer.
    ///
    /// Blocking: a `wait` request waits as long as the server does (it answers when the tab
    /// closes); one without gives up after [`NO_WAIT_TIMEOUT`].
    pub fn open(socket: &Path, request: &Request) -> Outcome {
        open_with(socket, request, NO_WAIT_TIMEOUT)
    }

    pub(super) fn open_with(socket: &Path, request: &Request, no_wait_timeout: Duration) -> Outcome {
        let mut stream = match std::os::unix::net::UnixStream::connect(socket) {
            Ok(s) => s,
            Err(e) => return Outcome::Fallback(format!("{}: {e}", socket.display())),
        };
        if !request.wait && stream.set_read_timeout(Some(no_wait_timeout)).is_err() {
            return Outcome::Done(1);
        }
        if stream.write_all(encode(request).as_bytes()).is_err() {
            return Outcome::Done(1);
        }
        let mut line = String::new();
        match BufReader::new(&stream).read_line(&mut line) {
            Ok(n) if n > 0 && decode::<Reply>(&line).is_ok() => Outcome::Done(0),
            _ => Outcome::Done(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::client::{Outcome, open};
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn request(wait: bool) -> Request {
        Request {
            open: "/tmp/a.rs".into(),
            line: Some(3),
            col: Some(2),
            wait,
        }
    }

    #[test]
    fn encode_decode_round_trip() {
        let r = request(true);
        let line = encode(&r);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        assert_eq!(decode::<Request>(&line).unwrap(), r);
        let bare = Request {
            open: "/x".into(),
            line: None,
            col: None,
            wait: false,
        };
        assert_eq!(encode(&bare), "{\"open\":\"/x\",\"wait\":false}\n");
        assert_eq!(decode::<Request>("{\"open\":\"/x\"}").unwrap(), bare);
        assert_eq!(encode(&Reply::closed()), "{\"closed\":true}\n");
        assert_eq!(encode(&Reply::opened()), "{\"opened\":true}\n");
    }

    #[test]
    fn decode_ignores_unknown_fields_and_rejects_bad_lines() {
        let r: Request = decode("{\"open\":\"/x\",\"future\":[1],\"wait\":true}\n").unwrap();
        assert!(r.wait);
        assert!(decode::<Request>("not json").is_err());
        assert!(decode::<Request>("{\"wait\":true}").is_err());
        assert!(decode::<Request>("").is_err());
    }

    #[test]
    fn socket_dir_choices() {
        let s = |v: &'static str| Some(OsStr::new(v));
        assert_eq!(
            socket_dir(s("/run/user/1"), s("/t"), 5),
            PathBuf::from("/run/user/1/jed")
        );
        assert_eq!(socket_dir(s("rel"), s("/var/t"), 5), PathBuf::from("/var/t/jed-5"));
        assert_eq!(socket_dir(s(""), s("/var/t"), 5), PathBuf::from("/var/t/jed-5"));
        assert_eq!(socket_dir(None, None, 5), PathBuf::from("/tmp/jed-5"));
        assert_eq!(socket_dir(None, s("rel"), 5), PathBuf::from("/tmp/jed-5"));
        assert_eq!(socket_path(Path::new("/d"), 42), PathBuf::from("/d/42.sock"));
    }

    #[test]
    fn prepare_dir_creates_private_and_refuses_open() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a").join("jed");
        prepare_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        prepare_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_dir(&dir).is_err());
        let file = tmp.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(prepare_dir(&file).is_err());
        // A symlink to a private directory of ours is still refused (someone could repoint it).
        let private = tmp.path().join("private");
        prepare_dir(&private).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        let err = prepare_dir(&link).unwrap_err();
        assert!(err.to_string().contains("not a directory"), "{err}");
    }

    type Seen = Arc<Mutex<Vec<Request>>>;

    /// A server whose responder answers per `behaviour`: None drops it, Some replies.
    fn serve(path: &Path, behaviour: Option<Reply>) -> (Server, Seen) {
        let seen: Seen = Arc::default();
        let sink = seen.clone();
        let server = Server::start(path, move |req, responder| {
            sink.lock().unwrap().push(req);
            if let Some(reply) = behaviour.clone() {
                responder.answer(reply);
            }
        })
        .unwrap();
        (server, seen)
    }

    async fn client(path: &Path, req: Request) -> Outcome {
        let path = path.to_path_buf();
        let task = tokio::task::spawn_blocking(move || open(&path, &req));
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("client hung")
            .unwrap()
    }

    #[tokio::test]
    async fn request_is_delivered_and_answered() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        let (_server, seen) = serve(&path, Some(Reply::closed()));
        assert_eq!(client(&path, request(true)).await, Outcome::Done(0));
        assert_eq!(*seen.lock().unwrap(), vec![request(true)]);
    }

    #[tokio::test]
    async fn without_wait_answers_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        let (_server, seen) = serve(&path, Some(Reply::opened()));
        assert_eq!(client(&path, request(false)).await, Outcome::Done(0));
        assert!(!seen.lock().unwrap()[0].wait);
    }

    #[tokio::test]
    async fn dropped_responder_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        let (_server, seen) = serve(&path, None);
        assert_eq!(client(&path, request(true)).await, Outcome::Done(1));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    /// A server that hands each request's responder to the test, unanswered.
    fn holding(path: &Path) -> (Server, tokio::sync::mpsc::UnboundedReceiver<(Request, Responder)>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let server = Server::start(path, move |req, r| {
            let _ = tx.send((req, r));
        })
        .unwrap();
        (server, rx)
    }

    async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<(Request, Responder)>) -> (Request, Responder) {
        tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("no request reached the server")
            .unwrap()
    }

    #[tokio::test]
    async fn answer_can_come_later_and_connections_are_concurrent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        let (_server, mut rx) = holding(&path);
        let mut a_req = request(true);
        a_req.open = "/tmp/a".into();
        let mut b_req = request(true);
        b_req.open = "/tmp/b".into();
        let a = tokio::spawn({
            let path = path.clone();
            async move { client(&path, a_req).await }
        });
        let b = tokio::spawn({
            let path = path.clone();
            async move { client(&path, b_req).await }
        });
        let (first, r1) = next(&mut rx).await;
        let (second, r2) = next(&mut rx).await;
        assert!(
            !a.is_finished() && !b.is_finished(),
            "both clients wait for their answers"
        );
        // Answer one, drop the other: each connection gets its own outcome.
        let (answered, dropped) = if first.open == Path::new("/tmp/a") {
            (r1, r2)
        } else {
            assert_eq!(second.open, Path::new("/tmp/a"));
            (r2, r1)
        };
        answered.answer(Reply::closed());
        assert_eq!(a.await.unwrap(), Outcome::Done(0));
        assert!(!b.is_finished(), "b is still waiting after a's answer");
        drop(dropped);
        assert_eq!(b.await.unwrap(), Outcome::Done(1));
    }

    #[tokio::test]
    async fn only_requests_without_wait_time_out() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        let (_server, mut rx) = holding(&path);
        let timeout = Duration::from_millis(100);
        let waiting = tokio::task::spawn_blocking({
            let path = path.clone();
            move || client::open_with(&path, &request(true), timeout)
        });
        let (_, held_wait) = next(&mut rx).await;
        // No wait, never answered: gives up with 1 (the server keeps the responder).
        let quick = tokio::task::spawn_blocking({
            let path = path.clone();
            move || client::open_with(&path, &request(false), timeout)
        });
        let (req, _held_quick) = next(&mut rx).await;
        assert!(!req.wait);
        let quick = tokio::time::timeout(Duration::from_secs(10), quick)
            .await
            .expect("no-wait client hung");
        assert_eq!(quick.unwrap(), Outcome::Done(1));
        // The waiting client has been waiting longer than the timeout by now and still gets its answer.
        assert!(!waiting.is_finished());
        held_wait.answer(Reply::closed());
        let waiting = tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .expect("client hung");
        assert_eq!(waiting.unwrap(), Outcome::Done(0));
    }

    #[tokio::test]
    async fn bad_request_closes_the_connection() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        let (_server, seen) = serve(&path, Some(Reply::closed()));
        let p = path.clone();
        let out = tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut s = std::os::unix::net::UnixStream::connect(p).unwrap();
            s.write_all(b"garbage\n").unwrap();
            let mut buf = String::new();
            s.read_to_string(&mut buf).unwrap();
            buf
        })
        .await
        .unwrap();
        assert_eq!(out, "");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn dead_sockets_fall_back() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.sock");
        assert!(matches!(open(&missing, &request(true)), Outcome::Fallback(_)));
        let dead = tmp.path().join("dead.sock");
        drop(std::os::unix::net::UnixListener::bind(&dead).unwrap());
        assert!(dead.exists());
        assert!(matches!(open(&dead, &request(true)), Outcome::Fallback(_)));
        let file = tmp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(open(&file, &request(true)), Outcome::Fallback(_)));
    }

    #[tokio::test]
    async fn socket_removed_on_drop_and_stale_file_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        let (server, _) = serve(&path, Some(Reply::closed()));
        assert_eq!(client(&path, request(true)).await, Outcome::Done(0));
        assert_eq!(server.path(), path);
        drop(server);
        assert!(!path.exists());
        remove_socket(&path);
    }
}
