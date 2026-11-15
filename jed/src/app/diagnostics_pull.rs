//! Pull diagnostics (`textDocument/diagnostic`) into the same store and marks as published ones
//! (`lsp::Manager::publish`). Model: `semantic` (the same debounce / version / refresh pattern).
//!
//! - `check_pull` runs after every event: a buffer whose document is open on a running server with
//!   a `diagnosticProvider` gets a `Doc` (the last `resultId`); a new one is due at once (open), an
//!   edit makes it due `DELAY` after the last one (`Timer::PullDiagnostics` -> `pull_tick`). An edit
//!   only notes the revision here: the timer sends, so a burst (61k selections included) is one
//!   request after the rest.
//! - Pulled marks are a set of their own (`ServerId::source(Source::Pull)`), beside the server's published
//!   one: neither replaces the other (rust-analyzer pushes `cargo check`'s and answers pulls with its native
//!   ones), as in VS Code.
//! - One request per buffer: a newer one cancels the older (`App::send_request`). It carries
//!   `previousResultId` (and the provider's `identifier`) when there is one.
//! - An answer is never dropped for a newer revision: a `full` report is read in the text of the
//!   version asked for and mapped forward (`Manager::publish`), unless the server's marks for a
//!   newer version are shown already (published or pulled: the newer one wins). An `unchanged`
//!   report keeps the marks. `relatedDocuments` are applied to their files; with
//!   `interFileDependencies` a `full` answer to an edit makes the server's other documents due.
//! - `workspace/diagnostic/refresh` (already answered by the server task) makes every document of
//!   that server due. A routine failure (cancelled, content modified) asks again after `DELAY`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lsp_types::{DiagnosticServerCapabilities, ServerCapabilities};
use serde_json::{Value, json};

use super::App;
use super::lsp::{Pending, Request};
use crate::buffer::BufferId;
use crate::buffer::pos::ByteIdx;
use crate::lsp::diagnostics::{self, Published, Report};
use crate::lsp::{ServerId, Source};
use crate::syntax::Language;

/// Why a document is asked for: an edit or opening may make the server's other documents due
/// again (`interFileDependencies`); a refresh or such a dependency doesn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Edit,
    Refresh,
}

/// How long after the last edit the diagnostics are asked for again.
pub const DELAY: Duration = Duration::from_millis(300);

pub const METHOD: &str = "textDocument/diagnostic";
pub const REFRESH: &str = "workspace/diagnostic/refresh";

#[derive(Default)]
pub struct State {
    docs: HashMap<BufferId, Doc>,
}

struct Doc {
    server: ServerId,
    /// The language and path the document was opened with: a change reopens it, so its
    /// `resultId` starts again.
    language: Language,
    path: PathBuf,
    /// The buffer revision last seen (an edit sets `deadline`).
    seen: u64,
    deadline: Option<Instant>,
    /// The server's `resultId` for the last report.
    result_id: Option<String>,
    /// What the next request follows.
    trigger: Trigger,
}

impl State {
    pub fn deadline(&self) -> Option<Instant> {
        self.docs.values().filter_map(|d| d.deadline).min()
    }

    /// Tests: a document of buffer `id` whose request is due at `at`.
    #[cfg(test)]
    pub fn test_arm(&mut self, id: BufferId, at: Instant) {
        self.docs.insert(
            id,
            Doc {
                server: ServerId::none(),
                language: Language::default(),
                path: PathBuf::new(),
                seen: 0,
                deadline: Some(at),
                result_id: None,
                trigger: Trigger::Refresh,
            },
        );
    }

    /// Tests: the `resultId` kept for `buffer`.
    #[cfg(test)]
    pub fn result_id(&self, buffer: BufferId) -> Option<&str> {
        self.docs.get(&buffer)?.result_id.as_deref()
    }
}

fn supported(caps: &ServerCapabilities) -> bool {
    caps.diagnostic_provider.is_some()
}

fn options(caps: &ServerCapabilities) -> Option<&lsp_types::DiagnosticOptions> {
    match caps.diagnostic_provider.as_ref()? {
        DiagnosticServerCapabilities::Options(o) => Some(o),
        DiagnosticServerCapabilities::RegistrationOptions(o) => Some(&o.diagnostic_options),
    }
}

impl App {
    /// After every event: the buffers' `Doc`s follow their servers, and edits arm the debounce.
    pub(super) fn check_pull(&mut self) {
        if !self.lsp.started() {
            return;
        }
        let now = Instant::now();
        let buffers = &self.buffers;
        self.pull
            .docs
            .retain(|id, _| buffers.get(*id).is_some_and(|b| b.has_file()));
        for (id, buf) in self.buffers.iter().filter(|(_, b)| b.has_file()) {
            let server = self
                .lsp
                .server_of(id)
                .filter(|(_, _, caps)| supported(caps))
                .map(|(s, ..)| s);
            let Some(server) = server else {
                self.pull.docs.remove(&id);
                continue;
            };
            let revision = buf.revision();
            let language = buf.syntax.language();
            match self.pull.docs.get_mut(&id) {
                Some(d) if d.server == server && d.language == language && d.path == buf.path => {
                    if d.seen != revision {
                        d.seen = revision;
                        d.deadline = Some(now + DELAY);
                        d.trigger = Trigger::Edit;
                    }
                }
                _ => {
                    self.pull.docs.insert(
                        id,
                        Doc {
                            server,
                            language,
                            path: buf.path.clone(),
                            seen: revision,
                            deadline: Some(now),
                            result_id: None,
                            trigger: Trigger::Edit,
                        },
                    );
                }
            }
        }
    }

