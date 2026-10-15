//! Language servers: one tokio task per server (`server::Task`), started on the first open of a
//! file in one of its languages, reporting back through a `Sink` (the app turns each `Event` into
//! an `AppEvent::Lsp`). `Manager` is the app's side: which server serves which language, their
//! state, and shutting them all down on quit.

pub mod config;
pub mod documents;
#[cfg(test)]
pub mod fake;
pub mod rpc;
mod server;
pub mod uri;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use lsp_types::ServerCapabilities;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub use config::{Builtins, Resolved, ServerConfig};
use documents::Documents;
use rpc::RpcError;
pub use server::{Launcher, ProcessLauncher, SHUTDOWN_TIMEOUT};
use server::{Task, ToServer};

use crate::app::Level;
use crate::buffer::pos::PosEncoding;
use crate::buffer::{BufferId, Buffers};
use crate::syntax::Language;
use crate::workspace::config::WorkspaceConfig;
use crate::workspace::trust::Trust;

/// Numbers started servers; never reused, so events from a replaced server are dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServerId(u64);

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
    /// A server notification red's task doesn't handle itself (`window/logMessage` is logged,
    /// `$/progress` dropped).
    Notification {
        server: ServerId,
        method: String,
        params: Value,
    },
    /// The answer to `Server::request`'s request `id`.
    #[cfg_attr(not(test), allow(dead_code))] // read from hover / definition (9e) on
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
    tx: mpsc::UnboundedSender<ToServer>,
    next_id: Arc<AtomicI64>,
    task: JoinHandle<()>,
}

impl Server {
    /// The negotiated position encoding, once initialized.
    #[cfg_attr(not(test), allow(dead_code))] // tests; requests from 9c on
    pub fn encoding(&self) -> Option<PosEncoding> {
        match self.state {
            State::Running { encoding, .. } => Some(encoding),
            _ => None,
        }
    }

    /// Sends a request; its answer comes back as `Event::Response` with the returned id. Before
    /// `initialize` is answered it waits in the task. A server that fails or exits first answers
    /// none of its outstanding requests: callers drop them on `Event::Failed` / `Event::Exited`.
    #[cfg_attr(not(test), allow(dead_code))] // first used by hover / definition (9e)
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

    fn stop(&self) {
        let _ = self.tx.send(ToServer::Shutdown);
    }
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
    next_server: u64,
    /// Buffers open on servers (`textDocument/didOpen` …).
    docs: Documents,
}

impl Manager {
    pub fn new(launcher: Option<Arc<dyn Launcher>>) -> Self {
        Self {
            launcher,
            builtins: Builtins::default(),
            sink: None,
            servers: Vec::new(),
            stopping: Vec::new(),
            next_server: 0,
            docs: Documents::default(),
        }
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

    #[cfg_attr(not(test), allow(dead_code))] // tests; the status bar later
    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    /// The server for `language`, in any state.
    #[cfg_attr(not(test), allow(dead_code))] // tests; requests from 9c on
    pub fn server_for(&self, language: Language) -> Option<&Server> {
        self.servers.iter().find(|s| s.languages.contains(&language))
    }

    /// Brings servers in line with the open files' languages and the config: starts the one
    /// each language resolves to (once: a failed one stays failed), shuts down any no language
    /// resolves to any more (a `.workspace` change, a trust decision).
    pub fn sync(&mut self, root: &Path, languages: &[Language], workspace: &WorkspaceConfig, trust: Trust) {
        let (Some(sink), Some(launcher)) = (self.sink.clone(), self.launcher.clone()) else {
            return;
        };
        let resolve = |l: Language| config::resolve(l, workspace, trust, &self.builtins);
        let mut kept = Vec::with_capacity(self.servers.len());
        for mut s in std::mem::take(&mut self.servers) {
            s.languages
                .retain(|&l| s.root == root && resolve(l) == Resolved::Server(s.config.clone()));
            if s.languages.is_empty() {
                tracing::info!(server = %s.config.name, "language server no longer configured: stopping it");
                s.stop();
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
            self.next_server += 1;
            let id = ServerId(self.next_server);
            let (tx, rx) = mpsc::unbounded_channel();
            let next_id = Arc::new(AtomicI64::new(1));
            tracing::info!(server = %config.name, language = language.id(), "starting language server");
            let task = tokio::spawn(
                Task {
                    id,
                    config: config.clone(),
                    root: root.to_path_buf(),
                    launcher: launcher.clone(),
                    rx,
                    sink: sink.clone(),
                    next_id: next_id.clone(),
                }
                .run(),
            );
            self.servers.push(Server {
                id,
                config,
                root: root.to_path_buf(),
                languages: vec![language],
                state: State::Starting,
                tx,
                next_id,
                task,
            });
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
    #[cfg_attr(not(test), allow(dead_code))] // tests; requests from 9c on
    pub fn version(&self, buffer: BufferId) -> Option<i32> {
        self.docs.version(buffer)
    }

    /// Takes in a server task's event; returns what the status bar should say, if anything.
    pub fn handle(&mut self, event: Event) -> Option<Notice> {
        let id = match &event {
            Event::Initialized { server, .. }
            | Event::Failed { server, .. }
            | Event::Exited { server, .. }
            | Event::Notification { server, .. }
            | Event::Response { server, .. } => *server,
        };
        let Some(index) = self.servers.iter().position(|s| s.id == id) else {
            tracing::debug!(?event, "event from a stopped language server");
            return None;
        };
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
                error(format!("Language server {name} stopped ({why}); not restarted"))
            }
            Event::Exited { crashed: None, .. } => {
                self.servers.remove(index);
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
            Event::Notification { method, .. } => {
                tracing::debug!(server = %name, method, "unhandled server notification");
                None
            }
            Event::Response { id, .. } => {
                tracing::debug!(server = %name, id, "response nobody waits for");
                None
            }
        }
    }

    /// Asks every server to shut down and waits for them to exit, at most `SHUTDOWN_TIMEOUT`
    /// plus a little (each task kills its server after that); then gives up on the stragglers
    /// (aborting a task drops its process, which kills it). The future borrows nothing.
    pub fn shutdown(&mut self) -> impl Future<Output = ()> + 'static {
        self.sink = None;
        let mut tasks = std::mem::take(&mut self.stopping);
        for s in self.servers.drain(..) {
            s.stop();
            tasks.push(s.task);
        }
        let aborts: Vec<_> = tasks.iter().map(JoinHandle::abort_handle).collect();
        async move {
            if tasks.is_empty() {
                return;
            }
            let all = futures_util::future::join_all(tasks);
            if tokio::time::timeout(SHUTDOWN_TIMEOUT + Duration::from_secs(1), all)
                .await
                .is_err()
            {
                tracing::warn!("language servers didn't stop in time");
                aborts.iter().for_each(|a| a.abort());
            }
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
