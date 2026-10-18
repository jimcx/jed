//! Language servers: one tokio task per server (`server::Task`), started on the first open of a
//! file in one of its languages, reporting back through a `Sink` (the app turns each `Event` into
//! an `AppEvent::Lsp`). `Manager` is the app's side: which server serves which language, their
//! state, and shutting them all down on quit.

pub mod config;
pub mod diagnostics;
pub mod documents;
#[cfg(test)]
pub mod fake;
pub mod progress;
pub mod rpc;
pub mod scons;
mod server;
pub mod uri;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use lsp_types::ServerCapabilities;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub use config::{Builtins, Resolved, ServerConfig};
use documents::Documents;
use rpc::RpcError;
#[cfg(test)]
pub use server::Transport;
pub use server::{Launcher, ProcessLauncher, SHUTDOWN_TIMEOUT};
use server::{Task, ToServer};

use crate::app::Level;
use crate::buffer::pos::{self, ByteIdx, PosEncoding};
use crate::buffer::{BufferId, Buffers};
use crate::syntax::Language;
use crate::workspace::config::WorkspaceConfig;
use crate::workspace::trust::Trust;

/// How long quitting waits past `SHUTDOWN_TIMEOUT` for the server tasks (which kill their
/// servers at `SHUTDOWN_TIMEOUT`) before aborting them.
const ABORT_MARGIN: Duration = Duration::from_secs(1);

/// Numbers started servers; never reused, so events from a replaced server are dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServerId(u64);

impl ServerId {
    #[cfg(test)]
    pub fn test(n: u64) -> Self {
        Self(n)
    }
}

/// Where server tasks send their events. Called on runtime threads.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    /// The command isn't there (not on `PATH`, or no such file).
    NotFound,
    Spawn(String),
    /// The server answered `initialize` with an error.
    Initialize(String),
}

/// From a server's task to the app.
#[derive(Debug)]
pub enum Event {
    /// `initialize` answered and `initialized` sent: the server takes requests now.
    Initialized {
        server: ServerId,
        encoding: PosEncoding,
        capabilities: Box<ServerCapabilities>,
    },
    /// It never got going (reported once; it isn't restarted).
    Failed { server: ServerId, error: StartError },
    /// It's gone: `crashed` is why, `None` after a requested shutdown.
    Exited { server: ServerId, crashed: Option<String> },
    /// `textDocument/publishDiagnostics`, read (in the server's task).
    Diagnostics {
        server: ServerId,
        published: Box<diagnostics::Published>,
    },
    /// A server notification jed's task doesn't handle itself (`window/logMessage` is logged;
    /// `$/progress` goes to the server's `progress`).
    Notification {
        server: ServerId,
        method: String,
        params: Value,
    },
    /// The answer to `Server::request`'s request `id`.
    Response {
        server: ServerId,
        id: i64,
        result: Result<Value, RpcError>,
    },
}

#[derive(Debug)]
pub enum State {
    Starting,
    Running {
        encoding: PosEncoding,
        capabilities: Box<ServerCapabilities>,
    },
    /// Not found, failed to initialize, or crashed: kept, so it isn't started again.
    Failed,
}

/// The app's handle on one server.
pub struct Server {
    pub id: ServerId,
    pub config: ServerConfig,
    pub root: PathBuf,
    /// Languages it serves (of those opened so far).
    pub languages: Vec<Language>,
    pub state: State,
    /// Its work-done progress (`$/progress`), for the status bar.
    pub progress: progress::Progress,
    tx: mpsc::UnboundedSender<ToServer>,
    next_id: Arc<AtomicI64>,
    /// Its protocol trace is on: every JSON-RPC message in / out also goes to its tool log.
    trace: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl Server {
    /// The negotiated position encoding, once initialized.
    pub fn encoding(&self) -> Option<PosEncoding> {
        match self.state {
            State::Running { encoding, .. } => Some(encoding),
            _ => None,
        }
    }

    /// Sends a request; its answer comes back as `Event::Response` with the returned id. Before
    /// `initialize` is answered it waits in the task. A server that fails or exits first answers
    /// none of its outstanding requests: callers drop them on `Event::Failed` / `Event::Exited`.
    pub fn request(&self, method: &str, params: Value) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(ToServer::Request {
            id,
            method: method.into(),
            params,
        });
        id
    }

