//! Talking to a running jed over a Unix socket (PLAN.md, Git editor, "From a task terminal").
//!
//! Protocol: one JSON line each way, any number of requests per connection, answered in order.
//! The client sends a [`Request`] (`{"open": "/abs/path", "line": 3, "col": 1, "wait": true}`);
//! the server answers with one [`Reply`] line: `{"closed": true}` once the file's last tab is
//! closed (with `wait`), or `{"opened": true}` at once (without). A connection closed with no
//! reply means jed quit first. A request may carry a [`Query`] instead of `open` (the answer is
//! in the reply's `data`, or its `error`), or a [`LogLine`] (a log-only line, acked with `{}`).
//! Old requests (`open` alone) decode and behave as before.
//!
//! The server side (`Server`, tokio) hands each request to a callback together with a
//! [`Responder`]; the app answers it when it is time, or drops it to close the connection. The
//! client side ([`client::open`], [`client::Connection`]) is blocking std I/O for `main`, before
//! logging, the runtime and raw mode.
//!
//! The socket is `<socket_dir>/<pid>.sock` in a `0700` directory ([`prepare_dir`]); [`Server`]
//! removes it on drop and [`remove_socket`] does the same from the panic hook.

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
/// Largest reply line the server sends: a bigger answer is replaced by an error (the builders in
/// `app/claude.rs` cap their data well below it).
pub const MAX_REPLY: usize = 256 * 1024;
/// Longest reply line the client reads.
const MAX_REPLY_LINE: u64 = 1024 * 1024;

/// Name of the environment variable task processes find the socket path in.
pub const ENV: &str = "JED_IPC";
/// The number of the terminal (`tasks::RunId`) a process runs in, set next to `JED_IPC`.
pub const TERMINAL_ENV: &str = "JED_TERMINAL";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Request {
    /// Absolute path of the file to open (empty in a query or a log line).
    #[serde(default, skip_serializing_if = "is_empty_path")]
    pub open: PathBuf,
    /// 1-based position, if given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
    /// Answer only when the buffer's last tab closes.
    #[serde(default)]
    pub wait: bool,
    /// Ask jed about its state instead of opening a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<Query>,
    /// A line for jed's logs; nothing is answered but an ack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LogLine>,
    /// The caller's `JED_TERMINAL` (the number of the terminal it runs in), so the terminals
    /// list can flag it as `self`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<u64>,
    /// The caller's process id (a log line's `[mcp <pid>]` in the Tools view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Which connection it came from, set by the server (never on the wire; 0 for a request
    /// that didn't come through [`Server`]).
    #[serde(skip)]
    pub conn: u64,
}

impl Request {
    /// A query from the terminal numbered `terminal`.
    pub fn query(query: Query, terminal: Option<u64>) -> Request {
        Request {
            query: Some(query),
            terminal,
            ..Request::default()
        }
    }

    /// Asks for something: a file to open, a query or a log line.
    fn is_valid(&self) -> bool {
        !self.open.as_os_str().is_empty() || self.query.is_some() || self.log.is_some()
    }
}

fn is_empty_path(p: &Path) -> bool {
    p.as_os_str().is_empty()
}

/// What a client can ask jed. Terminals and commands are named by their numbers
/// (`tasks::RunId`, `CommandRecord::id`), as the `terminals` / `commands` answers list them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Query {
    /// The active editor: file, language, cursor, the primary selection and its text.
    Context,
    /// Every group's tabs.
    Editors,
    /// Diagnostics of one open file (`path`), else of every open file, errors first.
    Diagnostics {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
    /// Shell tabs and task runs.
    Terminals,
    /// The last `n` commands (default 10) of one terminal, else of every shell but the caller's.
    Commands {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        n: Option<usize>,
    },
    /// One command's output, at most `max_lines` (default 200) of its last lines.
    Output {
        terminal: u64,
        command: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<usize>,
    },
    /// A terminal's visible screen.
    Screen { terminal: u64 },
    /// Version, workspace, git, open files, servers, tasks, shells.
    Environment,
    /// Open a file in the active group, as quick open does.
    Open {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        col: Option<usize>,
    },
}

/// A line for the logs: `dir` is "in" or "out" for a protocol message, `msg` the message
/// (`ms`: how long the client took to answer, on "out"), or "hook" for what a hook returned
/// (`msg` a string). The server itself sends [`CLOSED`] for a connection that sent log lines
/// and is gone (never on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub dir: String,
    pub msg: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
}

