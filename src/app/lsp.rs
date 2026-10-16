//! Language servers from the app's side: starting the ones open files need, their events,
//! requests and their answers, and stopping them on quit. The servers themselves live in
//! `crate::lsp`.
//!
//! Requests: `send_request` asks about a position in a buffer shown in a group and keeps a
//! `Pending` in `App::requests` by (server, id). A newer request of the same kind replaces
//! (cancels: `$/cancelRequest`) the older one; after every input event `check_requests` cancels
//! those whose place moved on (an edit, the cursor or the mouse elsewhere); an answer is taken
//! only while its request is still pending and current, and goes to its kind's handler; a server
//! that fails or exits drops its pending requests (it answers none of them).

use lsp_types::ServerCapabilities;
use serde_json::Value;

use super::App;
use crate::buffer::BufferId;
use crate::buffer::pos::{ByteIdx, PosEncoding};
use crate::layout::GroupId;
use crate::lsp::rpc::RpcError;
use crate::lsp::{Event, Sent, ServerId};
use crate::syntax::Language;

/// What a pending request is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Request {
    /// Hover: for the mouse resting on `word` (`Some`: the run under the pointer), else for the
    /// cursor (the command).
    Hover { word: Option<(ByteIdx, ByteIdx)> },
    /// Go to definition of `word` (the name under the cursor, for the "not found" message).
    Definition { word: String },
    /// Completion at the cursor; `app.completion.ask` with this generation holds where it was
    /// asked (current while the cursor stays in that word: typing on doesn't make it stale).
    Completion { generation: u64 },
    /// `completionItem/resolve` for item `index` of the list `generation`: current while that list
    /// is shown; with `accepted`, the item was inserted leaving the buffer at that revision, and
    /// the answer's additional edits go in while it still is.
    Resolve {
        generation: u64,
        index: usize,
        accepted: Option<u64>,
    },
    /// Find references to `word` (the name under the cursor, for the title and messages).
    References { word: String },
    /// `textDocument/prepareRename` at the cursor; `word` is the name there (offered when the
    /// server answers `defaultBehavior`).
    PrepareRename { word: String },
    /// `textDocument/rename` of `old` to `new` (for the messages).
    Rename { old: String, new: String },
    /// `textDocument/formatting` of the whole document: current like hover / definition (VS Code
    /// cancels formatting when the text or the cursor changes).
    Format,
}

impl Request {
    fn same_kind(&self, other: &Request) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// A request waiting for its answer, and where it was asked.
#[derive(Debug, Clone)]
pub(super) struct Pending {
    pub request: Request,
    pub group: GroupId,
    pub buffer: BufferId,
    /// The buffer's revision when sent: an edit since makes the answer stale.
    pub revision: u64,
    pub at: ByteIdx,
    /// The server's position encoding, to read the answer.
    pub encoding: PosEncoding,
}

impl App {
    /// Starts (or stops) servers to match the open files' languages, `.workspace` `[lsp.*]` and
    /// trust, then brings their documents up to date (`didOpen` / `didChange` / `didClose`:
    /// `lsp::documents`). Runs each event-loop turn once `start` has run; the servers only after
    /// `servers_dirty` is set (a buffer added, languages re-detected, `.workspace` reloaded,
    /// trust decided).
    pub(super) fn sync_servers(&mut self) {
        if !self.lsp.started() {
            return;
        }
        if self.servers_dirty {
            self.servers_dirty = false;
            self.start_servers();
        }
        self.lsp.sync_documents(&mut self.buffers);
        self.drop_cleared_diagnostics();
    }

    fn start_servers(&mut self) {
        let mut languages: Vec<Language> = Vec::new();
        for (_, buf) in self.buffers.iter() {
            let l = buf.syntax.language();
            if !languages.contains(&l) {
                languages.push(l);
            }
        }
        self.lsp
            .sync(&self.workspace.root, &languages, &self.workspace.config, self.trust);
    }

