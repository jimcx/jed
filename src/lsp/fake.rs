//! An in-process language server for tests: `FakeLauncher` hands the client one end of a
//! `tokio::io::duplex` and runs a small server on the other, recording what it receives.
//!
//! The fake answers `initialize` (with `positionEncoding: utf-8` when `FakeOptions::utf8`, an
//! error when `init_error`), asks
//! the client `workspace/configuration` (id `"cfg"`), `window/workDoneProgress/create`
//! (`"progress"`) and an unknown method (`"bogus"`) while initializing, answers `shutdown` and
//! stops on `exit`. Test methods: `test/echo` answers with its params, `test/later` is held until
//! a `test/flush` notification answers the held ones newest first; anything else gets
//! MethodNotFound. A `test/publish` notification makes it send `textDocument/publishDiagnostics`
//! with the same params (a test's way to publish diagnostics: for any URI and version, in order
//! after the document notifications sent before it), `test/progress` a `$/progress` with its
//! params (a token's `begin` / `report` / `end`); `test/crash` makes it hang up. For tool logs:
//! it writes `fake server starting` to its stderr when launched, `test/stderr` writes its
//! `text` param as a stderr line, `test/log` / `test/show` send `window/logMessage` /
//! `window/showMessage` with their params; `test/inactive` sends `textDocument/inactiveRegions`
//! with its params; `test/refresh` sends a `workspace/semanticTokens/refresh` request (id
//! `"refresh"`; the client's answer in `answers()`).
//!
//! Scripted answers: `respond(method, result)` makes it answer every `method` request (e.g.
//! `textDocument/hover`) with `result`; `hold(method, true)` keeps those answers until a
//! `test/flush` (out of order, as `test/later`). A `$/cancelRequest` for a held request answers
//! it at once with RequestCancelled (-32800), as servers do. `fail(method, code, message)` makes
//! it answer every `method` request with that error instead (e.g. ContentModified, -32801). It
//! advertises `hoverProvider` (unless `FakeOptions::no_hover`), `definitionProvider`, `completionProvider` (trigger characters `.` and `>`, resolve),
//! `referencesProvider`, `renameProvider` (with `prepareProvider` unless
//! `FakeOptions::no_prepare_rename`) and `documentFormattingProvider` (unless
//! `FakeOptions::no_formatting`) and `codeActionProvider` (with `resolveProvider`, unless
//! `FakeOptions::no_code_actions`) and, with `FakeOptions::semantic_tokens`, `semanticTokensProvider`; an unscripted `completionItem/resolve` or
//! `codeAction/resolve` answers with the item it got, any other unscripted request with
//! MethodNotFound. `apply_edit_on(method, params)` makes it send `workspace/applyEdit` with
//! `params` before answering each `method` request (a command's edit, as servers send it).
//!
//! Documents: it keeps its own copy of every document opened (`didOpen`), applies each
//! `didChange` to it (ranges in the negotiated encoding, converted here independently of
//! `buffer::pos`; checks that versions go up by one, ranges are in the text and a `didSave`'s text is
//! its copy's) and drops it on
//! `didClose`; `document(uri)` reads it, `saves()` lists the `didSave`s, and `errors()` anything
//! wrong in the stream. `FakeOptions::sync` replaces the advertised `textDocumentSync` (default
//! `2`, incremental).

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
    /// The `textDocumentSync` capability to advertise (default `2`: incremental).
    pub sync: Option<Value>,
    /// Advertise `renameProvider: true` (no `prepareRename`) instead of `{ prepareProvider: true }`.
    pub no_prepare_rename: bool,
    /// Don't advertise `documentFormattingProvider`.
    pub no_formatting: bool,
    /// Don't advertise `hoverProvider`.
    pub no_hover: bool,
    /// Don't advertise `codeActionProvider`.
    pub no_code_actions: bool,
    /// What `Launcher::locate` answers (the `xcrun --find` fallback for `sourcekit-lsp`).
    pub located: Option<String>,
    /// Advertise `semanticTokensProvider` (legend: types `variable`, `class`, `function`;
    /// modifiers `declaration`, `constructorOrDestructor`; `full` with `delta`, no `range`).
    pub semantic_tokens: bool,
}

