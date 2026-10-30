//! kitty's remote control protocol, spoken over the socket kitty listens on (`listen_on`,
//! exported to children as `KITTY_LISTEN_ON`): no `kitten` binary, no tty round trip.
//! One request per connection: `ESC P @kitty-cmd <json> ESC \`, then read to EOF. Reference:
//! kitty's `docs/rc_protocol.rst` and `kitty/remote_control.py`. Passwords / encrypted requests
//! and `tcp:` / `fd:` addresses are not supported (the backend is then unavailable).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Backend, Env, Window, WindowRef};

/// Read / write limit for a whole call.
const TIMEOUT: Duration = Duration::from_secs(2);
/// Only has to be <= kitty's own version; the commands used exist since 0.14.
const VERSION: [u32; 3] = [0, 14, 2];
const PREFIX: &[u8] = b"\x1bP@kitty-cmd";
const SUFFIX: &[u8] = b"\x1b\\";

/// Where kitty listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// `unix:<path>`
    Path(PathBuf),
    /// `unix:@name` (Linux only)
    Abstract(String),
}

impl Address {
    /// `spec` as in `KITTY_LISTEN_ON`. `tcp:`, `fd:` and anything else is an error.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let Some(rest) = spec.strip_prefix("unix:") else {
            return Err(format!("unsupported kitty address {spec:?} (only unix: sockets)"));
        };
        if rest.is_empty() {
            return Err("empty kitty socket path".into());
        }
        if let Some(name) = rest.strip_prefix('@') {
            if !cfg!(target_os = "linux") {
                return Err("abstract kitty sockets exist only on Linux".into());
            }
            if name.is_empty() {
                return Err("empty abstract kitty socket name".into());
            }
            return Ok(Self::Abstract(name.to_string()));
        }
        Ok(Self::Path(PathBuf::from(rest)))
    }

    fn connect(&self) -> std::io::Result<UnixStream> {
        match self {
            Self::Path(p) => UnixStream::connect(p),
            #[cfg(target_os = "linux")]
            Self::Abstract(name) => {
                use std::os::linux::net::SocketAddrExt;
                let addr = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
                UnixStream::connect_addr(&addr)
            }
            #[cfg(not(target_os = "linux"))]
            Self::Abstract(_) => Err(std::io::Error::other("abstract sockets are Linux only")),
        }
    }
}

/// The backend when `env` names a kitty socket we can speak to.
pub fn detect(env: &Env) -> Option<Box<dyn Backend>> {
    let spec = env.kitty_listen_on.as_deref()?;
    match Address::parse(spec) {
        Ok(address) => Some(Box::new(Kitty { address })),
        Err(e) => {
            tracing::info!(error = %e, "kitty remote control unavailable");
            None
        }
    }
}

pub struct Kitty {
    pub address: Address,
}

/// The bytes of a request.
pub fn encode(cmd: &str, payload: Value) -> Vec<u8> {
    let request = json!({"cmd": cmd, "version": VERSION, "no_response": false, "payload": payload});
    let mut out = PREFIX.to_vec();
    out.extend_from_slice(request.to_string().as_bytes());
    out.extend_from_slice(SUFFIX);
    out
}

/// The `data` of a response (everything read), or kitty's error.
pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    let start = find(bytes, PREFIX).ok_or("no kitty response")? + PREFIX.len();
    let end = find(&bytes[start..], SUFFIX).ok_or("incomplete kitty response")? + start;
    let v: Value = serde_json::from_slice(&bytes[start..end]).map_err(|e| format!("bad kitty response: {e}"))?;
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(v.get("data").cloned().unwrap_or(Value::Null));
    }
    let error = v.get("error").and_then(Value::as_str).unwrap_or("unknown error");
    let lower = error.to_lowercase();
    let hint =
        if lower.contains("password") || lower.contains("allow_remote_control") || lower.contains("remote control") {
            " (remote control refused: set allow_remote_control socket-only, no password)"
        } else {
            ""
        };
    Err(format!("kitty: {error}{hint}"))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Reads to EOF within `deadline`.
fn read_all(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("kitty did not answer in time".into());
        }
        stream.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
        match stream.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err("kitty did not answer in time".into());
            }
            Err(e) => return Err(format!("reading from kitty: {e}")),
        }
    }
}

impl Kitty {
    /// Blocking: one command, the response's `data`.
    fn call(&self, cmd: &str, payload: Value) -> Result<Value, String> {
        let deadline = Instant::now() + TIMEOUT;
        let mut stream = self
            .address
            .connect()
            .map_err(|e| format!("can't connect to kitty: {e}"))?;
        stream.set_write_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
        stream
            .write_all(&encode(cmd, payload))
            .map_err(|e| format!("writing to kitty: {e}"))?;
        stream.shutdown(std::net::Shutdown::Write).map_err(|e| e.to_string())?;
        let bytes = read_all(&mut stream, deadline)?;
        decode(&bytes).map_err(|e| format!("{cmd}: {e}"))
    }
}

