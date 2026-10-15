//! An in-process language server for tests: `FakeLauncher` hands the client one end of a
//! `tokio::io::duplex` and runs a small server on the other, recording what it receives.
//!
//! The fake answers `initialize` (with `positionEncoding: utf-8` when `FakeOptions::utf8`, an
//! error when `init_error`), asks
//! the client `workspace/configuration` (id `"cfg"`), `window/workDoneProgress/create`
//! (`"progress"`) and an unknown method (`"bogus"`) while initializing, answers `shutdown` and
//! stops on `exit`. Test methods: `test/echo` answers with its params, `test/later` is held until
//! a `test/flush` notification answers the held ones newest first; anything else gets
//! MethodNotFound.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::config::ServerConfig;
use super::rpc::{Framer, Id, METHOD_NOT_FOUND, Message, RpcError};
use super::server::{Launcher, Transport};

#[derive(Debug, Clone, Default)]
pub struct FakeOptions {
    /// Answer `initialize` with `positionEncoding: utf-8` (else no encoding: UTF-16).
    pub utf8: bool,
    /// Commands that fail to launch as not found.
    pub missing: Vec<String>,
    /// Hang up right after `initialized`.
    pub crash_after_init: bool,
    /// Never answer `shutdown`.
    pub ignore_shutdown: bool,
    /// Answer `initialize` with an error.
    pub init_error: bool,
}

#[derive(Default)]
struct Shared {
    /// Each launch's server name (also the ones that fail as missing).
    launches: Vec<String>,
    /// Every message received (by any launch), in order.
    received: Vec<Value>,
    /// The client's answers to the fake's requests, by request id.
    answers: HashMap<String, Value>,
}

pub struct FakeLauncher {
    options: FakeOptions,
    shared: Arc<Mutex<Shared>>,
    exited: Arc<AtomicUsize>,
}

impl FakeLauncher {
    pub fn new(options: FakeOptions) -> Arc<Self> {
        Arc::new(Self {
            options,
            shared: Default::default(),
            exited: Default::default(),
        })
    }

    pub fn launches(&self) -> Vec<String> {
        self.shared.lock().unwrap().launches.clone()
    }

    /// Methods of the requests and notifications received, in order.
    pub fn methods(&self) -> Vec<String> {
        self.received()
            .iter()
            .filter_map(|m| m.get("method").and_then(Value::as_str).map(String::from))
            .collect()
    }

    pub fn received(&self) -> Vec<Value> {
        self.shared.lock().unwrap().received.clone()
    }

    /// The first received message with this method.
    pub fn first(&self, method: &str) -> Option<Value> {
        self.received().into_iter().find(|m| m["method"] == method)
    }

    pub fn answers(&self) -> HashMap<String, Value> {
        self.shared.lock().unwrap().answers.clone()
    }

    /// How many fake servers have stopped.
    pub fn exited(&self) -> usize {
        self.exited.load(Ordering::SeqCst)
    }
}

impl Launcher for FakeLauncher {
    fn launch(&self, config: &ServerConfig, _root: &Path) -> std::io::Result<Transport> {
        self.shared.lock().unwrap().launches.push(config.name.clone());
        if self.options.missing.contains(&config.command) {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client);
        let (server_read, server_write) = tokio::io::split(server);
        let (stderr_read, mut stderr_write) = tokio::io::duplex(1024);
        let shared = self.shared.clone();
        let options = self.options.clone();
        let exited = self.exited.clone();
        tokio::spawn(async move {
            let _ = stderr_write.write_all(b"fake server starting\n").await;
            let mut server_write = server_write;
            serve(server_read, &mut server_write, options, shared).await;
            // Counted before the output closes, so a client that saw it close sees the count.
            exited.fetch_add(1, Ordering::SeqCst);
            drop(server_write);
        });
        Ok(Transport {
            reader: Box::pin(client_read),
            writer: Box::pin(client_write),
            stderr: Some(Box::pin(stderr_read)),
            child: None,
        })
    }
}

async fn serve(
    mut input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
    options: FakeOptions,
    shared: Arc<Mutex<Shared>>,
) {
    let mut framer = Framer::default();
    let mut buf = vec![0u8; 8192];
    let mut held: Vec<(Id, Value)> = Vec::new();
    loop {
        let n = match input.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        framer.push(&buf[..n]);
        while let Some(Ok(body)) = framer.next() {
            let value: Value = serde_json::from_slice(&body).unwrap();
            shared.lock().unwrap().received.push(value.clone());
            let mut out = Vec::new();
            match Message::from_value(value).unwrap() {
                Message::Request { id, method, params } => match method.as_str() {
                    "initialize" if options.init_error => out.push(Message::Response {
                        id,
                        result: Err(RpcError {
                            code: -32603,
                            message: "fake can't start".into(),
                        }),
                    }),
                    "initialize" => {
                        for (id, method, params) in [
                            ("cfg", "workspace/configuration", json!({ "items": [{}, {}] })),
                            ("progress", "window/workDoneProgress/create", json!({ "token": "t" })),
                            ("bogus", "fake/bogus", Value::Null),
                        ] {
                            out.push(Message::Request {
                                id: Id::Str(id.into()),
                                method: method.into(),
                                params,
                            });
                        }
                        out.push(Message::notification(
                            "window/logMessage",
                            json!({ "type": 3, "message": "fake ready" }),
                        ));
                        let mut caps = json!({ "textDocumentSync": 2 });
                        if options.utf8 {
                            caps["positionEncoding"] = json!("utf-8");
                        }
                        out.push(Message::Response {
                            id,
                            result: Ok(
                                json!({ "capabilities": caps, "serverInfo": { "name": "fake", "version": "1" } }),
                            ),
                        });
                    }
                    "shutdown" if options.ignore_shutdown => {}
                    "shutdown" => out.push(Message::Response {
                        id,
                        result: Ok(Value::Null),
                    }),
                    "test/echo" => out.push(Message::Response { id, result: Ok(params) }),
                    "test/later" => held.push((id, params)),
                    _ => out.push(Message::Response {
                        id,
                        result: Err(RpcError {
                            code: METHOD_NOT_FOUND,
                            message: format!("fake doesn't know {method}"),
                        }),
                    }),
                },
                Message::Notification { method, .. } => match method.as_str() {
                    "exit" => return,
                    "initialized" if options.crash_after_init => return,
                    "test/flush" => {
                        for (id, params) in held.drain(..).rev() {
                            out.push(Message::Response { id, result: Ok(params) });
                        }
                    }
                    _ => {}
                },
                Message::Response {
                    id: Id::Str(id),
                    result,
                } => {
                    let answer = match result {
                        Ok(v) => v,
                        Err(e) => json!({ "error": { "code": e.code, "message": e.message } }),
                    };
                    shared.lock().unwrap().answers.insert(id, answer);
                }
                Message::Response { .. } => {}
            }
            for m in out {
                if output.write_all(&m.encode()).await.is_err() {
                    return;
                }
            }
        }
    }
}
