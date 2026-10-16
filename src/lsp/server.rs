//! One language server: the tokio task that starts it, runs `initialize`, routes messages both
//! ways and shuts it down. A `Launcher` makes the byte streams, so tests plug in a fake server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use lsp_types::{
    ClientCapabilities, ClientInfo, CompletionClientCapabilities, CompletionItemCapability,
    CompletionItemCapabilityResolveSupport, CompletionListCapability, GeneralClientCapabilities, GotoCapability,
    HoverClientCapabilities, InitializeParams, MarkupKind, PositionEncodingKind, PrepareSupportDefaultBehavior,
    PublishDiagnosticsClientCapabilities, ReferenceClientCapabilities, RenameClientCapabilities, ServerCapabilities,
    TextDocumentClientCapabilities, WindowClientCapabilities, WorkspaceClientCapabilities,
    WorkspaceEditClientCapabilities, WorkspaceFolder,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::config::ServerConfig;
use super::rpc::{Framer, Id, METHOD_NOT_FOUND, Message, RpcError};
use super::{Event, ServerId, Sink, StartError, uri};
use crate::buffer::pos::PosEncoding;

/// After asking a server to shut down, how long it gets to exit before it is killed.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(1500);
/// After `exit` went out, how long the server gets to close its output before it is killed:
/// clangd with a file open keeps running ~2 s after `exit` to finish indexing the standard
/// library, which would hold up quitting.
const EXIT_GRACE: Duration = Duration::from_millis(300);
/// How many of the last stderr lines a crash report quotes in the log.
const STDERR_TAIL: usize = 20;

/// A started server's streams (its stdout, stdin and stderr) and its process, if any.
pub struct Transport {
    pub reader: Pin<Box<dyn AsyncRead + Send>>,
    pub writer: Pin<Box<dyn AsyncWrite + Send>>,
    pub stderr: Option<Pin<Box<dyn AsyncRead + Send>>>,
    /// Killed when dropped (`kill_on_drop`), so an aborted task leaves no process behind.
    pub child: Option<tokio::process::Child>,
}

/// Starts servers. Called on a runtime worker thread, from the server's task.
pub trait Launcher: Send + Sync {
    fn launch(&self, config: &ServerConfig, root: &Path) -> std::io::Result<Transport>;
}

/// Runs the configured command as a child process with piped stdio, in the workspace root.
pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    fn launch(&self, config: &ServerConfig, root: &Path) -> std::io::Result<Transport> {
        let mut child = tokio::process::Command::new(program(&config.command, root))
            .args(&config.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let piped = || std::io::Error::other("stdio not piped");
        Ok(Transport {
            reader: Box::pin(child.stdout.take().ok_or_else(piped)?),
            writer: Box::pin(child.stdin.take().ok_or_else(piped)?),
            stderr: child
                .stderr
                .take()
                .map(|e| Box::pin(e) as Pin<Box<dyn AsyncRead + Send>>),
            child: Some(child),
        })
    }
}

/// A bare name is looked up on `PATH` (by `Command`); a relative path is relative to the root.
fn program(command: &str, root: &Path) -> PathBuf {
    let path = Path::new(command);
    if path.is_relative() && path.components().count() > 1 {
        root.join(path)
    } else {
        path.to_path_buf()
    }
}

/// From the app to a server's task.
#[derive(Debug)]
pub enum ToServer {
    Request {
        id: i64,
        method: String,
        params: Value,
    },
    Notify {
        method: String,
        params: Value,
    },
    /// `shutdown`, then `exit`; killed after `SHUTDOWN_TIMEOUT`.
    Shutdown,
}

/// What a request we sent is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Initialize,
    Shutdown,
    /// The app's: the response goes out as `Event::Response`.
    App,
}

/// Everything one server's task owns.
pub struct Task {
    pub id: ServerId,
    pub config: ServerConfig,
    pub root: PathBuf,
    pub launcher: Arc<dyn Launcher>,
    pub rx: mpsc::UnboundedReceiver<ToServer>,
    pub sink: Sink,
    /// Request ids, shared with the app's handle (which numbers its own requests).
    pub next_id: Arc<AtomicI64>,
}

/// How the task ended.
enum End {
    /// Asked to (`exit` sent, or shut down before it was initialized).
    Requested,
    /// Already reported (`Event::Failed`).
    Failed,
    /// The server went away by itself, or broke the protocol.
    Crashed(String),
}

