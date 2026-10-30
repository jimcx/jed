//! One language server: the tokio task that starts it, runs `initialize`, routes messages both
//! ways and shuts it down. A `Launcher` makes the byte streams, so tests plug in a fake server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use lsp_types::{
    ClientCapabilities, ClientInfo, CodeActionCapabilityResolveSupport, CodeActionClientCapabilities,
    CodeActionKindLiteralSupport, CodeActionLiteralSupport, CompletionClientCapabilities, CompletionItemCapability,
    CompletionItemCapabilityResolveSupport, CompletionListCapability, DocumentFormattingClientCapabilities,
    DynamicRegistrationClientCapabilities, GeneralClientCapabilities, GotoCapability, HoverClientCapabilities,
    InitializeParams, MarkupKind, PositionEncodingKind, PrepareSupportDefaultBehavior,
    PublishDiagnosticsClientCapabilities, ReferenceClientCapabilities, RenameClientCapabilities, SemanticTokenModifier,
    SemanticTokenType, SemanticTokensClientCapabilities, SemanticTokensClientCapabilitiesRequests,
    SemanticTokensFullOptions, SemanticTokensWorkspaceClientCapabilities, ServerCapabilities,
    TextDocumentClientCapabilities, TokenFormat, WindowClientCapabilities, WorkspaceClientCapabilities,
    WorkspaceEditClientCapabilities, WorkspaceFolder,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::config::ServerConfig;
use super::rpc::{Framer, Id, METHOD_NOT_FOUND, Message, RpcError};
use super::{Event, ServerId, Sink, StartError, uri};
use crate::buffer::pos::PosEncoding;
use crate::logs::{Level, ToolLog};

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

    /// Where to find `sourcekit-lsp` (`XCRUN_TOOL`) when it isn't on `PATH`, if anywhere: an
    /// absolute path to launch instead. Blocking: call from spawn_blocking.
    fn locate(&self, _command: &str) -> Option<String> {
        None
    }
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

    fn locate(&self, command: &str) -> Option<String> {
        xcrun_fallback(command, cfg!(target_os = "macos"), on_path, xcrun_find)
    }
}