/// The fake's copy of an open document.
#[derive(Debug, Clone, PartialEq)]
pub struct FakeDoc {
    pub language: String,
    pub version: i64,
    pub text: String,
}

#[derive(Default)]
struct Shared {
    /// Each launch's server name (also the ones that fail as missing).
    launches: Vec<String>,
    /// Each launch's command (a located one's absolute path).
    commands: Vec<String>,
    /// Every message received (by any launch), in order.
    received: Vec<Value>,
    /// The client's answers to the fake's requests, by request id.
    answers: HashMap<String, Value>,
    /// Open documents by URI.
    docs: HashMap<String, FakeDoc>,
    /// `didSave`s: URI and text, if sent.
    saves: Vec<(String, Option<String>)>,
    /// What was wrong in the document notifications.
    errors: Vec<String>,
    /// Scripted answers by request method.
    scripts: HashMap<String, Value>,
    /// Methods whose scripted answers wait for `test/flush`.
    holding: std::collections::HashSet<String>,
    /// Scripted errors by request method (before `scripts`).
    failures: HashMap<String, RpcError>,
    /// `workspace/applyEdit` params to send (before answering) on a request with this method.
    apply_on: HashMap<String, Value>,
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

    /// Each launch's command, as launched.
    pub fn commands(&self) -> Vec<String> {
        self.shared.lock().unwrap().commands.clone()
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

    /// The fake's copy of the document at `uri`, if open.
    pub fn document(&self, uri: &str) -> Option<FakeDoc> {
        self.shared.lock().unwrap().docs.get(uri).cloned()
    }

    pub fn saves(&self) -> Vec<(String, Option<String>)> {
        self.shared.lock().unwrap().saves.clone()
    }

    /// Problems found in the document notifications (none expected).
    pub fn errors(&self) -> Vec<String> {
        self.shared.lock().unwrap().errors.clone()
    }

    /// Answers every `method` request with `result` from now on.
    pub fn respond(&self, method: &str, result: Value) {
        self.shared.lock().unwrap().scripts.insert(method.into(), result);
    }

    /// Answers every `method` request with the error `code` / `message` from now on.
    pub fn fail(&self, method: &str, code: i64, message: &str) {
        let error = RpcError {
            code,
            message: message.into(),
        };
        self.shared.lock().unwrap().failures.insert(method.into(), error);
    }

    /// Holds (`true`) the scripted answers to `method` until a `test/flush`, or stops holding.
    pub fn hold(&self, method: &str, on: bool) {
        let mut shared = self.shared.lock().unwrap();
        if on {
            shared.holding.insert(method.into());
        } else {
            shared.holding.remove(method);
        }
    }

    /// From now on, every `method` request (e.g. `workspace/executeCommand`) first makes it send
    /// `workspace/applyEdit` with `params` (ids `"apply1"`, `"apply2"`, …; the client's answers in
    /// `answers()`), then answers as it would.
    pub fn apply_edit_on(&self, method: &str, params: Value) {
        self.shared.lock().unwrap().apply_on.insert(method.into(), params);
    }

    /// The `params` of every request received with `method`, in order.
    pub fn requests(&self, method: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|m| m["method"] == method && m.get("id").is_some())
            .map(|m| m["params"].clone())
            .collect()
    }

    /// The ids `$/cancelRequest` named, in order.
    pub fn cancelled(&self) -> Vec<i64> {
        self.received()
            .into_iter()
            .filter(|m| m["method"] == "$/cancelRequest")
            .filter_map(|m| m["params"]["id"].as_i64())
            .collect()
    }

    /// How many fake servers have stopped.
    pub fn exited(&self) -> usize {
        self.exited.load(Ordering::SeqCst)
    }
}

impl Launcher for FakeLauncher {
    fn locate(&self, _command: &str) -> Option<String> {
        self.options.located.clone()
    }