/// `LogLine::dir` of the server's note that a logging connection closed.
pub const CLOSED: &str = "closed";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reply {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,
    /// A query's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Why a query got no answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    /// The wait is over: the file's last tab closed.
    pub fn closed() -> Reply {
        Reply {
            closed: true,
            ..Reply::default()
        }
    }

    /// Opened, not waiting.
    pub fn opened() -> Reply {
        Reply {
            opened: true,
            ..Reply::default()
        }
    }

    /// A query's answer.
    pub fn data(value: serde_json::Value) -> Reply {
        Reply {
            data: Some(value),
            ..Reply::default()
        }
    }

    /// A query that can't be answered.
    pub fn error(why: impl Into<String>) -> Reply {
        Reply {
            error: Some(why.into()),
            ..Reply::default()
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
    static NEXT_CONN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let conn = NEXT_CONN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (mut pid, mut logged) = (None, false);
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        // `take` caps each line; an overlong one has no newline and ends the connection.
        match tokio::io::AsyncReadExt::take(&mut reader, MAX_LINE)
            .read_line(&mut line)
            .await
        {
            Ok(n) if n > 0 && line.ends_with('\n') => {}
            // The client is done (or never sent a whole line).
            other => {
                tracing::debug!(?other, "ipc: no complete request line");
                break;
            }
        }
        let request: Request = match decode(&line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "ipc: bad request");
                break;
            }
        };
        if !request.is_valid() {
            tracing::warn!("ipc: request asks for nothing");
            break;
        }
        let mut request = request;
        request.conn = conn;
        if request.log.is_some() {
            (logged, pid) = (true, request.pid.or(pid));
        }
        let (tx, rx) = oneshot::channel();
        on_request(request, Responder(tx));
        // A dropped responder closes the connection.
        let Ok(reply) = rx.await else { break };
        let mut out = encode(&reply);
        if out.len() > MAX_REPLY {
            tracing::warn!(bytes = out.len(), "ipc: reply too large");
            out = encode(&Reply::error("reply too large"));
        }
        if write.write_all(out.as_bytes()).await.is_err() {
            break;
        }
    }
    let _ = write.shutdown().await;
    if logged {
        // Tell the app the logging client is gone (nobody reads this answer).
        let log = LogLine {
            dir: CLOSED.into(),
            msg: serde_json::Value::Null,
            ms: None,
        };
        let (tx, _rx) = oneshot::channel();
        let gone = Request {
            log: Some(log),
            pid,
            conn,
            ..Request::default()
        };
        on_request(gone, Responder(tx));
    }
}

pub mod client {
    //! The blocking client, for `main` before logging, the runtime and raw mode.

    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
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

    /// One connection for many requests: connect once, then [`Connection::request`] per query.
    pub struct Connection {
        stream: std::os::unix::net::UnixStream,
        reader: BufReader<std::os::unix::net::UnixStream>,
    }

    impl Connection {
        /// Blocking: connects to the jed listening on `socket`; `timeout` bounds each request's
        /// wait for its answer (`None`: as long as jed takes).
        pub fn connect(socket: &Path, timeout: Option<Duration>) -> io::Result<Connection> {
            let stream = std::os::unix::net::UnixStream::connect(socket)?;
            stream.set_read_timeout(timeout)?;
            let reader = BufReader::new(stream.try_clone()?);
            Ok(Connection { stream, reader })
        }

        /// Blocking: sends `request` and reads its answer. An error (closed, timed out, a reply
        /// that isn't one) leaves the connection unusable.
        pub fn request(&mut self, request: &Request) -> io::Result<Reply> {
            self.stream.write_all(encode(request).as_bytes())?;
            let mut line = String::new();
            let n = (&mut self.reader).take(MAX_REPLY_LINE).read_line(&mut line)?;
            if n == 0 || !line.ends_with('\n') {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "jed closed the connection",
                ));
            }
            decode(&line).map_err(io::Error::other)
        }
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
            ..Request::default()
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
            ..Request::default()
        };
        assert_eq!(encode(&bare), "{\"open\":\"/x\",\"wait\":false}\n");
        assert_eq!(decode::<Request>("{\"open\":\"/x\"}").unwrap(), bare);
        assert_eq!(encode(&Reply::closed()), "{\"closed\":true}\n");
        assert_eq!(encode(&Reply::opened()), "{\"opened\":true}\n");
    }

    #[test]
    fn queries_and_log_lines_round_trip_and_old_replies_still_decode() {
        let q = Request::query(
            Query::Output {
                terminal: 2,
                command: 14,
                max_lines: None,
            },
            Some(2),
        );
        assert_eq!(
            encode(&q),
            "{\"wait\":false,\"query\":{\"type\":\"output\",\"terminal\":2,\"command\":14},\"terminal\":2}\n"
        );
        assert_eq!(decode::<Request>(&encode(&q)).unwrap(), q);
        assert!(q.is_valid());
        let q: Request = decode("{\"query\":{\"type\":\"commands\"}}").unwrap();
        assert_eq!(
            q.query,
            Some(Query::Commands {
                terminal: None,
                n: None
            })
        );
        let log: Request = decode("{\"log\":{\"dir\":\"in\",\"msg\":{\"a\":1}}}").unwrap();
        assert!(log.is_valid() && log.open.as_os_str().is_empty());
        // An old client's reply line has no data / error; a query's has no `opened`.
        assert_eq!(decode::<Reply>("{\"opened\":true}").unwrap(), Reply::opened());
        assert_eq!(
            encode(&Reply::data(serde_json::json!({"a": 1}))),
            "{\"data\":{\"a\":1}}\n"
        );
        assert_eq!(encode(&Reply::default()), "{}\n");
    }

    #[test]
    fn decode_ignores_unknown_fields_and_rejects_bad_lines() {
        let r: Request = decode("{\"open\":\"/x\",\"future\":[1],\"wait\":true}\n").unwrap();
        assert!(r.wait);
        assert!(decode::<Request>("not json").is_err());
        // Decodes, but asks for nothing: the server refuses it.
        assert!(!decode::<Request>("{\"wait\":true}").unwrap().is_valid());
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
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].conn > 0, "the server numbers connections");
        assert_eq!(
            Request {
                conn: 0,
                ..seen[0].clone()
            },
            request(true)
        );
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
