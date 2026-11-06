//! A local HTTP server for the forge tests: serves canned replies to GET requests and records
//! what it was asked. Plain HTTP on 127.0.0.1 (the tests build clients with `allow_http`).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a route answers.
#[derive(Clone, Debug)]
pub struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    pub fn json(body: &str) -> Reply {
        Reply {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Reply {
        Reply {
            status,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Reply {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_body(mut self, body: &str) -> Reply {
        self.body = body.into();
        self
    }
}

/// A request the server saw.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// Path and query, as sent.
    pub target: String,
    /// The request body as text (empty for a GET).
    pub body: String,
    headers: Vec<(String, String)>,
}

impl Request {
    /// A header's value; `name` is lower case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

pub struct FakeServer {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}

impl FakeServer {
    /// `routes` maps a request target (`/path?query`) or just a path to its reply; anything else is a 404.
    pub fn start(routes: Vec<(String, Reply)>) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(stream) = stream {
                    let _ = serve(stream, &routes, &seen2);
                }
            }
        });
        FakeServer { addr, seen, stop }
    }

    /// `http://127.0.0.1:port`
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr); // wake the accept loop
    }
}

fn serve(stream: TcpStream, routes: &[(String, Reply)], seen: &Mutex<Vec<Request>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Ok(());
    };
    let (method, target) = (method.to_string(), target.to_string());
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((n, v)) = h.split_once(':') {
            headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let length: usize = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut raw = vec![0u8; length];
    std::io::Read::read_exact(&mut reader, &mut raw)?;
    let body = String::from_utf8_lossy(&raw).into_owned();
    let path = target.split('?').next().unwrap_or(&target).to_string();
    // A route may start with a method (`POST /path`); it then matches that method only.
    let (with_target, with_path) = (format!("{method} {target}"), format!("{method} {path}"));
    let reply = [&with_target, &with_path, &target, &path]
        .iter()
        .find_map(|k| routes.iter().find(|(t, _)| t == *k))
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| Reply::status(404).with_body("{\"message\":\"not found\"}"));
    seen.lock().unwrap().push(Request {
        method,
        target,
        headers,
        body,
    });
    let mut out = format!(
        "HTTP/1.1 {} X\r\nConnection: close\r\nContent-Length: {}\r\n",
        reply.status,
        reply.body.len()
    );
    for (n, v) in &reply.headers {
        out.push_str(&format!("{n}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(&reply.body);
    let mut stream = stream;
    stream.write_all(out.as_bytes())?;
    stream.flush()
}