impl Backend for Kitty {
    fn name(&self) -> &str {
        "kitty"
    }

    fn windows(&self) -> Result<Vec<Window>, String> {
        let data = self.call("ls", json!({}))?;
        parse_ls(&data)
    }

    fn focus(&self, window: &WindowRef) -> Result<(), String> {
        self.call("focus-window", json!({"match": format!("id:{}", window.0)}))
            .map(|_| ())
    }

    fn launch(&self, workspace: &Path) -> Result<(), String> {
        let path = workspace.to_string_lossy();
        self.call(
            "launch",
            json!({"type": "os-window", "cwd": path, "args": ["jed", path]}),
        )
        .map(|_| ())
    }
}

/// The windows with a `jed_workspace` user var in `ls`'s `data` (a JSON string holding the
/// OS windows, their tabs and windows). The var is set by `terminal::Identity`; titles, cwds
/// and command lines are never matched.
pub fn parse_ls(data: &Value) -> Result<Vec<Window>, String> {
    let parsed;
    let list = match data {
        Value::String(s) => {
            parsed = serde_json::from_str::<Value>(s).map_err(|e| format!("bad ls output: {e}"))?;
            &parsed
        }
        other => other,
    };
    let os_windows = list.as_array().ok_or("ls output is not a list")?;
    let mut out = Vec::new();
    for window in os_windows
        .iter()
        .filter_map(|o| o.get("tabs")?.as_array())
        .flatten()
        .filter_map(|t| t.get("windows")?.as_array())
        .flatten()
    {
        let Some(id) = window.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(ws) = window.pointer("/user_vars/jed_workspace").and_then(Value::as_str) else {
            continue;
        };
        if ws.is_empty() {
            continue;
        }
        out.push(Window {
            workspace: PathBuf::from(ws),
            id: WindowRef(id.to_string()),
        });
    }
    Ok(out)
}