impl Task {
    pub async fn run(self) {
        let name = self.config.name.clone();
        let transport = match self.launcher.launch(&self.config, &self.root) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(server = %name, command = %self.config.command, "can't start language server: {e}");
                let error = if e.kind() == std::io::ErrorKind::NotFound {
                    StartError::NotFound
                } else {
                    StartError::Spawn(e.to_string())
                };
                (self.sink)(Event::Failed { server: self.id, error });
                return;
            }
        };
        tracing::info!(server = %name, command = %self.config.command, root = %self.root.display(), "language server started");
        let Transport {
            mut reader,
            writer,
            stderr,
            mut child,
        } = transport;
        let (wtx, wrx) = mpsc::unbounded_channel();
        // Writes run apart from reads: a server blocked writing to a full stdout pipe while we
        // block writing to its full stdin would deadlock.
        let write_task = tokio::spawn(write_loop(writer, wrx));
        let mut conn = Conn {
            id: self.id,
            name: name.clone(),
            root: self.root.clone(),
            sink: self.sink.clone(),
            out: wtx,
            next_id: self.next_id.clone(),
            pending: HashMap::new(),
            initialized: false,
            queued: Vec::new(),
            shutting_down: false,
            exit_sent: false,
        };
        conn.send_initialize(&self.config);

        let mut rx = self.rx;
        let mut rx_open = true;
        let mut stderr = stderr.map(|e| BufReader::new(e).lines());
        let mut tail: std::collections::VecDeque<String> = Default::default();
        let mut framer = Framer::default();
        let mut buf = vec![0u8; 64 * 1024];
        let mut deadline: Option<tokio::time::Instant> = None;
        let mut timed_out = false;
        let end = 'run: loop {
            let timeout = async move {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                read = reader.read(&mut buf) => {
                    let n = match read {
                        Ok(0) => break 'run conn.eof(),
                        Ok(n) => n,
                        Err(e) => break 'run End::Crashed(format!("reading its output: {e}")),
                    };
                    framer.push(&buf[..n]);
                    while let Some(frame) = framer.next() {
                        match frame {
                            Ok(body) => {
                                if let Some(end) = conn.incoming(&body) {
                                    break 'run end;
                                }
                            }
                            Err(e) => break 'run End::Crashed(format!("protocol error: {e}")),
                        }
                    }
                    if conn.exit_sent {
                        let grace = tokio::time::Instant::now() + EXIT_GRACE;
                        deadline = Some(deadline.map_or(grace, |d| d.min(grace)));
                    }
                }
                line = async { stderr.as_mut().expect("guarded").next_line().await }, if stderr.is_some() => {
                    match line {
                        Ok(Some(line)) => {
                            tracing::debug!(target: "lsp::stderr", server = %name, "{line}");
                            if tail.len() == STDERR_TAIL {
                                tail.pop_front();
                            }
                            tail.push_back(line);
                        }
                        _ => stderr = None,
                    }
                }
                cmd = rx.recv(), if rx_open => {
                    let cmd = cmd.unwrap_or_else(|| {
                        rx_open = false;
                        ToServer::Shutdown
                    });
                    if matches!(cmd, ToServer::Shutdown) && deadline.is_none() {
                        deadline = Some(tokio::time::Instant::now() + SHUTDOWN_TIMEOUT);
                    }
                    if let Some(end) = conn.command(cmd) {
                        break 'run end;
                    }
                }
                () = timeout => {
                    if conn.exit_sent {
                        tracing::info!(server = %name, "language server still running after exit: killing it");
                    } else {
                        tracing::warn!(server = %name, "language server didn't exit in time: killing it");
                    }
                    timed_out = true;
                    break 'run End::Requested;
                }
            }
        };
        drop(conn);
        let _ = write_task.await;
        let status = match &mut child {
            // Out of time already: no more waiting.
            Some(child) if timed_out => {
                let _ = child.kill().await;
                None
            }
            Some(child) => match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
                Ok(Ok(status)) => Some(status.to_string()),
                _ => {
                    let _ = child.kill().await;
                    None
                }
            },
            None => None,
        };
        match end {
            End::Requested => {
                tracing::info!(server = %name, status = status.as_deref().unwrap_or("killed"), "language server stopped");
                (self.sink)(Event::Exited {
                    server: self.id,
                    crashed: None,
                });
            }
            End::Failed => {}
            End::Crashed(why) => {
                let why = match status {
                    Some(s) => format!("{why}, {s}"),
                    None => why,
                };
                let lines: Vec<String> = tail.into();
                tracing::warn!(server = %name, stderr = %lines.join("\n"), "language server exited: {why}");
                (self.sink)(Event::Exited {
                    server: self.id,
                    crashed: Some(why),
                });
            }
        }
    }
}

