//! LSP semantic tokens (`textDocument/semanticTokens/full` and `full/delta`) into `Buffer::semantic`
//! (model: `semantic`). Drawing them is not here.
//!
//! - `check_semantic` runs after every event: a buffer whose document is open on a running server
//!   with a `semanticTokensProvider` gets a `Doc` (the last `resultId` and the server's raw array,
//!   what the next delta applies to); a new one is due at once, an edit makes it due `DELAY` after
//!   the last one (`Timer::Semantic` -> `semantic_tick`). A buffer that lost its server (stopped,
//!   crashed, another one for its language) is cleared; one whose language or path changed (its
//!   document reopened, maybe on the same server) is cleared and asked again at once.
//! - One request per buffer at a time (decoding included); edits meanwhile ask again after it.
//!   The request is `full/delta` while there is a `resultId`, else `full`.
//! - An answer is never dropped for a newer revision: the tokens are read in the text of the
//!   version it was asked for (`Manager::text_at`) on a worker (`AppEvent::SemanticDecoded`), then
//!   mapped through the change sets since (`Manager::since`) and replace the buffer's.
//! - `semantic_highlighting = off` treats every server as having none: nothing is asked, tokens and
//!   inactive regions are dropped (a clangd reparse sends the regions again once it is on).
//! - `textDocument/inactiveRegions` (`semantic_inactive`) replaces the buffer's inactive regions.
//! - `workspace/semanticTokens/refresh` (an `Event::Notification` from the server task, already
//!   answered) makes every buffer of that server due.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lsp_types::{
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities,
};
use serde_json::{Value, json};

use super::lsp::{Pending, Request};
use super::{App, AppEvent};
use crate::buffer::BufferId;
use crate::buffer::pos::{ByteIdx, LspPos, LspReader};
use crate::edit::SemanticHighlighting;
use crate::lsp::ServerId;
use crate::semantic::{self, DeltaEdit, Legend, Region, Token};
use crate::syntax::Language;

/// How long after the last edit the tokens are asked for again.
pub const DELAY: Duration = Duration::from_millis(150);

pub const FULL: &str = "textDocument/semanticTokens/full";
pub const DELTA: &str = "textDocument/semanticTokens/full/delta";
pub const REFRESH: &str = "workspace/semanticTokens/refresh";
/// clangd's notification (opt-in at initialize) for code its preprocessor left out.
pub const INACTIVE: &str = "textDocument/inactiveRegions";

#[derive(Default)]
pub struct State {
    docs: HashMap<BufferId, Doc>,
}

struct Doc {
    server: ServerId,
    /// The language and path the document was opened with: a change (C++ to C on the same
    /// clangd, a rename) reopens it, so its tokens and `resultId` start again.
    language: Language,
    path: PathBuf,
    /// The buffer revision last seen (an edit sets `deadline`).
    seen: u64,
    deadline: Option<Instant>,
    /// The server's `resultId` for `data`, for the next delta.
    result_id: Option<String>,
    /// The server's array as of `result_id`.
    data: Vec<u32>,
    /// An answer is being decoded.
    decoding: bool,
    /// Something changed while a request or decode was running.
    again: bool,
}

impl State {
    pub fn deadline(&self) -> Option<Instant> {
        self.docs.values().filter_map(|d| d.deadline).min()
    }
}

fn options(caps: &ServerCapabilities) -> Option<&SemanticTokensOptions> {
    match caps.semantic_tokens_provider.as_ref()? {
        SemanticTokensServerCapabilities::SemanticTokensOptions(o) => Some(o),
        SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(o) => Some(&o.semantic_tokens_options),
    }
}

/// Whether the server answers `textDocument/semanticTokens/full`.
pub fn full_supported(caps: &ServerCapabilities) -> bool {
    options(caps).is_some_and(|o| match &o.full {
        Some(SemanticTokensFullOptions::Bool(b)) => *b,
        Some(SemanticTokensFullOptions::Delta { .. }) => true,
        None => false,
    })
}

fn delta_supported(caps: &ServerCapabilities) -> bool {
    options(caps).is_some_and(|o| matches!(o.full, Some(SemanticTokensFullOptions::Delta { delta: Some(true) })))
}