    fn launch(&self, config: &ServerConfig, _root: &Path) -> std::io::Result<Transport> {
        {
            let mut shared = self.shared.lock().unwrap();
            shared.launches.push(config.name.clone());
            shared.commands.push(config.command.clone());
        }
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
            serve(server_read, &mut server_write, &mut stderr_write, options, shared).await;
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
    mut stderr: impl AsyncWrite + Unpin,
    options: FakeOptions,
    shared: Arc<Mutex<Shared>>,
) {
    let mut framer = Framer::default();
    let mut buf = vec![0u8; 8192];
    let mut held: Vec<(Id, Value)> = Vec::new();
    let mut applies = 0;
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
            if let Some(Message::Request { method, .. }) = Message::from_value(value.clone()) {
                let apply = shared.lock().unwrap().apply_on.get(&method).cloned();
                if let Some(params) = apply {
                    applies += 1;
                    out.push(Message::Request {
                        id: Id::Str(format!("apply{applies}")),
                        method: "workspace/applyEdit".into(),
                        params,
                    });
                }
            }
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
                        let mut caps = json!({
                            "textDocumentSync": options.sync.clone().unwrap_or(json!(2)),
                            "hoverProvider": !options.no_hover,
                            "definitionProvider": true,
                            "completionProvider": {
                                "triggerCharacters": [".", ">"],
                                "resolveProvider": true,
                            },
                            "referencesProvider": true,
                            "renameProvider": if options.no_prepare_rename {
                                json!(true)
                            } else {
                                json!({ "prepareProvider": true })
                            },
                            "documentFormattingProvider": !options.no_formatting,
                        });
                        if !options.no_code_actions {
                            caps["codeActionProvider"] = json!({
                                "codeActionKinds": ["quickfix", "refactor", "source"],
                                "resolveProvider": true,
                            });
                        }
                        if options.semantic_tokens {
                            caps["semanticTokensProvider"] = json!({
                                "legend": {
                                    "tokenTypes": ["variable", "class", "function"],
                                    "tokenModifiers": ["declaration", "constructorOrDestructor"],
                                },
                                "full": { "delta": true },
                                "range": false,
                            });
                        }
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
                    m if shared.lock().unwrap().failures.contains_key(m) => {
                        let error = shared.lock().unwrap().failures[m].clone();
                        out.push(Message::Response { id, result: Err(error) });
                    }
                    m if shared.lock().unwrap().scripts.contains_key(m) => {
                        let shared = shared.lock().unwrap();
                        let result = shared.scripts[m].clone();
                        if shared.holding.contains(m) {
                            held.push((id, result));
                        } else {
                            out.push(Message::Response { id, result: Ok(result) });
                        }
                    }
                    // Unscripted, a resolve answers with the item as it came.
                    "completionItem/resolve" | "codeAction/resolve" => {
                        out.push(Message::Response { id, result: Ok(params) })
                    }
                    _ => out.push(Message::Response {
                        id,
                        result: Err(RpcError {
                            code: METHOD_NOT_FOUND,
                            message: format!("fake doesn't know {method}"),
                        }),
                    }),
                },
                Message::Notification { method, params } => match method.as_str() {
                    "exit" => return,
                    m if m.starts_with("textDocument/") => {
                        let mut shared = shared.lock().unwrap();
                        if let Err(e) = document(&mut shared, m, &params, options.utf8) {
                            shared.errors.push(format!("{m}: {e}"));
                        }
                    }
                    "initialized" if options.crash_after_init => return,
                    "test/crash" => return,
                    "$/cancelRequest" => {
                        let id = params["id"].as_i64();
                        if let Some(i) = held.iter().position(|(h, _)| matches!(h, Id::Num(n) if Some(*n) == id)) {
                            let (id, _) = held.remove(i);
                            out.push(Message::Response {
                                id,
                                result: Err(RpcError {
                                    code: -32800,
                                    message: "cancelled".into(),
                                }),
                            });
                        }
                    }
                    "test/stderr" => {
                        let text = params["text"].as_str().unwrap_or("");
                        let _ = stderr.write_all(format!("{text}\n").as_bytes()).await;
                    }
                    "test/log" => out.push(Message::notification("window/logMessage", params)),
                    "test/show" => out.push(Message::notification("window/showMessage", params)),
                    "test/publish" => out.push(Message::notification("textDocument/publishDiagnostics", params)),
                    "test/inactive" => out.push(Message::notification("textDocument/inactiveRegions", params)),
                    "test/progress" => out.push(Message::notification("$/progress", params)),
                    "test/refresh" => out.push(Message::Request {
                        id: Id::Str("refresh".into()),
                        method: "workspace/semanticTokens/refresh".into(),
                        params: Value::Null,
                    }),
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

/// Applies a document notification to the fake's copies.
fn document(shared: &mut Shared, method: &str, params: &Value, utf8: bool) -> Result<(), String> {
    let uri = params["textDocument"]["uri"].as_str().ok_or("no uri")?.to_string();
    match method {
        "textDocument/didOpen" => {
            let doc = FakeDoc {
                language: params["textDocument"]["languageId"]
                    .as_str()
                    .ok_or("no languageId")?
                    .into(),
                version: params["textDocument"]["version"].as_i64().ok_or("no version")?,
                text: params["textDocument"]["text"].as_str().ok_or("no text")?.into(),
            };
            if shared.docs.insert(uri.clone(), doc).is_some() {
                return Err(format!("{uri} opened twice"));
            }
        }
        "textDocument/didChange" => {
            let doc = shared.docs.get_mut(&uri).ok_or_else(|| format!("{uri} isn't open"))?;
            let version = params["textDocument"]["version"].as_i64().ok_or("no version")?;
            if version != doc.version + 1 {
                return Err(format!("version {version} after {}", doc.version));
            }
            doc.version = version;
            for c in params["contentChanges"].as_array().ok_or("no contentChanges")? {
                let text = c["text"].as_str().ok_or("no text")?;
                match c.get("range") {
                    None => doc.text = text.to_string(),
                    Some(r) => {
                        let at = |p: &Value| {
                            offset(
                                &doc.text,
                                p["line"].as_u64().ok_or("no line")?,
                                p["character"].as_u64().ok_or("no character")?,
                                utf8,
                            )
                        };
                        let (start, end) = (at(&r["start"])?, at(&r["end"])?);
                        if start > end {
                            return Err(format!("reversed range {r}"));
                        }
                        doc.text.replace_range(start..end, text);
                    }
                }
            }
        }
        "textDocument/didClose" => {
            shared
                .docs
                .remove(&uri)
                .ok_or_else(|| format!("{uri} closed but not open"))?;
        }
        "textDocument/didSave" => {
            if !shared.docs.contains_key(&uri) {
                return Err(format!("{uri} saved but not open"));
            }
            let text = params.get("text").and_then(Value::as_str).map(String::from);
            if text.as_ref().is_some_and(|t| *t != shared.docs[&uri].text) {
                return Err(format!("{uri} saved with text that isn't the document's"));
            }
            shared.saves.push((uri, text));
        }
        _ => {}
    }
    Ok(())
}

/// The byte offset of an LSP position in `text`, strictly: the line must exist, the column must
/// be within its content and on a char boundary. Line breaks: LF, CRLF, CR. Written apart from
/// `buffer::pos`, so the client's conversions are checked against something else.
fn offset(text: &str, line: u64, character: u64, utf8: bool) -> Result<usize, String> {
    let bytes = text.as_bytes();
    let mut start = 0;
    for _ in 0..line {
        let rel = bytes[start..]
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
            .ok_or_else(|| format!("line {line} past the end"))?;
        let br = start + rel;
        start = if bytes[br] == b'\r' && bytes.get(br + 1) == Some(&b'\n') {
            br + 2
        } else {
            br + 1
        };
    }
    let content_end = bytes[start..]
        .iter()
        .position(|&b| b == b'\n' || b == b'\r')
        .map_or(text.len(), |r| start + r);
    let mut units = 0u64;
    for (i, c) in text[start..content_end].char_indices() {
        if units == character {
            return Ok(start + i);
        }
        if units > character {
            return Err(format!("{line}:{character} inside a char"));
        }
        units += if utf8 { c.len_utf8() } else { c.len_utf16() } as u64;
    }
    if units == character {
        Ok(content_end)
    } else {
        Err(format!("{line}:{character} past the line's end ({units})"))
    }
}