/// The command and its arguments as one line, for the log (arguments with spaces quoted).
fn command_line(config: &ServerConfig) -> String {
    std::iter::once(&config.command)
        .chain(&config.args)
        .map(|a| crate::recent::shell_quote(a).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The one built-in server that Xcode / the command line tools hide off `PATH`.
pub(crate) const XCRUN_TOOL: &str = "sourcekit-lsp";

/// Where to run `command` when it isn't on `PATH`: only on macOS, only for `sourcekit-lsp`, and
/// only when the PATH lookup (`on_path`) failed, ask `xcrun --find` (`xcrun`). `None`: run it as
/// named (a missing one then gives the usual "couldn't start" message).
fn xcrun_fallback(
    command: &str,
    macos: bool,
    on_path: impl FnOnce(&str) -> bool,
    xcrun: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    if !macos || command != XCRUN_TOOL || on_path(command) {
        return None;
    }
    xcrun(command)
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// `xcrun --find <tool>`. Blocking: call from spawn_blocking.
fn xcrun_find(tool: &str) -> Option<String> {
    let out = std::process::Command::new("xcrun")
        .args(["--find", tool])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    xcrun_path(out.status.success(), &out.stdout)
}

/// What `xcrun --find` printed, if it found something: it succeeded and printed the absolute path
/// of an existing file (trailing newline trimmed).
fn xcrun_path(success: bool, stdout: &[u8]) -> Option<String> {
    let path = std::str::from_utf8(stdout).ok()?.trim();
    (success && Path::new(path).is_absolute() && Path::new(path).is_file()).then(|| path.to_string())
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
    /// The app's answer to a request the server sent (`Event::ServerRequest`).
    Respond {
        id: Id,
        result: Result<Value, RpcError>,
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
    /// Where jed writes files servers read (`Manager::support_dir`): ty's SCons builtins.
    pub support_dir: Option<PathBuf>,
    /// Its tool log: stderr, lifecycle, `window/logMessage` / `showMessage`, the protocol trace.
    pub log: ToolLog,
    /// The protocol trace is on (`Server::set_trace`).
    pub trace: Arc<AtomicBool>,
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
        let mut config = self.config.clone();
        if let Some(dir) = self.support_dir.clone()
            && super::scons::applies(&name)
        {
            let root = self.root.clone();
            if let Ok(Some(stub)) = tokio::task::spawn_blocking(move || super::scons::prepare(&root, &dir)).await {
                tracing::info!(server = %name, stub = %stub.display(), "SCons workspace: builtins stub on ty's search path");
                config.init_options = Some(super::scons::with_extra_path(config.init_options.take(), &stub));
            }
        }
        let log = self.log.clone();
        log.info(&format!(
            "Starting: {} (in {})",
            command_line(&self.config),
            self.root.display()
        ));
        // `sourcekit-lsp` is often not on PATH on macOS (it lives in the Xcode toolchain).
        let mut launch_config = self.config.clone();
        if launch_config.command == XCRUN_TOOL {
            let launcher = self.launcher.clone();
            let found = tokio::task::spawn_blocking(move || launcher.locate(XCRUN_TOOL)).await;
            if let Ok(Some(path)) = found {
                log.info(&format!("Not on PATH: using {path} (from xcrun)"));
                launch_config.command = path;
            }
        }
        let transport = match self.launcher.launch(&launch_config, &self.root) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(server = %name, command = %self.config.command, "can't start language server: {e}");
                let error = if e.kind() == std::io::ErrorKind::NotFound {
                    log.error(&format!("Can't start: {} not found", self.config.command));
                    StartError::NotFound
                } else {
                    log.error(&format!("Can't start: {e}"));
                    StartError::Spawn(e.to_string())
                };
                (self.sink)(Event::Failed { server: self.id, error });
                return;
            }
        };
        tracing::info!(server = %name, command = %self.config.command, root = %self.root.display(), "language server started");
        if let Some(pid) = transport.child.as_ref().and_then(tokio::process::Child::id) {
            log.info(&format!("Started, pid {pid}"));
        }
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
            log: log.clone(),
            trace: self.trace.clone(),
        };
        conn.send_initialize(&config);

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
                            log.info(&line);
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
        let initialized = conn.initialized;
        drop(conn);
        // A server that died before answering `initialize` may still have its reason in the
        // stderr pipe: read what's left (briefly) for the crash message.
        if let (End::Crashed(_), false, Some(lines)) = (&end, initialized, stderr.as_mut()) {
            let rest = async {
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "lsp::stderr", server = %name, "{line}");
                    log.info(&line);
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            };
            let _ = tokio::time::timeout(Duration::from_millis(300), rest).await;
        }
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
                log.info(&format!("Stopped ({})", status.as_deref().unwrap_or("killed")));
                (self.sink)(Event::Exited {
                    server: self.id,
                    crashed: None,
                });
            }
            End::Failed => {}
            End::Crashed(mut why) => {
                // Gone before answering `initialize`: its last words usually say why (a rustup
                // proxy whose toolchain lacks the component: "Unknown binary 'rust-analyzer' …").
                if !initialized && let Some(last) = tail.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty()) {
                    why = format!("{why}: {last}");
                }
                let why = match status {
                    Some(s) => format!("{why}, {s}"),
                    None => why,
                };
                let lines: Vec<String> = tail.into();
                tracing::warn!(server = %name, stderr = %lines.join("\n"), "language server exited: {why}");
                log.error(&format!("Crashed: {why}"));
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
    log: ToolLog,
    /// The protocol trace is on: messages also go to `log`.
    trace: Arc<AtomicBool>,
}