impl App {
    /// After every event: the buffers' `Doc`s follow their servers, and edits arm the debounce.
    pub(super) fn check_semantic(&mut self) {
        if !self.lsp.started() {
            return;
        }
        let now = Instant::now();
        let on = self.settings.semantic_highlighting == SemanticHighlighting::On;
        let ids: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.has_file())
            .map(|(id, _)| id)
            .collect();
        self.semantic.docs.retain(|id, _| ids.contains(id));
        for id in ids {
            let server = self
                .lsp
                .server_of(id)
                .filter(|(_, _, caps)| on && full_supported(caps))
                .map(|(s, ..)| s);
            let Some(buf) = self.buffers.get_mut(id) else {
                continue;
            };
            let Some(server) = server else {
                self.semantic.docs.remove(&id);
                if !buf.semantic.is_empty() {
                    buf.semantic.clear();
                    self.needs_redraw = true;
                }
                continue;
            };
            let revision = buf.revision();
            let language = buf.syntax.language();
            match self.semantic.docs.get_mut(&id) {
                Some(d) if d.server == server && d.language == language && d.path == buf.path => {
                    if d.seen != revision {
                        d.seen = revision;
                        d.deadline = Some(now + DELAY);
                    }
                }
                other => {
                    if other.is_some() && !buf.semantic.is_empty() {
                        buf.semantic.clear();
                        self.needs_redraw = true;
                    }
                    self.semantic.docs.insert(
                        id,
                        Doc {
                            server,
                            language,
                            path: buf.path.clone(),
                            seen: revision,
                            deadline: Some(now),
                            result_id: None,
                            data: Vec::new(),
                            decoding: false,
                            again: false,
                        },
                    );
                }
            }
        }
    }

    /// The debounce passed: the due buffers ask (or wait for the running request to end).
    pub(super) fn semantic_tick(&mut self, now: Instant) {
        let due: Vec<BufferId> = self
            .semantic
            .docs
            .iter()
            .filter(|(_, d)| d.deadline.is_some_and(|t| t <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let busy = self.semantic_busy(id);
            let Some(doc) = self.semantic.docs.get_mut(&id) else {
                continue;
            };
            doc.deadline = None;
            if busy {
                doc.again = true;
            } else {
                self.request_semantic(id);
            }
        }
    }

    fn semantic_busy(&self, buffer: BufferId) -> bool {
        self.semantic.docs.get(&buffer).is_some_and(|d| d.decoding)
            || self
                .requests
                .values()
                .any(|p| matches!(p.request, Request::SemanticTokens { buffer: b, .. } if b == buffer))
    }

    fn request_semantic(&mut self, buffer: BufferId) {
        self.lsp.sync_documents(&mut self.buffers);
        let Some(version) = self.lsp.version(buffer) else {
            return;
        };
        let Some(doc) = self.semantic.docs.get(&buffer) else {
            return;
        };
        let delta = doc.result_id.is_some() && self.lsp.server_of(buffer).is_some_and(|(_, _, c)| delta_supported(c));
        let (method, extra) = match (&doc.result_id, delta) {
            (Some(id), true) => (DELTA, json!({ "previousResultId": id })),
            _ => (FULL, Value::Null),
        };
        let place = (self.groups.active, buffer, ByteIdx(0));
        self.send_doc_request(
            place,
            method,
            full_supported,
            Request::SemanticTokens { buffer, version, delta },
            extra,
        );
    }

    /// `workspace/semanticTokens/refresh` from `server`: everything of it is asked again.
    pub(super) fn semantic_refresh(&mut self, server: ServerId) {
        let now = Instant::now();
        for d in self.semantic.docs.values_mut().filter(|d| d.server == server) {
            d.deadline = Some(now);
        }
    }

    /// clangd's `textDocument/inactiveRegions` (`{ textDocument: { uri }, regions: Range[] }`): the
    /// regions replace the buffer's, read in its current text (the notification names no version;
    /// an edit since is mapped on the next one clangd sends after reparsing).
    pub(super) fn semantic_inactive(&mut self, server: ServerId, params: &Value) {
        if self.settings.semantic_highlighting == SemanticHighlighting::Off {
            return;
        }
        let Some(encoding) = self.lsp.encoding_of(server) else {
            return;
        };
        let path = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .and_then(|u| u.parse::<lsp_types::Uri>().ok())
            .and_then(|u| crate::lsp::uri::to_path(&u));
        let Some(id) = path.and_then(|p| self.buffers.find_path(&p)) else {
            return;
        };
        if self.lsp.server_of(id).is_none_or(|(s, ..)| s != server) {
            return;
        }
        let Some(buf) = self.buffers.get_mut(id) else {
            return;
        };
        let pos = |v: &Value| -> Option<LspPos> {
            Some(LspPos {
                line: v.get("line")?.as_u64()? as u32,
                character: v.get("character")?.as_u64()? as u32,
            })
        };
        let mut reader = LspReader::new(&buf.rope, encoding);
        let regions: Vec<Region> = params
            .get("regions")
            .and_then(Value::as_array)
            .map(|rs| {
                rs.iter()
                    .filter_map(|r| {
                        let (s, e) = (pos(r.get("start")?)?, pos(r.get("end")?)?);
                        Some(Region {
                            start: reader.byte(s),
                            end: reader.byte(e),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        buf.semantic.replace_inactive(regions);
        self.needs_redraw = true;
    }

    /// The request ended without tokens to use: the next one is a full request.
    pub(super) fn semantic_failed(&mut self, buffer: BufferId) {
        if let Some(d) = self.semantic.docs.get_mut(&buffer) {
            d.result_id = None;
            d.data.clear();
            d.decoding = false;
            if std::mem::take(&mut d.again) {
                d.deadline = Some(Instant::now());
            }
        }
    }

    /// The answer to a `Request::SemanticTokens` asked for `version`.
    pub(super) fn semantic_answer(&mut self, p: &Pending, version: i32, value: &Value) {
        let buffer = p.buffer;
        let Some(doc) = self.semantic.docs.get_mut(&buffer).filter(|d| d.server == p.server) else {
            return;
        };
        let Some(obj) = value.as_object() else {
            return self.semantic_failed(buffer);
        };
        let result_id = obj.get("resultId").and_then(Value::as_str).map(String::from);
        let ok = if let Some(edits) = obj.get("edits").and_then(Value::as_array) {
            let parsed: Vec<DeltaEdit> = edits.iter().filter_map(delta_edit).collect();
            parsed.len() == edits.len() && semantic::apply_delta(&mut doc.data, &parsed)
        } else if let Some(data) = obj.get("data").and_then(Value::as_array) {
            doc.data = data.iter().map(|n| n.as_u64().unwrap_or(0) as u32).collect();
            true
        } else {
            false
        };
        if !ok {
            tracing::debug!("unusable semantic tokens result");
            return self.semantic_failed(buffer);
        }
        doc.result_id = result_id;
        let data = doc.data.clone();
        let legend = self
            .lsp
            .server_of(buffer)
            .and_then(|(_, _, c)| options(c))
            .map(|o| Legend::from_lsp(&o.legend))
            .unwrap_or_default();
        let text = self.lsp.text_at(buffer, version).cloned();
        let rt = tokio::runtime::Handle::try_current().ok();
        let (Some(text), Some(rt)) = (text, rt) else {
            // The version isn't kept (or no runtime: unit tests): ask for a whole new set.
            return self.semantic_failed(buffer);
        };
        self.semantic.docs.get_mut(&buffer).expect("checked").decoding = true;
        let (tx, server, encoding) = (self.tx.clone(), p.server, p.encoding);
        rt.spawn_blocking(move || {
            let t = Instant::now();
            let tokens = semantic::decode(&data, &text, &legend, encoding);
            tracing::debug!(
                tokens = tokens.len(),
                elapsed_us = t.elapsed().as_micros() as u64,
                "semantic tokens decoded"
            );
            let _ = tx.send(AppEvent::SemanticDecoded {
                buffer,
                server,
                version,
                tokens,
            });
        });
    }

    /// Decoded tokens (in `version`'s text) arrived: mapped through the edits since, they replace
    /// the buffer's.
    pub(super) fn semantic_decoded(
        &mut self,
        buffer: BufferId,
        server: ServerId,
        version: i32,
        mut tokens: Vec<Token>,
    ) {
        let Some(doc) = self.semantic.docs.get_mut(&buffer).filter(|d| d.server == server) else {
            return;
        };
        doc.decoding = false;
        self.lsp.sync_documents(&mut self.buffers);
        match self.lsp.since(buffer, server, version) {
            Some((_, sets)) => {
                for set in &sets {
                    semantic::map_tokens(&mut tokens, set);
                }
                if let Some(buf) = self.buffers.get_mut(buffer) {
                    buf.semantic.replace(tokens);
                    self.needs_redraw = true;
                }
                let doc = self.semantic.docs.get_mut(&buffer).expect("checked");
                if std::mem::take(&mut doc.again) {
                    doc.deadline = Some(Instant::now());
                }
            }
            None => {
                tracing::debug!(version, "semantic tokens for a version no longer kept");
                self.semantic_failed(buffer);
            }
        }
    }
}

fn delta_edit(v: &Value) -> Option<DeltaEdit> {
    Some(DeltaEdit {
        start: v.get("start")?.as_u64()? as usize,
        delete: v.get("deleteCount")?.as_u64()? as usize,
        data: v
            .get("data")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|n| n.as_u64().unwrap_or(0) as u32).collect())
            .unwrap_or_default(),
    })
}