    /// The debounce passed: the due buffers ask.
    pub(super) fn pull_tick(&mut self, now: Instant) {
        let due: Vec<BufferId> = self
            .pull
            .docs
            .iter()
            .filter(|(_, d)| d.deadline.is_some_and(|t| t <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(doc) = self.pull.docs.get_mut(&id) {
                doc.deadline = None;
                self.request_pull(id);
            }
        }
    }

    fn request_pull(&mut self, buffer: BufferId) {
        self.lsp.sync_documents(&mut self.buffers);
        let Some(version) = self.lsp.version(buffer) else {
            return;
        };
        let identifier = self
            .lsp
            .server_of(buffer)
            .and_then(|(_, _, c)| options(c))
            .and_then(|o| o.identifier.clone());
        let Some(doc) = self.pull.docs.get_mut(&buffer) else {
            return;
        };
        let mut extra = json!({});
        if let Some(id) = &doc.result_id {
            extra["previousResultId"] = json!(id);
        }
        if let Some(i) = identifier {
            extra["identifier"] = json!(i);
        }
        let trigger = std::mem::replace(&mut doc.trigger, Trigger::Refresh);
        let place = (self.groups.active, buffer, ByteIdx(0));
        self.send_doc_request(
            place,
            METHOD,
            supported,
            Request::Diagnostics {
                buffer,
                version,
                trigger,
            },
            extra,
        );
    }

    /// `workspace/diagnostic/refresh` from `server`: everything of it is asked again.
    pub(super) fn pull_refresh(&mut self, server: ServerId) {
        let now = Instant::now();
        for d in self.pull.docs.values_mut().filter(|d| d.server == server) {
            d.deadline = Some(now);
            d.trigger = Trigger::Refresh;
        }
    }

    /// A request ended quietly (cancelled by the server, content modified): ask again soon.
    pub(super) fn pull_retry(&mut self, buffer: BufferId) {
        if let Some(d) = self.pull.docs.get_mut(&buffer) {
            d.deadline.get_or_insert(Instant::now() + DELAY);
        }
    }

    /// The answer to a `Request::Diagnostics` asked for `version`.
    pub(super) fn pull_answer(&mut self, p: &Pending, version: i32, trigger: Trigger, value: Value) {
        let buffer = p.buffer;
        if self.pull.docs.get(&buffer).is_none_or(|d| d.server != p.server) {
            return;
        }
        let pulled = match diagnostics::parse_pulled(value) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("unreadable pull diagnostics: {e}");
                return;
            }
        };
        let Some(path) = self.buffers.get(buffer).map(|b| b.path.clone()) else {
            return;
        };
        let mut inter_file = false;
        let result_id = match pulled.report {
            Report::Full { result_id, items } => {
                inter_file = true;
                let taken = self.apply_pulled(
                    p.server,
                    Published {
                        path: path.clone(),
                        version: Some(version),
                        items,
                    },
                );
                taken.then_some(result_id).flatten()
            }
            Report::Unchanged { result_id } => {
                let fresh = self.lsp.applied_unchanged(p.server, &path, version);
                let old = self.pull.docs.get(&buffer).and_then(|d| d.result_id.clone());
                if fresh { result_id.or(old) } else { None }
            }
        };
        for (path, report) in pulled.related {
            if let Report::Full { items, .. } = report {
                self.apply_pulled(
                    p.server,
                    Published {
                        path,
                        version: None,
                        items,
                    },
                );
            }
        }
        if let Some(d) = self.pull.docs.get_mut(&buffer) {
            d.result_id = result_id;
        }
        let dependent = self
            .lsp
            .server_of(buffer)
            .and_then(|(_, _, c)| options(c))
            .is_some_and(|o| o.inter_file_dependencies);
        if trigger == Trigger::Edit && inter_file && dependent {
            let now = Instant::now();
            for (_, d) in self
                .pull
                .docs
                .iter_mut()
                .filter(|(id, d)| **id != buffer && d.server == p.server)
            {
                d.deadline.get_or_insert(now);
            }
        }
    }

    /// Puts a pulled report where a published one goes; false when it was dropped.
    fn apply_pulled(&mut self, server: ServerId, published: Published) -> bool {
        let buffer = self.buffers.find_path(&published.path);
        let taken = self.lsp.publish(server, Source::Pull, published, &mut self.buffers);
        if taken {
            self.needs_redraw = true;
            if let Some(id) = buffer {
                self.diagnostics_changed(id);
            }
        }
        taken
    }
}