impl Conn {
    fn send(&self, msg: &Message) {
        let body = msg.to_value().to_string();
        tracing::trace!(target: "lsp::wire", server = %self.name, "→ {body}");
        if self.trace.load(Ordering::Relaxed) {
            self.log.push(Level::Trace, &format!("→ {body}"));
        }
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
                name: "jed".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            ..Default::default()
        };
        let mut params = serde_json::to_value(params).unwrap_or(Value::Null);
        // clangd's pre-3.17 extension, for clangd versions without `positionEncodings`.
        params["capabilities"]["offsetEncoding"] = json!(["utf-8", "utf-16"]);
        // clangd's extension (not in lsp_types): it sends `textDocument/inactiveRegions` for code
        // its preprocessor left out (`#if 0`), instead of tokens.
        params["capabilities"]["textDocument"]["inactiveRegionsCapabilities"] = json!({ "inactiveRegions": true });
        self.request("initialize", params, Pending::Initialize);
    }

    /// One body from the server; `Some` ends the task.
    fn incoming(&mut self, body: &[u8]) -> Option<End> {
        tracing::trace!(target: "lsp::wire", server = %self.name, "← {}", String::from_utf8_lossy(body));
        if self.trace.load(Ordering::Relaxed) {
            self.log
                .push(Level::Trace, &format!("← {}", String::from_utf8_lossy(body)));
        }
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
            // The app applies it and answers (`ToServer::Respond`).
            Message::Request { id, method, params } if method == "workspace/applyEdit" => {
                (self.sink)(Event::ServerRequest {
                    server: self.id,
                    id,
                    method,
                    params,
                });
            }
            Message::Request { id, method, params } => {
                let result = self.answer(&method, &params);
                self.send(&Message::Response { id, result });
                // Answered here; the app asks for the tokens again.
                if method == "workspace/semanticTokens/refresh" {
                    (self.sink)(Event::Notification {
                        server: self.id,
                        method,
                        params,
                    });
                }
            }
            Message::Notification { method, params } => match method.as_str() {
                "window/logMessage" => {
                    let text = params.get("message").and_then(Value::as_str).unwrap_or("");
                    let kind = params.get("type").and_then(Value::as_i64);
                    self.log.push(Level::from_lsp(kind), text);
                    match kind {
                        Some(1) => tracing::warn!(server = %self.name, "{text}"),
                        Some(2) | Some(3) => tracing::info!(server = %self.name, "{text}"),
                        _ => tracing::debug!(server = %self.name, "{text}"),
                    }
                }
                // Logged here; the app shows it (`Manager::handle`).
                "window/showMessage" => {
                    let text = params.get("message").and_then(Value::as_str).unwrap_or("");
                    let kind = params.get("type").and_then(Value::as_i64);
                    self.log.push(Level::from_lsp(kind), &format!("Message: {text}"));
                    (self.sink)(Event::Notification {
                        server: self.id,
                        method,
                        params,
                    });
                }
                "$/logTrace" | "telemetry/event" => {}
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
            _ => {
                tracing::debug!(server = %self.name, method, "unhandled server request");
                Err(RpcError {
                    code: METHOD_NOT_FOUND,
                    message: format!("jed doesn't handle {method}"),
                })
            }
        }
    }

    fn initialize_done(&mut self, result: Result<Value, RpcError>) -> Option<End> {
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                self.log.error(&format!("Failed to initialize: {}", e.message));
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
        let info = match value.pointer("/serverInfo/name").and_then(Value::as_str) {
            Some(server) => format!("{server} {version}"),
            None => format!("version {version}"),
        };
        let positions = match encoding {
            PosEncoding::Utf8 => "UTF-8",
            PosEncoding::Utf16 => "UTF-16",
        };
        self.log.info(&format!("Initialized: {info}, positions in {positions}"));
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
            ToServer::Respond { id, result } => self.send(&Message::Response { id, result }),
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

/// What jed tells servers it can do: UTF-8 positions first (UTF-16 is the fallback every server
/// must support), work-done progress, pushed diagnostics (with the document version they're
/// for, so jed can map them through the edits made since; no pull diagnostics: ty and clangd
/// push when the client doesn't declare `textDocument.diagnostic`), hover (markdown first),
/// go to definition (`LocationLink`s welcome), completion, references, rename (with
/// `prepareRename` and versioned `WorkspaceEdit`s), code actions and `workspace/applyEdit`.
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(vec![PositionEncodingKind::UTF8, PositionEncodingKind::UTF16]),
            ..Default::default()
        }),
        text_document: Some(TextDocumentClientCapabilities {
            publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                version_support: Some(true),
                // Kept and sent back with code action requests (`Diagnostic::raw`).
                data_support: Some(true),
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
            // Code actions (Cmd+.): literals of every kind, preferred / disabled / data, and the
            // edit resolved later (`codeAction/resolve`) when the server leaves it out.
            code_action: Some(CodeActionClientCapabilities {
                code_action_literal_support: Some(CodeActionLiteralSupport {
                    code_action_kind: CodeActionKindLiteralSupport {
                        value_set: [
                            "",
                            "quickfix",
                            "refactor",
                            "refactor.extract",
                            "refactor.inline",
                            "refactor.rewrite",
                            "source",
                            "source.organizeImports",
                            "source.fixAll",
                        ]
                        .map(String::from)
                        .to_vec(),
                    },
                }),
                is_preferred_support: Some(true),
                disabled_support: Some(true),
                data_support: Some(true),
                resolve_support: Some(CodeActionCapabilityResolveSupport {
                    properties: vec!["edit".into()],
                }),
                ..Default::default()
            }),
            // Format document (no range / on-type formatting).
            formatting: Some(DocumentFormattingClientCapabilities::default()),
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
            // Semantic tokens: the relative format, whole documents and deltas, no multi-line or
            // overlapping tokens (`semantic::decode` cuts / sorts them anyway).
            semantic_tokens: Some(SemanticTokensClientCapabilities {
                dynamic_registration: None,
                requests: SemanticTokensClientCapabilitiesRequests {
                    range: None,
                    full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                },
                token_types: crate::semantic::Kind::NAMES.map(SemanticTokenType::from).to_vec(),
                token_modifiers: crate::semantic::mods::NAMES.map(SemanticTokenModifier::from).to_vec(),
                formats: vec![TokenFormat::RELATIVE],
                overlapping_token_support: Some(false),
                multiline_token_support: Some(false),
                server_cancel_support: None,
                augments_syntax_tokens: Some(true),
            }),
            ..Default::default()
        }),
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..Default::default()
        }),
        // `WorkspaceEdit`s (rename, code actions, server-sent `workspace/applyEdit`): versioned
        // `documentChanges` welcome; no file operations (create / rename / delete), which jed
        // refuses.
        workspace: Some(WorkspaceClientCapabilities {
            apply_edit: Some(true),
            execute_command: Some(DynamicRegistrationClientCapabilities::default()),
            // `workspace/semanticTokens/refresh`: the tokens are asked for again.
            semantic_tokens: Some(SemanticTokensWorkspaceClientCapabilities {
                refresh_support: Some(true),
            }),
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
/// `offsetEncoding` extension), else UTF-16: absent means UTF-16, and jed offers nothing else.
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
                "server chose an encoding jed didn't offer; using UTF-16"
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
    fn xcrun_finds_sourcekit_lsp_only_on_macos_when_not_on_path() {
        let found = |_: &str| Some("/Xcode/sourcekit-lsp".to_string());
        let find = |cmd, macos, path| xcrun_fallback(cmd, macos, |_| path, found);
        assert_eq!(
            find("sourcekit-lsp", true, false).as_deref(),
            Some("/Xcode/sourcekit-lsp")
        );
        assert_eq!(find("sourcekit-lsp", true, true), None, "on PATH: run as named");
        assert_eq!(find("sourcekit-lsp", false, false), None, "not macOS");
        assert_eq!(find("clangd", true, false), None, "other servers");
        assert_eq!(
            xcrun_fallback("sourcekit-lsp", true, |_| false, |_| None),
            None,
            "xcrun can't find it"
        );
    }

    #[test]
    fn xcrun_output_is_used_only_when_it_names_an_existing_absolute_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("sourcekit-lsp");
        std::fs::write(&tool, "").unwrap();
        let tool = tool.to_str().unwrap();
        assert_eq!(xcrun_path(true, format!("{tool}\n").as_bytes()).as_deref(), Some(tool));
        assert_eq!(xcrun_path(false, format!("{tool}\n").as_bytes()), None, "xcrun failed");
        assert_eq!(xcrun_path(true, b""), None, "nothing printed");
        assert_eq!(xcrun_path(true, b"  \n"), None, "only whitespace");
        assert_eq!(xcrun_path(true, b"sourcekit-lsp\n"), None, "not absolute");
        let gone = dir.path().join("gone");
        assert_eq!(
            xcrun_path(true, gone.to_str().unwrap().as_bytes()),
            None,
            "no such file"
        );
        assert_eq!(
            xcrun_path(true, dir.path().to_str().unwrap().as_bytes()),
            None,
            "a directory"
        );
        // No `xcrun` at all (not macOS, or no developer tools): nothing, and no error.
        if which_xcrun().is_none() {
            assert_eq!(xcrun_find("sourcekit-lsp"), None);
        }
    }

    fn which_xcrun() -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join("xcrun"))
            .find(|p| p.is_file())
    }

    #[test]
    fn process_launcher_locates_only_sourcekit_lsp() {
        // Off macOS (or for another command) it never asks xcrun.
        if !cfg!(target_os = "macos") {
            assert_eq!(ProcessLauncher.locate("sourcekit-lsp"), None);
        }
        assert_eq!(ProcessLauncher.locate("clangd"), None);
        assert_eq!(ProcessLauncher.locate("/opt/swift/bin/sourcekit-lsp"), None);
    }

    #[test]
    fn client_offers_utf8_first() {
        let v = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(v["general"]["positionEncodings"], json!(["utf-8", "utf-16"]));
        assert_eq!(v["textDocument"]["publishDiagnostics"]["versionSupport"], json!(true));
        assert!(v["textDocument"].get("diagnostic").is_none(), "no pull diagnostics");
        assert_eq!(v["window"]["workDoneProgress"], json!(true));
        assert_eq!(v["textDocument"]["rename"]["prepareSupport"], json!(true));
        assert_eq!(v["textDocument"]["rename"]["prepareSupportDefaultBehavior"], json!(1));
        assert_eq!(v["workspace"]["workspaceEdit"]["documentChanges"], json!(true));
        assert_eq!(v["workspace"]["workspaceEdit"]["resourceOperations"], json!([]));
        let st = &v["textDocument"]["semanticTokens"];
        assert_eq!(st["formats"], json!(["relative"]));
        assert_eq!(st["requests"]["full"], json!({ "delta": true }));
        assert!(st["requests"].get("range").is_none());
        assert_eq!(st["overlappingTokenSupport"], json!(false));
        assert_eq!(st["multilineTokenSupport"], json!(false));
        assert_eq!(st["tokenTypes"][0], json!("namespace"));
        assert!(
            st["tokenModifiers"]
                .as_array()
                .unwrap()
                .contains(&json!("constructorOrDestructor"))
        );
        assert_eq!(v["workspace"]["semanticTokens"]["refreshSupport"], json!(true));
    }
}