    pub(super) fn lsp_event(&mut self, event: Event) {
        match event {
            Event::Diagnostics { server, published } => {
                if self.lsp.publish(server, *published, &mut self.buffers) {
                    self.needs_redraw = true;
                }
            }
            Event::Response { server, id, result } => match self.requests.remove(&(server, id)) {
                Some(p) => self.answer(p, result),
                None => tracing::debug!(id, "answer to a request nobody waits for (cancelled or replaced)"),
            },
            event => {
                if let Event::Failed { server, .. } | Event::Exited { server, .. } = &event {
                    // It answers none of them now.
                    self.requests.retain(|(s, _), _| s != server);
                }
                if let Some(notice) = self.lsp.handle(event) {
                    self.notify(notice.level, notice.text);
                }
            }
        }
        self.drop_cleared_diagnostics();
    }

    /// Drops the marks of servers whose diagnostics the manager dropped (stopped, failed,
    /// crashed) from every buffer.
    fn drop_cleared_diagnostics(&mut self) {
        for server in self.lsp.take_cleared() {
            for (_, buf) in self.buffers.iter_mut() {
                buf.diagnostics.clear(server);
            }
            self.needs_redraw = true;
        }
    }

    /// Sends `method` about `at` in `buffer` (shown in `group`) to the server that has it open,
    /// replacing a pending request of the same kind. False when no running server has the
    /// document or `supported` says it lacks the feature.
    pub(super) fn send_request(
        &mut self,
        group: GroupId,
        buffer: BufferId,
        at: ByteIdx,
        method: &str,
        supported: fn(&ServerCapabilities) -> bool,
        request: Request,
    ) -> bool {
        self.send_request_with(group, buffer, at, method, supported, request, Value::Null)
    }

    /// `send_request` with `extra`'s fields added to the params (`textDocument`, `position`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_request_with(
        &mut self,
        group: GroupId,
        buffer: BufferId,
        at: ByteIdx,
        method: &str,
        supported: fn(&ServerCapabilities) -> bool,
        request: Request,
        extra: Value,
    ) -> bool {
        self.cancel_requests(|p| p.request.same_kind(&request));
        if !self.lsp.started() {
            return false;
        }
        let Some(sent) = self
            .lsp
            .request_at(&mut self.buffers, buffer, at, method, supported, extra)
        else {
            return false;
        };
        self.track_request(sent, request, (group, buffer, at));
        true
    }

    /// `send_request_with` about the whole document (no `position`; `extra`'s fields added);
    /// `at` is the cursor that makes the answer current.
    pub(super) fn send_doc_request(
        &mut self,
        (group, buffer, at): (GroupId, BufferId, ByteIdx),
        method: &str,
        supported: fn(&ServerCapabilities) -> bool,
        request: Request,
        extra: Value,
    ) -> bool {
        self.cancel_requests(|p| p.request.same_kind(&request));
        if !self.lsp.started() {
            return false;
        }
        let Some(sent) = self
            .lsp
            .request_doc(&mut self.buffers, buffer, method, supported, extra)
        else {
            return false;
        };
        self.track_request(sent, request, (group, buffer, at));
        true
    }

    /// Keeps a sent request's `Pending` until its answer.
    fn track_request(&mut self, sent: Sent, request: Request, (group, buffer, at): (GroupId, BufferId, ByteIdx)) {
        let revision = self.buffers.get(buffer).map_or(0, |b| b.revision());
        self.requests.insert(
            (sent.server, sent.id),
            Pending {
                request,
                group,
                buffer,
                revision,
                at,
                encoding: sent.encoding,
            },
        );
    }