/// A fake kitty for tests (here and in `kitty_recent_tests`): a thread serving a unix socket.
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    /// A socket path short enough for macOS's 104-byte `sun_path`.
    pub fn short_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = PathBuf::from(format!(
            "/tmp/jk-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[derive(Clone, Copy)]
    pub enum Mode {
        Normal,
        /// `ok: false` to everything.
        Refuse,
        /// Reads the request and never answers.
        Silent,
        /// Reads the request and closes.
        Close,
    }

    /// The `ls` data: windows 10 (`/w/a`) and 20 (`/w/c d`) run jed.
    pub fn ls_json() -> Value {
        let list = json!([
            {"id": 1, "tabs": [
                {"id": 1, "windows": [
                    {"id": 10, "title": "jed", "user_vars": {"jed": "1", "jed_workspace": "/w/a"}},
                    {"id": 11, "title": "jed", "cwd": "/w/b", "user_vars": {}}]},
                {"id": 2, "windows": [{"id": 12, "title": "zsh"}]}]},
            {"id": 2, "tabs": [{"id": 3, "windows": [
                {"id": 20, "user_vars": {"jed_workspace": "/w/c d"}},
                {"id": 21, "user_vars": {"jed_workspace": 5}},
                {"id": 22, "user_vars": {"jed_workspace": ""}}]}]}
        ]);
        Value::String(list.to_string())
    }

    /// Serves `ls` with `ls` (a JSON string, like kitty), records every request.
    pub fn serve(mode: Mode, ls: Value) -> (PathBuf, Arc<Mutex<Vec<Value>>>) {
        let path = short_dir().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let log: Arc<Mutex<Vec<Value>>> = Arc::default();
        let l = log.clone();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let mut req = Vec::new();
                let _ = s.read_to_end(&mut req);
                let Some(v) = req
                    .strip_prefix(PREFIX)
                    .and_then(|r| r.strip_suffix(SUFFIX))
                    .and_then(|r| serde_json::from_slice::<Value>(r).ok())
                else {
                    continue;
                };
                let cmd = v["cmd"].as_str().unwrap_or("").to_string();
                l.lock().unwrap().push(v);
                let body = match (mode, cmd.as_str()) {
                    (Mode::Close, _) => continue,
                    (Mode::Silent, _) => {
                        held.push(s);
                        continue;
                    }
                    (Mode::Refuse, _) => json!({"ok": false, "error": "Remote control is disabled"}),
                    (_, "ls") => json!({"ok": true, "data": ls}),
                    _ => json!({"ok": true}),
                };
                let _ = s.write_all(&[PREFIX, body.to_string().as_bytes(), SUFFIX].concat());
            }
        });
        (path, log)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(Address::parse("unix:/tmp/k-1"), Ok(Address::Path("/tmp/k-1".into())));
        if cfg!(target_os = "linux") {
            assert_eq!(Address::parse("unix:@kitty-1"), Ok(Address::Abstract("kitty-1".into())));
        } else {
            assert!(Address::parse("unix:@kitty-1").is_err());
        }
        for bad in ["", "tcp:localhost:1234", "fd:3", "junk", "unix:", "unix:@", "/tmp/x"] {
            assert!(Address::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn requests_are_framed_like_kitty() {
        let bytes = encode("focus-window", json!({"match": "id:3"}));
        assert!(bytes.starts_with(b"\x1bP@kitty-cmd{") && bytes.ends_with(b"}\x1b\\"));
        let v: Value = serde_json::from_slice(&bytes[PREFIX.len()..bytes.len() - 2]).unwrap();
        assert_eq!(v["cmd"], "focus-window");
        assert_eq!(v["version"], json!([0, 14, 2]));
        assert_eq!(v["no_response"], false);
        assert_eq!(v["payload"], json!({"match": "id:3"}));
    }

    #[test]
    fn responses_decode() {
        let ok = b"\x1bP@kitty-cmd{\"ok\": true, \"data\": \"x\"}\x1b\\";
        assert_eq!(decode(ok), Ok(json!("x")));
        let mut trailing = ok.to_vec();
        trailing.extend_from_slice(b"junk");
        assert_eq!(decode(&trailing), Ok(json!("x")));
        assert_eq!(decode(b"\x1bP@kitty-cmd{\"ok\": true}\x1b\\"), Ok(Value::Null));
        let err = decode(b"\x1bP@kitty-cmd{\"ok\": false, \"error\": \"nope\"}\x1b\\").unwrap_err();
        assert_eq!(err, "kitty: nope");
        let err =
            decode(b"\x1bP@kitty-cmd{\"ok\": false, \"error\": \"Remote control is disabled\"}\x1b\\").unwrap_err();
        assert!(err.contains("remote control refused"), "{err}");
        assert!(decode(b"").is_err());
        assert!(decode(&ok[..ok.len() - 2]).unwrap_err().contains("incomplete"));
        assert!(
            decode(b"\x1bP@kitty-cmd{nope}\x1b\\")
                .unwrap_err()
                .contains("bad kitty response")
        );
    }

    #[test]
    fn a_response_split_across_reads_is_assembled() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            for chunk in [&b"\x1bP@kitty-"[..], b"cmd{\"ok\": tr", b"ue, \"data\": 1}\x1b", b"\\"] {
                a.write_all(chunk).unwrap();
                a.flush().unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let bytes = read_all(&mut b, Instant::now() + Duration::from_secs(2)).unwrap();
        t.join().unwrap();
        assert_eq!(decode(&bytes), Ok(json!(1)));
    }

    #[test]
    fn ls_maps_windows_with_the_workspace_var_only() {
        let w = |p: &str, id: &str| Window {
            workspace: p.into(),
            id: WindowRef(id.into()),
        };
        assert_eq!(parse_ls(&ls_json()), Ok(vec![w("/w/a", "10"), w("/w/c d", "20")]));
        assert!(parse_ls(&json!("not json")).is_err());
        assert!(parse_ls(&json!({})).is_err());
    }

    fn kitty(path: &Path) -> Kitty {
        Kitty {
            address: Address::parse(&format!("unix:{}", path.display())).unwrap(),
        }
    }

    #[test]
    fn the_backend_lists_focuses_and_launches() {
        let (path, log) = serve(Mode::Normal, ls_json());
        let k = kitty(&path);
        let windows = k.windows().unwrap();
        assert_eq!(windows.len(), 2);
        k.focus(&windows[0].id).unwrap();
        k.launch(Path::new("/w/new")).unwrap();
        let log = log.lock().unwrap();
        let cmds: Vec<_> = log.iter().map(|v| v["cmd"].as_str().unwrap()).collect();
        assert_eq!(cmds, ["ls", "focus-window", "launch"]);
        assert_eq!(log[1]["payload"], json!({"match": "id:10"}));
        assert_eq!(
            log[2]["payload"],
            json!({"type": "os-window", "cwd": "/w/new", "args": ["jed", "/w/new"]})
        );
        assert!(
            log.iter()
                .all(|v| v["version"] == json!([0, 14, 2]) && v["no_response"] == false)
        );
    }

    #[test]
    fn failures_are_errors() {
        let (path, _) = serve(Mode::Refuse, ls_json());
        assert!(kitty(&path).windows().unwrap_err().contains("remote control refused"));
        let (path, _) = serve(Mode::Close, ls_json());
        assert!(kitty(&path).windows().is_err());
        assert!(
            kitty(&short_dir().join("none"))
                .windows()
                .unwrap_err()
                .contains("can't connect")
        );
        let (path, _) = serve(Mode::Silent, ls_json());
        let t = Instant::now();
        assert!(kitty(&path).windows().unwrap_err().contains("in time"));
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn detect_needs_a_unix_address() {
        let env = |s: Option<&str>| Env {
            kitty_listen_on: s.map(String::from),
        };
        assert!(crate::termctl::detect(&env(None)).is_none());
        assert!(crate::termctl::detect(&env(Some("tcp:localhost:1"))).is_none());
        assert_eq!(
            crate::termctl::detect(&env(Some("unix:/tmp/x"))).unwrap().name(),
            "kitty"
        );
    }
}
