//! The socket tasks reach this jed by (`src/ipc.rs`; PLAN.md, Git editor, "From a task
//! terminal"): started from `start`, requests arrive as `AppEvent::Ipc`, and a `wait` request is
//! answered when its file's last tab closes (`ipc_sweep`, called wherever a tab or a load ends).

use std::path::{Path, PathBuf};

use tokio::sync::mpsc::UnboundedSender;

use super::files::Goto;
use super::{App, AppEvent, Focus, Level};
use crate::buffer::BufferId;
use crate::ipc::{self, Reply, Request, Responder, Server};

/// A client waiting for `path` to be closed.
struct Waiter {
    path: PathBuf,
    responder: Responder,
    /// The file's buffer once seen open: the wait ends when it's gone (so a rename while
    /// waiting doesn't end it). Before that, no buffer and no load running means it failed.
    buffer: Option<BufferId>,
}

/// The listening socket and the clients waiting on files.
#[derive(Default)]
pub struct Ipc {
    server: Option<Server>,
    waiters: Vec<Waiter>,
    /// `start_ipc` has spawned the socket and `ipc_started` hasn't answered yet.
    starting: bool,
    /// Shells asked for meanwhile: `(in_terminal, keep_focus)`, opened by `ipc_started`.
    queued_shells: Vec<(bool, bool)>,
}

impl App {
    /// Prepares the socket directory off-thread, then listens (`AppEvent::IpcStarted`). Only
    /// with an `ipc_dir` (not in tests unless set).
    pub(super) fn start_ipc(&mut self) {
        let Some(dir) = self.ipc_dir.clone() else { return };
        let tx = self.tx.clone();
        self.ipc.starting = true;
        tokio::spawn(async move {
            let path = ipc::socket_path(&dir, std::process::id());
            let prepared = tokio::task::spawn_blocking(move || ipc::prepare_dir(&dir).map(|()| dir)).await;
            let result = match prepared {
                Ok(Ok(_)) => start_server(&path, tx.clone()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppEvent::IpcStarted(result));
        });
    }

    /// Whether the socket is still coming up, so a new shell should wait for `JED_IPC`.
    pub(super) fn ipc_starting(&self) -> bool {
        self.ipc.starting
    }

    pub(super) fn ipc_queue_shell(&mut self, in_terminal: bool, keep_focus: bool) {
        self.ipc.queued_shells.push((in_terminal, keep_focus));
    }

    pub(super) fn ipc_started(&mut self, result: Result<Server, String>) {
        self.ipc.starting = false;
        match result {
            Ok(server) => {
                tracing::info!(path = %server.path().display(), "listening for task clients");
                crate::panic::set_ipc_socket(Some(server.path().to_path_buf()));
                self.tasks.ipc = Some(server.path().to_path_buf());
                self.ipc.server = Some(server);
            }
            // Tasks just won't get `JED_IPC`.
            Err(e) => tracing::warn!(error = %e, "not listening for task clients"),
        }
        // Either way the waiting shells start now (without `JED_IPC` after a failure).
        for (in_terminal, keep_focus) in std::mem::take(&mut self.ipc.queued_shells) {
            self.open_queued_shell(in_terminal, keep_focus);
        }
    }

    /// A client asks to open a file: as quick open does, with the cursor at `line:col` (1:1).
    pub(super) fn ipc_request(&mut self, request: Request, responder: Responder) {
        let goto = Goto::Cursor(request.line.unwrap_or(1), request.col.unwrap_or(1));
        self.open_file(request.open.clone(), true, Some(goto));
        self.ui.focus = Focus::Editor;
        if !request.wait {
            responder.answer(Reply::opened());
            return;
        }
        self.ipc.waiters.push(Waiter {
            path: request.open,
            responder,
            buffer: None,
        });
        self.ipc_sweep();
    }

    /// Answers the clients whose file has no tab left, and drops those whose load failed.
    pub(super) fn ipc_sweep(&mut self) {
        if self.ipc.waiters.is_empty() {
            return;
        }
        let waiters = std::mem::take(&mut self.ipc.waiters);
        for mut w in waiters {
            if let Some(id) = w.buffer {
                if self.buffers.get(id).is_some() {
                    self.ipc.waiters.push(w);
                } else {
                    w.responder.answer(Reply::closed());
                }
            } else if let Some(id) = self.buffers.find_path(&w.path) {
                w.buffer = Some(id);
                self.ipc.waiters.push(w);
            } else if self.opening.contains_key(&w.path) {
                self.ipc.waiters.push(w);
            } else {
                self.notify(
                    Level::Error,
                    format!("{}: could not open for the editor client", w.path.display()),
                );
                // Dropped: the client exits 1.
            }
        }
    }

    /// How many clients wait on a file.
    #[cfg(test)]
    pub fn test_ipc_waiters(&self) -> usize {
        self.ipc.waiters.len()
    }

    /// On quit: the waiting clients' connections close (they exit 1) and the socket goes.
    pub(super) fn ipc_shutdown(&mut self) {
        self.ipc.waiters.clear();
        self.ipc.queued_shells.clear();
        self.ipc.server = None;
        self.tasks.ipc = None;
        crate::panic::set_ipc_socket(None);
    }
}

fn start_server(path: &Path, tx: UnboundedSender<AppEvent>) -> Result<Server, String> {
    Server::start(path, move |request, responder| {
        let _ = tx.send(AppEvent::Ipc(request, responder));
    })
    .map_err(|e| e.to_string())
}