    /// Sends `method` with `params` as they are to `server` (a request not about a position:
    /// `completionItem/resolve`), replacing a pending request of the same kind; the `Pending` gets
    /// `group`, `buffer` and `at`. False when the server isn't running.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_server_request(
        &mut self,
        server: ServerId,
        encoding: PosEncoding,
        (group, buffer, at): (GroupId, BufferId, ByteIdx),
        method: &str,
        params: Value,
        request: Request,
    ) -> bool {
        self.cancel_requests(|p| p.request.same_kind(&request));
        let Some(id) = self.lsp.request(server, method, params) else {
            return false;
        };
        let revision = self.buffers.get(buffer).map_or(0, |b| b.revision());
        self.requests.insert(
            (server, id),
            Pending {
                request,
                group,
                buffer,
                revision,
                at,
                encoding,
            },
        );
        true
    }

    /// Cancels (and forgets) the pending requests `which` picks.
    pub(super) fn cancel_requests(&mut self, which: impl Fn(&Pending) -> bool) {
        let gone: Vec<(ServerId, i64)> = self
            .requests
            .iter()
            .filter(|(_, p)| which(p))
            .map(|(k, _)| *k)
            .collect();
        for (server, id) in gone {
            self.requests.remove(&(server, id));
            self.lsp.cancel(server, id);
        }
    }

    /// After an input event: cancels the requests whose place moved on.
    pub(super) fn check_requests(&mut self) {
        let stale: Vec<(ServerId, i64)> = self
            .requests
            .iter()
            .filter(|(_, p)| !self.request_current(p))
            .map(|(k, _)| *k)
            .collect();
        for (server, id) in stale {
            self.requests.remove(&(server, id));
            self.lsp.cancel(server, id);
        }
    }

    /// Whether an answer to `p` still fits: the buffer unedited, and the cursor (the active view
    /// of `p.group`, still the active group) or for a mouse hover the pointer still there.
    fn request_current(&self, p: &Pending) -> bool {
        match &p.request {
            Request::Completion { generation } => return self.completion_ask_current(*generation),
            Request::Resolve {
                generation,
                accepted: None,
                ..
            } => {
                return self
                    .completion
                    .popup
                    .as_ref()
                    .is_some_and(|c| c.generation == *generation);
            }
            Request::Resolve {
                accepted: Some(revision),
                ..
            } => return self.buffers.get(p.buffer).is_some_and(|b| b.revision() == *revision),
            _ => {}
        }
        if self.buffers.get(p.buffer).is_none_or(|b| b.revision() != p.revision) {
            return false;
        }
        match &p.request {
            Request::Hover { word: Some(word) } => self.hover.mouse == Some((p.group, p.buffer, *word)),
            _ => {
                self.groups.active == p.group
                    && self
                        .group()
                        .active_tab()
                        .is_some_and(|t| t.buffer == p.buffer && t.view.selections.primary().head == p.at)
            }
        }
    }

    /// An answer to a pending request: dropped when stale or an error, else handed to its kind.
    fn answer(&mut self, p: Pending, result: Result<Value, RpcError>) {
        if !self.request_current(&p) {
            tracing::debug!(request = ?p.request, "stale answer dropped");
            return;
        }
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                // Cancelled (-32800) or content modified (-32801) are routine.
                tracing::debug!(code = e.code, "request failed: {}", e.message);
                match &p.request {
                    Request::Definition { word } => {
                        let text = super::locations::no_definition(word);
                        self.notify(super::Level::Info, text);
                    }
                    Request::Completion { generation } => self.completion_failed(*generation),
                    Request::References { word } => {
                        let text = super::locations::no_references(word);
                        self.notify(super::Level::Info, text);
                    }
                    // The server's reason (VS Code shows it as is).
                    Request::PrepareRename { .. } => {
                        let text = if e.message.is_empty() {
                            super::rename::CANT_RENAME.to_string()
                        } else {
                            e.message.clone()
                        };
                        self.notify(super::Level::Info, text);
                    }
                    Request::Rename { .. } => {
                        self.notify(super::Level::Error, format!("Rename failed: {}", e.message));
                    }
                    Request::Format => {
                        self.notify(super::Level::Error, format!("Format Document failed: {}", e.message));
                    }
                    _ => {}
                }
                return;
            }
        };
        self.needs_redraw = true;
        match p.request.clone() {
            Request::Hover { word } => self.hover_answer(&p, word, &value),
            Request::Definition { word } => self.definition_answer(&p, &word, &value),
            Request::Completion { generation } => self.completion_answer(generation, &value),
            Request::Resolve {
                generation,
                index,
                accepted,
            } => self.resolve_answer(&p, generation, index, accepted, &value),
            Request::References { word } => self.references_answer(&p, &word, &value),
            Request::PrepareRename { word } => self.prepare_rename_answer(&p, &word, &value),
            Request::Rename { old, new } => self.rename_answer(&p, &old, &new, &value),
            Request::Format => self.format_answer(&p, &value),
        }
    }

    /// Asks every server to exit and waits for them (bounded: see `lsp::Manager::shutdown`).
    /// `run` (via `shutdown`) on quit; tests call it after `Quit`.
    #[cfg_attr(not(test), allow(dead_code))] // tests
    pub async fn shutdown_servers(&mut self) {
        self.lsp.shutdown().await;
    }
}