async fn write_loop(mut writer: Pin<Box<dyn AsyncWrite + Send>>, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = rx.recv().await {
        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
            return;
        }
    }
    let _ = writer.shutdown().await;
}

/// The protocol state of one connection.
struct Conn {
    id: ServerId,
    name: String,
    root: PathBuf,
    sink: Sink,
    out: mpsc::UnboundedSender<Vec<u8>>,
    next_id: Arc<AtomicI64>,
    pending: HashMap<i64, Pending>,
    /// The `initialize` response came (and `initialized` went out).
    initialized: bool,
    /// The app's messages sent before that: the protocol allows none until then.
    queued: Vec<ToServer>,
    shutting_down: bool,
    /// `exit` went out: the server has `EXIT_GRACE` left.
    exit_sent: bool,
}

impl Conn {
    fn send(&self, msg: &Message) {
        let body = msg.to_value().to_string();
        tracing::trace!(target: "lsp::wire", server = %self.name, "→ {body}");
        let _ = self.out.send(super::rpc::encode_body(body.as_bytes()));
    }

    fn request(&mut self, method: &str, params: Value, kind: Pending) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending.insert(id, kind);
        self.send(&Message::request(id, method, params));
    }

    fn send_initialize(&mut self, config: &ServerConfig) {
        let root_uri = uri::from_path(&self.root);
        let folder_name = self
            .root
            .file_name()
            .map_or_else(|| self.root.display().to_string(), |n| n.to_string_lossy().into_owned());
        #[allow(deprecated)] // `root_uri`: still what clangd reads
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri.clone()),
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: root_uri,
                name: folder_name,
            }]),
            initialization_options: config.init_options.clone(),
            capabilities: client_capabilities(),
            client_info: Some(ClientInfo {
                name: "red".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            ..Default::default()
        };
        let mut params = serde_json::to_value(params).unwrap_or(Value::Null);
        // clangd's pre-3.17 extension, for clangd versions without `positionEncodings`.
        params["capabilities"]["offsetEncoding"] = json!(["utf-8", "utf-16"]);
        self.request("initialize", params, Pending::Initialize);
    }

    /// One body from the server; `Some` ends the task.
    fn incoming(&mut self, body: &[u8]) -> Option<End> {
        tracing::trace!(target: "lsp::wire", server = %self.name, "← {}", String::from_utf8_lossy(body));
        let msg = match serde_json::from_slice(body).ok().and_then(Message::from_value) {
            Some(m) => m,
            None => {
                tracing::warn!(server = %self.name, "ignoring a message that isn't JSON-RPC");
                return None;
            }
        };
        match msg {
            Message::Response { id, result } => {
                let pending = match id {
                    Id::Num(n) => self.pending.remove(&n).map(|p| (n, p)),
                    Id::Str(_) => None,
                };
                match pending {
                    Some((_, Pending::Initialize)) => return self.initialize_done(result),
                    Some((_, Pending::Shutdown)) => {
                        if let Err(e) = result {
                            tracing::warn!(server = %self.name, "shutdown failed: {}", e.message);
                        }
                        self.send(&Message::notification("exit", Value::Null));
                        self.exit_sent = true;
                    }
                    Some((n, Pending::App)) => (self.sink)(Event::Response {
                        server: self.id,
                        id: n,
                        result,
                    }),
                    None => tracing::warn!(server = %self.name, ?id, "response to no request"),
                }
            }
            Message::Request { id, method, params } => {
                let result = self.answer(&method, &params);
                self.send(&Message::Response { id, result });
            }
            Message::Notification { method, params } => match method.as_str() {
                "window/logMessage" => {
                    let text = params.get("message").and_then(Value::as_str).unwrap_or("");
                    match params.get("type").and_then(Value::as_i64) {
                        Some(1) => tracing::warn!(server = %self.name, "{text}"),
                        Some(2) | Some(3) => tracing::info!(server = %self.name, "{text}"),
                        _ => tracing::debug!(server = %self.name, "{text}"),
                    }
                }
                "$/progress" | "$/logTrace" | "telemetry/event" => {}
                // Read here, off the UI thread (tens of ms for tens of thousands).
                "textDocument/publishDiagnostics" => match super::diagnostics::parse(params) {
                    Ok(published) => (self.sink)(Event::Diagnostics {
                        server: self.id,
                        published: Box::new(published),
                    }),
                    Err(e) => tracing::warn!(server = %self.name, "unreadable publishDiagnostics: {e}"),
                },
                _ => (self.sink)(Event::Notification {
                    server: self.id,
                    method,
                    params,
                }),
            },
        }
        None
    }

    /// Minimal answers to what servers ask the client, so none of them waits on us.
    fn answer(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            // No settings: one `null` per item asked for.
            "workspace/configuration" => {
                let n = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
                Ok(Value::Array(vec![Value::Null; n]))
            }
            "workspace/workspaceFolders" => Ok(json!([{
                "uri": uri::from_path(&self.root).as_str(),
                "name": self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            }])),
            "window/workDoneProgress/create"
            | "client/registerCapability"
            | "client/unregisterCapability"
            | "window/showMessageRequest"
            | "workspace/diagnostic/refresh"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh" => Ok(Value::Null),
            "workspace/applyEdit" => Ok(json!({ "applied": false, "failureReason": "not supported yet" })),
            _ => {
                tracing::debug!(server = %self.name, method, "unhandled server request");
                Err(RpcError {
                    code: METHOD_NOT_FOUND,
                    message: format!("red doesn't handle {method}"),
                })
            }
        }
    }

    fn initialize_done(&mut self, result: Result<Value, RpcError>) -> Option<End> {
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                (self.sink)(Event::Failed {
                    server: self.id,
                    error: StartError::Initialize(e.message),
                });
                return Some(End::Failed);
            }
        };
        let encoding = negotiated_encoding(&value);
        let capabilities = value
            .get("capabilities")
            .cloned()
            .map(serde_json::from_value::<ServerCapabilities>)
            .transpose()
            .unwrap_or_else(|e| {
                tracing::warn!(server = %self.name, "unreadable server capabilities: {e}");
                None
            })
            .unwrap_or_default();
        let version = value
            .pointer("/serverInfo/version")
            .and_then(Value::as_str)
            .unwrap_or("?");
        tracing::info!(server = %self.name, ?encoding, version, "language server initialized");
        self.initialized = true;
        self.send(&Message::notification("initialized", json!({})));
        (self.sink)(Event::Initialized {
            server: self.id,
            encoding,
            capabilities: Box::new(capabilities),
        });
        for cmd in std::mem::take(&mut self.queued) {
            if let Some(end) = self.command(cmd) {
                return Some(end);
            }
        }
        None
    }

    /// A message from the app; `Some` ends the task.
    fn command(&mut self, cmd: ToServer) -> Option<End> {
        if self.shutting_down {
            return None;
        }
        if !self.initialized && !matches!(cmd, ToServer::Shutdown) {
            self.queued.push(cmd);
            return None;
        }
        match cmd {
            ToServer::Request { id, method, params } => {
                self.pending.insert(id, Pending::App);
                self.send(&Message::request(id, &method, params));
            }
            ToServer::Notify { method, params } => self.send(&Message::notification(&method, params)),
            ToServer::Shutdown => {
                self.shutting_down = true;
                if !self.initialized {
                    // Nothing to say yet: just stop it.
                    return Some(End::Requested);
                }
                self.request("shutdown", Value::Null, Pending::Shutdown);
            }
        }
        None
    }

    /// The server closed its output.
    fn eof(&self) -> End {
        if self.shutting_down {
            End::Requested
        } else if !self.initialized {
            End::Crashed("exited during initialize".into())
        } else {
            End::Crashed("exited".into())
        }
    }
}