    /// Sends a notification (queued in the task until the server is initialized).
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.tx.send(ToServer::Notify {
            method: method.into(),
            params,
        });
    }

    /// Turns the protocol trace (every message to its tool log) on or off ("Tools: Toggle
    /// Protocol Trace").
    pub fn set_trace(&self, on: bool) {
        self.trace.store(on, Ordering::Relaxed);
    }

    /// Whether the protocol trace is on.
    pub fn trace(&self) -> bool {
        self.trace.load(Ordering::Relaxed)
    }

    fn stop(&self) {
        let _ = self.tx.send(ToServer::Shutdown);
    }
}

/// A server that's gone from `Manager::servers`: stopped by `sync` (no language resolves to
/// its config any more: a `.workspace` change, a trust decision) or exited without a crash.
/// Failed ones stay in `servers` (`State::Failed`). Kept so the Tools view still lists it (its
/// log remains); one per name, the latest, dropped when a server of that config starts again.
#[derive(Debug, Clone)]
pub struct Stopped {
    pub config: ServerConfig,
    pub root: PathBuf,
    pub languages: Vec<Language>,
}

/// A server restarted by `Manager::restart`: the instance stopped (none for a stopped server)
/// and the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Restarted {
    pub old: Option<ServerId>,
    pub new: ServerId,
}

/// Remembers `s` as stopped (replacing an older one of its name).
fn note_stopped(stopped: &mut Vec<Stopped>, s: &Server, languages: Vec<Language>) {
    stopped.retain(|o| o.config.name != s.config.name);
    stopped.push(Stopped {
        config: s.config.clone(),
        root: s.root.clone(),
        languages,
    });
}

/// A request `Manager::request_at` sent: its server, id, and the server's position encoding
/// (to read positions in the answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub server: ServerId,
    pub id: i64,
    pub encoding: PosEncoding,
}

/// Something for the status bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub level: Level,
    pub text: String,
}

pub struct Manager {
    /// `None`: servers are off (the default in tests, which plug in a fake or `ProcessLauncher`).
    launcher: Option<Arc<dyn Launcher>>,
    pub builtins: Builtins,
    /// Set by `start`: nothing is spawned before (`App::new` runs outside the runtime).
    sink: Option<Sink>,
    servers: Vec<Server>,
    /// Servers being shut down because their config changed; awaited on quit.
    stopping: Vec<JoinHandle<()>>,
    /// Servers gone from `servers` (stopped or exited cleanly), for the Tools view.
    stopped: Vec<Stopped>,
    next_server: u64,
    /// Buffers open on servers (`textDocument/didOpen` …).
    docs: Documents,
    /// Every file's published diagnostics (open in a buffer or not).
    pub diagnostics: diagnostics::Store,
    /// Servers whose diagnostics were dropped since the last `take_cleared` (they stopped,
    /// failed or crashed): the app drops their marks from the buffers.
    cleared: Vec<ServerId>,
    /// Where jed writes files for servers (`scons`: ty's SCons builtins); `None` (tests'
    /// default): nothing written.
    pub support_dir: Option<PathBuf>,
    /// The tool logs: each server's task writes its stderr, lifecycle and messages to the
    /// ring of its name (`App::start` sets the app's store).
    pub logs: crate::logs::Logs,
}

impl Manager {
    pub fn new(launcher: Option<Arc<dyn Launcher>>) -> Self {
        Self {
            launcher,
            builtins: Builtins::default(),
            sink: None,
            servers: Vec::new(),
            stopping: Vec::new(),
            stopped: Vec::new(),
            next_server: 0,
            docs: Documents::default(),
            diagnostics: diagnostics::Store::default(),
            cleared: Vec::new(),
            support_dir: None,
            logs: crate::logs::Logs::default(),
        }
    }

    /// Sets `support_dir` (`App::new`: `~/.local/state/jed/lsp`).
    #[cfg_attr(test, allow(dead_code))]
    pub fn with_support_dir(mut self, dir: PathBuf) -> Self {
        self.support_dir = Some(dir);
        self
    }

    #[cfg(test)]
    pub fn set_launcher(&mut self, launcher: Arc<dyn Launcher>) {
        self.launcher = Some(launcher);
    }

    /// From `App::start`: servers may be spawned from now on.
    pub fn start(&mut self, sink: Sink) {
        self.sink = Some(sink);
    }

    pub fn started(&self) -> bool {
        self.sink.is_some()
    }

    /// Every server started and not stopped: starting, running, or failed.
    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    /// Servers that stopped (by `sync`) or exited without a crash, latest per name: the Tools
    /// view lists them as exited.
    pub fn stopped(&self) -> &[Stopped] {
        &self.stopped
    }