/// What red tells servers it can do: UTF-8 positions first (UTF-16 is the fallback every server
/// must support), work-done progress, pushed diagnostics (with the document version they're
/// for, so red can map them through the edits made since; no pull diagnostics: ty and clangd
/// push when the client doesn't declare `textDocument.diagnostic`), hover (markdown first),
/// go to definition (`LocationLink`s welcome), completion, references and rename (with
/// `prepareRename` and versioned `WorkspaceEdit`s).
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(vec![PositionEncodingKind::UTF8, PositionEncodingKind::UTF16]),
            ..Default::default()
        }),
        text_document: Some(TextDocumentClientCapabilities {
            publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                version_support: Some(true),
                ..Default::default()
            }),
            hover: Some(HoverClientCapabilities {
                dynamic_registration: None,
                content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
            }),
            definition: Some(GotoCapability {
                dynamic_registration: None,
                link_support: Some(true),
            }),
            references: Some(ReferenceClientCapabilities::default()),
            // `prepareRename` (its range or placeholder fills the prompt; `defaultBehavior`: the
            // word at the cursor). No change annotations: edits are applied without asking.
            rename: Some(RenameClientCapabilities {
                prepare_support: Some(true),
                prepare_support_default_behavior: Some(PrepareSupportDefaultBehavior::IDENTIFIER),
                ..Default::default()
            }),
            // No `snippetSupport`: without a snippet session (tab stops) placeholders would stay
            // in the text; a snippet a server sends anyway is read (`completion::strip_snippet`).
            completion: Some(CompletionClientCapabilities {
                completion_item: Some(CompletionItemCapability {
                    commit_characters_support: Some(true),
                    deprecated_support: Some(true),
                    preselect_support: Some(true),
                    insert_replace_support: Some(true),
                    label_details_support: Some(true),
                    resolve_support: Some(CompletionItemCapabilityResolveSupport {
                        properties: vec!["detail".into(), "documentation".into(), "additionalTextEdits".into()],
                    }),
                    ..Default::default()
                }),
                context_support: Some(true),
                completion_list: Some(CompletionListCapability {
                    item_defaults: Some(vec![
                        "editRange".into(),
                        "insertTextFormat".into(),
                        "commitCharacters".into(),
                        "data".into(),
                    ]),
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..Default::default()
        }),
        // Rename's `WorkspaceEdit`s: versioned `documentChanges` welcome; no file operations
        // (create / rename / delete), which red refuses.
        workspace: Some(WorkspaceClientCapabilities {
            workspace_edit: Some(WorkspaceEditClientCapabilities {
                document_changes: Some(true),
                resource_operations: Some(Vec::new()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// UTF-8 if the server chose it (`capabilities.positionEncoding`, or clangd's older
/// `offsetEncoding` extension), else UTF-16: absent means UTF-16, and red offers nothing else.
fn negotiated_encoding(result: &Value) -> PosEncoding {
    let chosen = result
        .pointer("/capabilities/positionEncoding")
        .or_else(|| result.get("offsetEncoding"))
        .and_then(Value::as_str);
    match chosen {
        Some("utf-8") => PosEncoding::Utf8,
        Some("utf-16") | None => PosEncoding::Utf16,
        Some(other) => {
            tracing::warn!(
                encoding = other,
                "server chose an encoding red didn't offer; using UTF-16"
            );
            PosEncoding::Utf16
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings() {
        let enc = |v: Value| negotiated_encoding(&v);
        assert_eq!(
            enc(json!({ "capabilities": { "positionEncoding": "utf-8" } })),
            PosEncoding::Utf8
        );
        assert_eq!(
            enc(json!({ "capabilities": { "positionEncoding": "utf-16" } })),
            PosEncoding::Utf16
        );
        assert_eq!(enc(json!({ "capabilities": {} })), PosEncoding::Utf16);
        assert_eq!(
            enc(json!({ "capabilities": {}, "offsetEncoding": "utf-8" })),
            PosEncoding::Utf8
        );
        assert_eq!(
            enc(json!({ "capabilities": { "positionEncoding": "utf-32" } })),
            PosEncoding::Utf16
        );
    }

    #[test]
    fn programs() {
        let root = Path::new("/ws");
        assert_eq!(program("clangd", root), Path::new("clangd"));
        assert_eq!(program("/usr/bin/clangd", root), Path::new("/usr/bin/clangd"));
        assert_eq!(program(".venv/bin/ty", root), Path::new("/ws/.venv/bin/ty"));
    }

    #[test]
    fn client_offers_utf8_first() {
        let v = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(v["general"]["positionEncodings"], json!(["utf-8", "utf-16"]));
        assert_eq!(v["textDocument"]["publishDiagnostics"]["versionSupport"], json!(true));
        assert!(v["textDocument"].get("diagnostic").is_none(), "no pull diagnostics");
        assert_eq!(v["textDocument"]["rename"]["prepareSupport"], json!(true));
        assert_eq!(v["textDocument"]["rename"]["prepareSupportDefaultBehavior"], json!(1));
        assert_eq!(v["workspace"]["workspaceEdit"]["documentChanges"], json!(true));
        assert_eq!(v["workspace"]["workspaceEdit"]["resourceOperations"], json!([]));
    }
}