    /// The server for `language`, in any state.
    pub fn server_for(&self, language: Language) -> Option<&Server> {
        self.servers.iter().find(|s| s.languages.contains(&language))
    }

    /// The live server (starting, running or failed) whose config is named `name`.
    pub fn server_named(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.config.name == name)
    }

    /// Restarts the server named `name`: a live one (running or failed) is stopped and removed,
    /// a stopped one dropped from `stopped`; then a new task starts with the same config, root
    /// and languages. Failed servers are no longer remembered as failed, so this is the one way
    /// back for them (`sync` never retries a failure). The old instance's diagnostics are
    /// dropped (the new one publishes its own), its trace setting carries over, and the log ring
    /// stays (it belongs to the name). `None` when no server by that name is known (or servers
    /// are off).
    pub fn restart(&mut self, name: &str) -> Option<Restarted> {
        if self.sink.is_none() || self.launcher.is_none() {
            return None;
        }
        let (config, root, languages, trace, old) =
            if let Some(i) = self.servers.iter().position(|s| s.config.name == name) {
                let s = self.servers.remove(i);
                let trace = s.trace();
                s.stop();
                if self.diagnostics.clear_server(s.id) {
                    self.cleared.push(s.id);
                }
                self.stopping.push(s.task);
                (s.config, s.root, s.languages, trace, Some(s.id))
            } else {
                let i = self.stopped.iter().position(|s| s.config.name == name)?;
                let s = self.stopped.remove(i);
                (s.config, s.root, s.languages, false, None)
            };
        self.stopping.retain(|t| !t.is_finished());
        let new = self.launch(config, root, languages, trace)?;
        Some(Restarted { old, new })
    }

    /// Starts a server task for `config` at `root`, serving `languages`, with the protocol
    /// trace `trace`, and records it. `None` before `start` or with servers off (no launcher).
    fn launch(
        &mut self,
        config: ServerConfig,
        root: PathBuf,
        languages: Vec<Language>,
        trace: bool,
    ) -> Option<ServerId> {
        let (Some(sink), Some(launcher)) = (self.sink.clone(), self.launcher.clone()) else {
            return None;
        };
        self.stopped.retain(|s| s.config != config);
        self.next_server += 1;
        let id = ServerId(self.next_server);
        let (tx, rx) = mpsc::unbounded_channel();
        let next_id = Arc::new(AtomicI64::new(1));
        let trace = Arc::new(AtomicBool::new(trace));
        let language = languages.first().map_or("…", |l| l.id());
        tracing::info!(server = %config.name, language, "starting language server");
        let task = tokio::spawn(
            Task {
                id,
                config: config.clone(),
                root: root.clone(),
                launcher,
                rx,
                sink,
                next_id: next_id.clone(),
                support_dir: self.support_dir.clone(),
                log: self.logs.tool(crate::logs::Tool::Server(config.name.clone())),
                trace: trace.clone(),
            }
            .run(),
        );
        self.servers.push(Server {
            id,
            config,
            root,
            languages,
            state: State::Starting,
            progress: progress::Progress::default(),
            tx,
            next_id,
            trace,
            task,
        });
        Some(id)
    }

    /// Brings servers in line with the open files' languages and the config: starts the one
    /// each language resolves to (once: a failed one stays failed), shuts down any no language
    /// resolves to any more (a `.workspace` change, a trust decision).
    pub fn sync(&mut self, root: &Path, languages: &[Language], workspace: &WorkspaceConfig, trust: Trust) {
        if self.sink.is_none() || self.launcher.is_none() {
            return;
        }
        // A clone, so `resolve` doesn't borrow `self` while a server starts below.
        let builtins = self.builtins.clone();
        let resolve = |l: Language| config::resolve(l, workspace, trust, &builtins);
        let mut kept = Vec::with_capacity(self.servers.len());
        for mut s in std::mem::take(&mut self.servers) {
            let languages = s.languages.clone();
            s.languages
                .retain(|&l| s.root == root && resolve(l) == Resolved::Server(s.config.clone()));
            if s.languages.is_empty() {
                tracing::info!(server = %s.config.name, "language server no longer configured: stopping it");
                s.stop();
                note_stopped(&mut self.stopped, &s, languages);
                if self.diagnostics.clear_server(s.id) {
                    self.cleared.push(s.id);
                }
                self.stopping.push(s.task);
            } else {
                kept.push(s);
            }
        }
        self.servers = kept;
        self.stopping.retain(|t| !t.is_finished());

        for &language in languages {
            let Resolved::Server(config) = resolve(language) else {
                continue;
            };
            if let Some(s) = self.servers.iter_mut().find(|s| s.config == config) {
                if !s.languages.contains(&language) {
                    s.languages.push(language);
                }
                continue;
            }
            self.launch(config, root.to_path_buf(), vec![language], false);
        }
    }

    /// Opens, closes and updates the servers' documents to match the buffers (once per loop
    /// turn: `documents::Documents::sync`).
    pub fn sync_documents(&mut self, buffers: &mut Buffers) {
        self.docs.sync(&self.servers, buffers);
    }

    /// `buffer` was saved: `didSave` to its server, if it has it open and wants it.
    pub fn saved(&mut self, buffers: &mut Buffers, buffer: BufferId) {
        self.docs.saved(&self.servers, buffers, buffer);
    }

    /// The document version a server has for `buffer`, if open.
    #[cfg_attr(not(test), allow(dead_code))] // tests
    pub fn version(&self, buffer: BufferId) -> Option<i32> {
        self.docs.version(buffer)
    }

    /// Sends `method` about position `at` of `buffer` (`textDocument` + `position`, plus the
    /// fields of `extra` if it's an object) to the server that has the buffer's document open,
    /// once this turn's changes went out (`sync_documents` first, so the position is in the text
    /// the server has). `None` when no running server has it open, or `supported` says the
    /// server lacks the feature. The answer comes as `Event::Response` with `Sent::id`.
    pub fn request_at(
        &mut self,
        buffers: &mut Buffers,
        buffer: BufferId,
        at: ByteIdx,
        method: &str,
        supported: fn(&ServerCapabilities) -> bool,
        extra: Value,
    ) -> Option<Sent> {
        self.request_about(buffers, buffer, Some(at), method, supported, extra)
    }

    /// `request_at` without a position: `textDocument` (+ `extra`'s fields) only
    /// (`textDocument/formatting`).
    pub fn request_doc(
        &mut self,
        buffers: &mut Buffers,
        buffer: BufferId,
        method: &str,
        supported: fn(&ServerCapabilities) -> bool,
        extra: Value,
    ) -> Option<Sent> {
        self.request_about(buffers, buffer, None, method, supported, extra)
    }

    fn request_about(
        &mut self,
        buffers: &mut Buffers,
        buffer: BufferId,
        at: Option<ByteIdx>,
        method: &str,
        supported: fn(&ServerCapabilities) -> bool,
        extra: Value,
    ) -> Option<Sent> {
        self.docs.sync(&self.servers, buffers);
        let (server, uri, version) = self.docs.document(buffer)?;
        let s = self.servers.iter().find(|s| s.id == server)?;
        let State::Running { encoding, capabilities } = &s.state else {
            return None;
        };
        if !supported(capabilities) {
            tracing::debug!(server = %s.config.name, method, "not supported by the server");
            return None;
        }
        let mut params = json!({ "textDocument": { "uri": uri.as_str() } });
        if let Some(at) = at {
            let rope = &buffers.get(buffer)?.rope;
            let p = pos::lsp_pos(rope, ByteIdx(at.0.min(rope.len())), *encoding);
            params["position"] = json!({ "line": p.line, "character": p.character });
        }
        if let Value::Object(fields) = extra {
            params.as_object_mut().expect("object").extend(fields);
        }
        let id = s.request(method, params);
        tracing::debug!(server = %s.config.name, method, id, version, "request");
        Some(Sent {
            server,
            id,
            encoding: *encoding,
        })
    }

    /// The server that has `buffer`'s document open, if it's running: its id, position encoding
    /// and capabilities.
    pub fn server_of(&self, buffer: BufferId) -> Option<(ServerId, PosEncoding, &ServerCapabilities)> {
        let (server, ..) = self.docs.document(buffer)?;
        let s = self.servers.iter().find(|s| s.id == server)?;
        match &s.state {
            State::Running { encoding, capabilities } => Some((server, *encoding, capabilities)),
            _ => None,
        }
    }

    /// Sends `method` with `params` as they are to `server` if it's running (a request not about
    /// a position: `completionItem/resolve`). The answer comes as `Event::Response` with the id.
    pub fn request(&self, server: ServerId, method: &str, params: Value) -> Option<i64> {
        let s = self.servers.iter().find(|s| s.id == server)?;
        if !matches!(s.state, State::Running { .. }) {
            return None;
        }
        let id = s.request(method, params);
        tracing::debug!(server = %s.config.name, method, id, "request");
        Some(id)
    }

    /// Tells a server the app no longer waits for request `id` (`$/cancelRequest`); its answer,
    /// if one still comes, is dropped as nobody's.
    pub fn cancel(&self, server: ServerId, id: i64) {
        if let Some(s) = self.servers.iter().find(|s| s.id == server) {
            tracing::debug!(server = %s.config.name, id, "cancelling request");
            s.notify("$/cancelRequest", json!({ "id": id }));
        }
    }

    /// A server's `textDocument/publishDiagnostics`: stored by path, replacing that server's
    /// earlier set, and placed in the file's buffer if open (its marks replaced): read against
    /// the document version it names (`Documents::since`, after `sync_documents` sends this turn's
    /// changes so the newest version is the buffer's text) and mapped through the edits since;
    /// without a version (or one no longer kept), against the current text. Dropped when the
    /// server isn't running (stopped, replaced, failed). True if it was taken.
    pub fn publish(&mut self, server: ServerId, published: diagnostics::Published, buffers: &mut Buffers) -> bool {
        let Some(encoding) = self.servers.iter().find(|s| s.id == server).and_then(Server::encoding) else {
            tracing::debug!("diagnostics from a language server that isn't running");
            return false;
        };
        if let Some(id) = buffers.find_path(&published.path) {
            self.docs.sync(&self.servers, buffers);
            let kept = published.version.and_then(|v| self.docs.since(id, server, v));
            if kept.is_none() && published.version.is_some() {
                tracing::debug!(version = published.version, path = %published.path.display(), "diagnostics for a version no longer kept: read against the current text");
            }
            let buf = buffers.get_mut(id).expect("found");
            let marks = match &kept {
                Some((text, since)) => diagnostics::resolve(server, &published.items, text, encoding, since),
                None => diagnostics::resolve(server, &published.items, &buf.rope, encoding, &[]),
            };
            buf.diagnostics.replace(server, marks);
        }
        self.diagnostics.publish(server, encoding, published);
        true
    }

    /// Places the stored diagnostics of `buffer`'s file in it (just opened: read against its
    /// text, which is what the server saw unless the file changed since).
    pub fn load_diagnostics(&self, buffer: &mut crate::buffer::Buffer) {
        for set in self.diagnostics.get(&buffer.path) {
            let marks = diagnostics::resolve(set.server, &set.items, &buffer.rope, set.encoding, &[]);
            buffer.diagnostics.replace(set.server, marks);
        }
    }

    /// Servers whose diagnostics were dropped since the last call.
    pub fn take_cleared(&mut self) -> Vec<ServerId> {
        std::mem::take(&mut self.cleared)
    }

    /// Takes in a server task's event; returns what the status bar should say, if anything.
    pub fn handle(&mut self, event: Event) -> Option<Notice> {
        let id = match &event {
            Event::Initialized { server, .. }
            | Event::Failed { server, .. }
            | Event::Exited { server, .. }
            | Event::Notification { server, .. }
            | Event::Diagnostics { server, .. }
            | Event::Response { server, .. } => *server,
        };
        let Some(index) = self.servers.iter().position(|s| s.id == id) else {
            tracing::debug!(?event, "event from a stopped language server");
            return None;
        };
        if matches!(event, Event::Failed { .. } | Event::Exited { .. }) && self.diagnostics.clear_server(id) {
            self.cleared.push(id);
        }
        let s = &mut self.servers[index];
        let name = s.config.name.clone();
        let error = |text: String| {
            Some(Notice {
                level: Level::Error,
                text,
            })
        };
        match event {
            Event::Initialized {
                encoding, capabilities, ..
            } => {
                s.state = State::Running { encoding, capabilities };
                None
            }
            Event::Failed { error: e, .. } => {
                s.state = State::Failed;
                s.progress.clear();
                let section = s.languages.first().map_or("…", |l| l.id());
                error(match e {
                    StartError::NotFound if s.config.from_workspace => format!(
                        "Language server not found: {} (.workspace [lsp.{section}])",
                        s.config.command
                    ),
                    StartError::NotFound => format!(
                        "Language server {name} not found: install it, or set [lsp.{section}] command in .workspace"
                    ),
                    StartError::Spawn(e) => format!("Can't start language server {name}: {e}"),
                    StartError::Initialize(e) => format!("Language server {name} failed to initialize: {e}"),
                })
            }
            Event::Exited { crashed: Some(why), .. } => {
                s.state = State::Failed;
                s.progress.clear();
                error(format!("Language server {name} stopped ({why}); not restarted"))
            }
            Event::Exited { crashed: None, .. } => {
                let s = self.servers.remove(index);
                let languages = s.languages.clone();
                note_stopped(&mut self.stopped, &s, languages);
                None
            }
            Event::Notification { method, params, .. } if method == "window/showMessage" => {
                let text = params.get("message").and_then(Value::as_str).unwrap_or("").to_string();
                let level = match params.get("type").and_then(Value::as_i64) {
                    Some(1) => Level::Error,
                    Some(2) => Level::Warning,
                    _ => Level::Info,
                };
                Some(Notice {
                    level,
                    text: format!("{name}: {text}"),
                })
            }
            Event::Notification { method, params, .. } if method == "$/progress" => {
                s.progress.update(&params);
                None
            }
            Event::Notification { method, .. } => {
                tracing::debug!(server = %name, method, "unhandled server notification");
                None
            }
            Event::Response { id, .. } => {
                tracing::debug!(server = %name, id, "response nobody waits for");
                None
            }
            // The app hands these to `publish` (it needs the buffers).
            Event::Diagnostics { .. } => None,
        }
    }

    /// What the status bar says about servers at work: the latest unfinished `$/progress` of the
    /// first running server with one, e.g. `rust-analyzer: Indexing 40% 3/25 (core)`.
    pub fn progress(&self) -> Option<String> {
        self.servers
            .iter()
            .filter(|s| matches!(s.state, State::Running { .. }))
            .find_map(|s| s.progress.latest().map(|w| format!("{}: {}", s.config.name, w.label())))
    }

    /// Asks every server to shut down and waits for them to exit, at most `SHUTDOWN_TIMEOUT`
    /// plus a little (each task kills its server after that); then gives up on the stragglers
    /// (aborting a task drops its process, which kills it). The future borrows nothing.
    pub fn shutdown(&mut self) -> impl Future<Output = ()> + 'static {
        let stopped = self.shutdown_within(SHUTDOWN_TIMEOUT + ABORT_MARGIN);
        async move {
            stopped.await;
        }
    }

    /// `shutdown`, aborting the server tasks still running after `limit`; true if every task
    /// ended by itself (its server exited, or the task's own `SHUTDOWN_TIMEOUT` killed it).
    pub fn shutdown_within(&mut self, limit: Duration) -> impl Future<Output = bool> + 'static {
        self.sink = None;
        let mut tasks = std::mem::take(&mut self.stopping);
        for s in self.servers.drain(..) {
            s.stop();
            tasks.push(s.task);
        }
        let aborts: Vec<_> = tasks.iter().map(JoinHandle::abort_handle).collect();
        async move {
            if tasks.is_empty() {
                return true;
            }
            let all = futures_util::future::join_all(tasks);
            if tokio::time::timeout(limit, all).await.is_err() {
                tracing::warn!("language servers didn't stop in time");
                aborts.iter().for_each(|a| a.abort());
                return false;
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeLauncher, FakeOptions};
    use super::*;
    use serde_json::json;

    /// A manager started with `fake`, and the channel its events arrive on.
    fn manager(fake: &Arc<FakeLauncher>) -> (Manager, mpsc::UnboundedReceiver<Event>) {
        let mut m = Manager::new(Some(fake.clone()));
        let (tx, rx) = mpsc::unbounded_channel();
        m.start(Arc::new(move |e| {
            let _ = tx.send(e);
        }));
        (m, rx)
    }

    async fn next(rx: &mut mpsc::UnboundedReceiver<Event>) -> Event {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("an event within 5 s")
            .expect("channel open")
    }

    fn sync(m: &mut Manager, languages: &[Language]) {
        m.sync(
            Path::new("/ws"),
            languages,
            &WorkspaceConfig::default(),
            Trust::NotNeeded,
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn responses_are_routed_by_id() {
        let fake = FakeLauncher::new(FakeOptions::default());
        let (mut m, mut rx) = manager(&fake);
        sync(&mut m, &[Language::Cpp, Language::C]);
        assert_eq!(m.servers().len(), 1, "C and C++ share clangd");
        let s = m.server_for(Language::C).unwrap();
        // Sent before `initialize` is answered: they wait in the task.
        s.notify("test/note", json!({ "n": 0 }));
        let a = s.request("test/later", json!("a"));
        let b = s.request("test/later", json!("b"));
        let c = s.request("test/echo", json!("c"));
        assert!(matches!(next(&mut rx).await, Event::Initialized { .. }));
        s.notify("test/flush", Value::Null); // answers the held ones, newest first
        let mut answers = Vec::new();
        for _ in 0..3 {
            match next(&mut rx).await {
                Event::Response { id, result, .. } => answers.push((id, result.unwrap())),
                e => panic!("{e:?}"),
            }
        }
        assert_eq!(answers, vec![(c, json!("c")), (b, json!("b")), (a, json!("a"))]);
        let s = m.server_for(Language::C).unwrap();
        let unknown = s.request("test/unknown", Value::Null);
        match next(&mut rx).await {
            Event::Response { id, result, .. } => {
                assert_eq!(id, unknown);
                assert_eq!(result.unwrap_err().code, rpc::METHOD_NOT_FOUND);
            }
            e => panic!("{e:?}"),
        }
        let methods = fake.methods();
        let pos = |m: &str| methods.iter().position(|x| x == m).unwrap();
        assert!(pos("initialized") < pos("test/note"), "{methods:?}");
        // The fake asked us things during `initialize`; each got an answer.
        let answers = fake.answers();
        assert_eq!(answers["cfg"], json!([null, null]));
        assert_eq!(answers["progress"], Value::Null);
        assert_eq!(answers["bogus"]["error"]["code"], json!(rpc::METHOD_NOT_FOUND));
        m.shutdown().await;
        assert!(fake.methods().ends_with(&["shutdown".to_string(), "exit".to_string()]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failures_are_reported_once_and_not_retried() {
        let fake = FakeLauncher::new(FakeOptions {
            missing: vec!["ty".into()],
            crash_after_init: true,
            ..Default::default()
        });
        let (mut m, mut rx) = manager(&fake);
        sync(&mut m, &[Language::Python]);
        let notice = m.handle(next(&mut rx).await).unwrap();
        assert!(notice.text.contains("Language server ty not found"), "{notice:?}");
        sync(&mut m, &[Language::Python, Language::Cpp]);
        assert!(m.handle(next(&mut rx).await).is_none(), "clangd initialized");
        let notice = m.handle(next(&mut rx).await).unwrap();
        assert!(notice.text.contains("clangd stopped (exited"), "{notice:?}");
        sync(&mut m, &[Language::Python, Language::Cpp, Language::C]);
        assert_eq!(fake.launches(), ["ty", "clangd"], "neither started again");
        assert!(m.servers().iter().all(|s| matches!(s.state, State::Failed)));
        m.shutdown().await;
    }

    /// A server `sync` stopped (its `.workspace` config can't run while trust is undecided)
    /// starts again through `launch` once it can, and its stopped row (the Tools view's
    /// "exited") goes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_server_starts_again_when_its_config_does() {
        let fake = FakeLauncher::new(FakeOptions::default());
        let (mut m, mut rx) = manager(&fake);
        let ws = WorkspaceConfig::parse("[lsp.python]\ncommand = \"pyright\"").unwrap();
        let root = Path::new("/ws");
        m.sync(root, &[Language::Python], &ws, Trust::Trusted);
        m.handle(next(&mut rx).await);
        m.sync(root, &[Language::Python], &ws, Trust::Undecided);
        assert_eq!(m.stopped().len(), 1);
        m.sync(root, &[Language::Python], &ws, Trust::Trusted);
        assert!(m.stopped().is_empty(), "the stopped row goes with the new instance");
        assert_eq!(m.server_for(Language::Python).unwrap().config.name, "pyright");
        m.shutdown().await;
    }

    /// "Tools: Restart Server": a failed server (which `sync` never retries) is started again
    /// under a new id; a name no server has restarts nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn restarting_a_failed_server_starts_it_again() {
        let fake = FakeLauncher::new(FakeOptions {
            crash_after_init: true,
            ..Default::default()
        });
        let (mut m, mut rx) = manager(&fake);
        sync(&mut m, &[Language::Cpp]);
        assert!(matches!(next(&mut rx).await, Event::Initialized { .. }));
        let notice = m.handle(next(&mut rx).await).unwrap();
        assert!(notice.text.contains("stopped"), "{notice:?}");
        assert!(matches!(m.servers()[0].state, State::Failed));
        let old = m.servers()[0].id;

        assert!(m.restart("nope").is_none(), "no server by that name");
        assert!(Manager::new(None).restart("clangd").is_none(), "servers off");
        let restarted = m.restart("clangd").expect("a server by that name");
        assert_eq!(restarted.old, Some(old));
        assert_ne!(restarted.new, old, "a new instance");
        assert!(matches!(m.servers()[0].state, State::Starting));
        match next(&mut rx).await {
            Event::Initialized { server, .. } => assert_eq!(server, restarted.new),
            e => panic!("{e:?}"),
        }
        assert_eq!(fake.launches(), ["clangd", "clangd"]);
        m.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_initialize_error_is_reported_once() {
        let fake = FakeLauncher::new(FakeOptions {
            init_error: true,
            ..Default::default()
        });
        let (mut m, mut rx) = manager(&fake);
        sync(&mut m, &[Language::Cpp]);
        let notice = m.handle(next(&mut rx).await).unwrap();
        assert_eq!(notice.level, Level::Error);
        assert_eq!(
            notice.text,
            "Language server clangd failed to initialize: fake can't start"
        );
        assert!(matches!(m.servers()[0].state, State::Failed));
        // Nothing more from it (no `Exited` crash on top), and it isn't started again.
        sync(&mut m, &[Language::Cpp, Language::C]);
        let more = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(!matches!(more, Ok(Some(_))), "{more:?}");
        assert_eq!(fake.launches(), ["clangd"]);
        assert_eq!(fake.methods(), ["initialize"], "no `initialized` after an error");
        m.shutdown().await;
    }

    /// Before `start` (as in `App::new`, which may run outside the runtime) nothing is spawned.
    #[test]
    fn nothing_starts_before_start() {
        let fake = FakeLauncher::new(FakeOptions::default());
        let mut m = Manager::new(Some(fake.clone()));
        sync(&mut m, &[Language::Cpp]); // `tokio::spawn` here would panic: no runtime
        assert!(m.servers().is_empty());
        assert!(fake.launches().is_empty());
    }

    /// A publish is stored and placed in the file's buffer; a server that's stopped (here: no
    /// longer configured) takes its diagnostics along, and later publishes from it are dropped.
    #[tokio::test(flavor = "multi_thread")]
    async fn diagnostics_go_with_their_server() {
        let fake = FakeLauncher::new(FakeOptions::default());
        let (mut m, mut rx) = manager(&fake);
        sync(&mut m, &[Language::Python]);
        m.handle(next(&mut rx).await);
        let id = m.server_for(Language::Python).unwrap().id;
        let published = || {
            diagnostics::parse(json!({
                "uri": "file:///ws/a.py",
                "diagnostics": [{
                    "range": { "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 7 } },
                    "message": "m",
                }],
            }))
            .unwrap()
        };
        let mut buffers = Buffers::default();
        let mut buf = crate::buffer::Buffer::from_str("x = abc\n");
        buf.path = PathBuf::from("/ws/a.py");
        let b = buffers.insert(buf);
        assert!(m.publish(id, published(), &mut buffers));
        let marks = buffers.get(b).unwrap().diagnostics.all();
        assert_eq!((marks.len(), marks[0].start.0, marks[0].end.0), (1, 4, 7));
        assert_eq!(m.diagnostics.files().count(), 1);
        assert!(m.take_cleared().is_empty());

        let ws = WorkspaceConfig::parse("[lsp.python]\ncommand = \"pyright\"").unwrap();
        m.sync(Path::new("/ws"), &[Language::Python], &ws, Trust::Trusted);
        assert_eq!(m.take_cleared(), [id]);
        assert_eq!(m.diagnostics.files().count(), 0);
        assert!(!m.publish(id, published(), &mut buffers), "from a stopped server");
        assert_eq!(m.diagnostics.files().count(), 0);
        m.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_no_longer_configured_is_stopped() {
        let fake = FakeLauncher::new(FakeOptions::default());
        let (mut m, mut rx) = manager(&fake);
        sync(&mut m, &[Language::Python]);
        m.handle(next(&mut rx).await);
        let ws = WorkspaceConfig::parse("[lsp.python]\ncommand = \"pyright\"").unwrap();
        m.sync(Path::new("/ws"), &[Language::Python], &ws, Trust::Trusted);
        assert_eq!(m.server_for(Language::Python).unwrap().config.name, "pyright");
        m.shutdown().await;
        assert_eq!(fake.launches(), ["ty", "pyright"]);
        // The second was stopped before it initialized: its fake sees the input close a moment later.
        let t = std::time::Instant::now();
        while fake.exited() < 2 && t.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(fake.exited(), 2, "both shut down");
    }
}
