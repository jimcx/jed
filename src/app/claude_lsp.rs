//! Language server queries for Claude (`find_definition`, `find_references`, `get_hover`): the
//! MCP tools' `Query::Lsp`, asked of the servers jed already runs without touching anything the
//! user sees (no selection, scroll, popup, Locations list, back / forward entry or `notify`).
//!
//! `claude_lsp_ask` resolves the place (`line` + `symbol` / `col`, in the buffer's text) and
//! sends a `Request::Claude { ask }`; the `Responder` waits in `State::waiting` under `ask`
//! until the answer (`claude_lsp_response`), a failure, the deadline (`State::timeout`, 10 s:
//! `$/cancelRequest`) or the client hanging up. Unlike the user's requests, a cursor move, a tab
//! switch or another request never cancels one; an edit to the buffer answers "ask again"
//! (`claude_lsp_check`, every input event); a routine error (ContentModified while a server
//! loads) is asked again after `State::retry` until the deadline. Waiting is by one timer
//! (`State::deadline`, `Timer::ClaudeLsp`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lsp_types::{DeclarationCapability, HoverProviderCapability, ImplementationProviderCapability, OneOf};
use lsp_types::{ServerCapabilities, TypeDefinitionProviderCapability};
use ropey::Rope;
use serde_json::{Value, json};

use super::claude::clip_head;
use super::files::{Loaded, OpenRequest};
use super::ipc::IpcEvent;
use super::lsp::{Pending, Request};
use super::{App, AppEvent};
use crate::buffer::Buffer;
use crate::buffer::file::LoadError;
use crate::buffer::pos::{self, ByteIdx, DisplayCol, LspPos};
use crate::ipc::{LspAsk, Query, Reply, Responder};
use crate::locations::{self, Location};

/// How long a query waits for its server.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Before a routine error's request is asked again.
const RETRY: Duration = Duration::from_millis(500);
/// Locations in an answer.
const MAX_LOCATIONS: usize = 200;
/// A location's line text, and the hover's text, in bytes.
const MAX_TEXT: usize = 200;
const MAX_HOVER: usize = 4096;

/// What `App::claude_lsp` keeps.
pub(super) struct State {
    next: u64,
    waiting: HashMap<u64, Waiting>,
    /// Queries about a file that isn't open: its background tab is loading.
    loading: HashMap<u64, Ask>,
    /// Why a file Claude asked about could not be loaded (`claude_lsp_load_result`).
    failed: HashMap<PathBuf, String>,
    /// Answers waiting for the lines of files that aren't open (read off-thread).
    reading: HashMap<u64, Reading>,
    /// How long a query waits (tests shorten it).
    pub timeout: Duration,
    /// Before an asked-again request is sent (tests shorten it).
    pub retry: Duration,
}

impl Default for State {
    fn default() -> Self {
        State {
            next: 0,
            waiting: HashMap::new(),
            loading: HashMap::new(),
            failed: HashMap::new(),
            reading: HashMap::new(),
            timeout: TIMEOUT,
            retry: RETRY,
        }
    }
}

impl State {
    /// A waiting query whose deadline is `at` (nothing is asked), for the timer test.
    #[cfg(test)]
    pub(super) fn test_arm(&mut self, buffer: crate::buffer::BufferId, at: Instant) {
        let (responder, _) = Responder::pair();
        self.waiting.insert(
            0,
            Waiting {
                responder,
                ask: LspAsk::Hover,
                buffer,
                revision: 0,
                at: ByteIdx(0),
                include_declaration: true,
                dirty: false,
                place: Value::Null,
                deadline: at,
                retry_at: None,
                parked: None,
                opened: false,
            },
        );
    }

    /// The earliest deadline or retry among the waiting queries.
    pub(super) fn deadline(&self) -> Option<Instant> {
        let waiting = self
            .waiting
            .values()
            .map(|w| w.retry_at.map_or(w.deadline, |r| r.min(w.deadline)));
        waiting.chain(self.loading.values().map(|a| a.deadline)).min()
    }
}

/// One query waiting for its server.
struct Waiting {
    responder: Responder,
    ask: LspAsk,
    buffer: crate::buffer::BufferId,
    /// The buffer's revision it was asked at: an edit since answers "ask again".
    revision: u64,
    at: ByteIdx,
    include_declaration: bool,
    /// Reported in the answer: the buffer had unsaved changes (its lines, not the file's).
    dirty: bool,
    /// The place asked about (`{line, col}`, 1-based).
    place: Value,
    deadline: Instant,
    /// `Some`: no request is in flight, it is sent again then.
    retry_at: Option<Instant>,
    /// `Some(why)`: no request is in flight because the server isn't ready (starting, or it
    /// hasn't the document yet); sent at the next check. The timeout names it.
    parked: Option<String>,
    /// The file was loaded for this query: the answer says `opened: true`.
    opened: bool,
}

/// A query as it came, before its place is resolved.
struct Ask {
    responder: Responder,
    ask: LspAsk,
    path: PathBuf,
    line: usize,
    symbol: Option<String>,
    col: Option<usize>,
    include_declaration: bool,
    deadline: Instant,
}

/// An answer waiting for the text of files that aren't open.
struct Reading {
    responder: Responder,
    data: Value,
    /// `data["locations"][index]` is `location`, its range and `text` still to fill.
    unread: Vec<(usize, Location)>,
    encoding: pos::PosEncoding,
}

fn supports_definition(caps: &ServerCapabilities) -> bool {
    caps.definition_provider
        .as_ref()
        .is_some_and(|d| !matches!(d, OneOf::Left(false)))
}

fn supports_type(caps: &ServerCapabilities) -> bool {
    caps.type_definition_provider
        .as_ref()
        .is_some_and(|d| !matches!(d, TypeDefinitionProviderCapability::Simple(false)))
}

fn supports_implementation(caps: &ServerCapabilities) -> bool {
    caps.implementation_provider
        .as_ref()
        .is_some_and(|d| !matches!(d, ImplementationProviderCapability::Simple(false)))
}

fn supports_declaration(caps: &ServerCapabilities) -> bool {
    caps.declaration_provider
        .as_ref()
        .is_some_and(|d| !matches!(d, DeclarationCapability::Simple(false)))
}

fn supports_references(caps: &ServerCapabilities) -> bool {
    caps.references_provider
        .as_ref()
        .is_some_and(|d| !matches!(d, OneOf::Left(false)))
}

fn supports_hover(caps: &ServerCapabilities) -> bool {
    caps.hover_provider
        .as_ref()
        .is_some_and(|h| !matches!(h, HoverProviderCapability::Simple(false)))
}

fn method(ask: LspAsk) -> (&'static str, fn(&ServerCapabilities) -> bool) {
    match ask {
        LspAsk::Definition => ("textDocument/definition", supports_definition),
        LspAsk::TypeDefinition => ("textDocument/typeDefinition", supports_type),
        LspAsk::Implementation => ("textDocument/implementation", supports_implementation),
        LspAsk::Declaration => ("textDocument/declaration", supports_declaration),
        LspAsk::References => ("textDocument/references", supports_references),
        LspAsk::Hover => ("textDocument/hover", supports_hover),
    }
}

/// `n` as "10 s" or "50 ms".
fn duration_text(d: Duration) -> String {
    if d.as_secs() >= 1 {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

/// Where in `buf`'s `line` (0-based) the query is about: the first whole-word `symbol` at or
/// after `col` (a 1-based display column), else the first one; no symbol: `col`, else the first
/// non-blank. The error quotes the line as jed has it.
fn resolve(
    buf: &Buffer,
    tab_width: usize,
    line: usize,
    symbol: Option<&str>,
    col: Option<usize>,
) -> Result<ByteIdx, String> {
    let lines = pos::line_count(&buf.rope);
    if line >= lines {
        return Err(format!("line {} is past the end of the file ({lines} lines)", line + 1));
    }
    let text = pos::line_text(&buf.rope, line);
    let start = pos::line_start(&buf.rope, line);
    let from = col.map(|c| pos::byte_at_col_in(&text, DisplayCol(c.saturating_sub(1)), tab_width));
    let Some(symbol) = symbol else {
        return Ok(match from {
            Some(off) => ByteIdx(start.0 + off),
            None => pos::first_non_blank(&buf.rope, line),
        });
    };
    let word = |c: char| pos::is_word_char(c);
    let whole = |at: usize| {
        let before = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !word(c) || !symbol.starts_with(word));
        let after = text[at + symbol.len()..]
            .chars()
            .next()
            .is_none_or(|c| !word(c) || !symbol.ends_with(word));
        before && after
    };
    let mut found = text.match_indices(symbol).map(|(i, _)| i).filter(|&i| whole(i));
    let first = found.next();
    let at = first
        .filter(|&i| from.is_none_or(|f| i >= f))
        .or_else(|| found.find(|&i| from.is_none_or(|f| i >= f)));
    match at.or(first) {
        Some(i) => Ok(ByteIdx(start.0 + i)),
        None => {
            let (shown, cut) = clip_head(&text, MAX_TEXT);
            Err(format!(
                "`{symbol}` is not on line {}; the line is: {shown}{}",
                line + 1,
                if cut { "..." } else { "" }
            ))
        }
    }
}

impl App {
    /// A `Query::Lsp`: resolves the place and asks the file's server; `responder` is answered
    /// when it is time, here for an error.
    pub(super) fn claude_lsp_ask(&mut self, query: &Query, responder: Responder) {
        let Query::Lsp {
            ask,
            path,
            line,
            symbol,
            col,
            include_declaration,
        } = query
        else {
            return responder.answer(Reply::error("not a language server query"));
        };
        if !self.lsp.started() {
            return responder.answer(Reply::error("language servers are off"));
        }
        let ask = Ask {
            responder,
            ask: *ask,
            path: path.clone(),
            line: *line,
            symbol: symbol.clone(),
            col: *col,
            include_declaration: *include_declaration,
            deadline: Instant::now() + self.claude_lsp.timeout,
        };
        self.claude_lsp.next += 1;
        let n = self.claude_lsp.next;
        if let Some(id) = self.buffers.find_path(path) {
            return self.claude_lsp_begin(n, ask, id, false);
        }
        // Not open: loaded as a background tab (the server needs the document), asked once it is.
        if !self.opening.contains_key(path) {
            self.start_open(
                path.clone(),
                OpenRequest {
                    background: true,
                    ..Default::default()
                },
            );
        }
        self.claude_lsp.failed.remove(path);
        self.claude_lsp.loading.insert(n, ask);
    }

    /// The file is open as `id`: resolves the place and asks.
    fn claude_lsp_begin(&mut self, n: u64, ask: Ask, id: crate::buffer::BufferId, opened: bool) {
        let Some(buf) = self.buffers.get(id) else {
            return ask
                .responder
                .answer(Reply::error(format!("{} is not open in jed", ask.path.display())));
        };
        let tab_width = self.settings.for_buffer(buf).tab_width;
        let at = match resolve(buf, tab_width, ask.line - 1, ask.symbol.as_deref(), ask.col) {
            Ok(at) => at,
            Err(e) => return ask.responder.answer(Reply::error(e)),
        };
        let waiting = Waiting {
            responder: ask.responder,
            ask: ask.ask,
            buffer: id,
            revision: buf.revision(),
            at,
            include_declaration: ask.include_declaration,
            dirty: buf.is_dirty(),
            place: self.claude_place(buf, at),
            deadline: ask.deadline,
            retry_at: None,
            parked: None,
            opened,
        };
        self.claude_lsp.waiting.insert(n, waiting);
        if let Err(e) = self.claude_lsp_send(n) {
            self.claude_lsp_finish(n, Reply::error(e));
        }
    }

    /// Why no request could be sent for `buffer`: `Ok(why)` while the server isn't ready (wait),
    /// `Err` for a cause that won't pass.
    fn claude_lsp_why(
        &self,
        buffer: crate::buffer::BufferId,
        method: &str,
        supported: fn(&ServerCapabilities) -> bool,
    ) -> Result<String, String> {
        let Some(buf) = self.buffers.get(buffer) else {
            return Err("the file was closed".to_string());
        };
        let language = buf.syntax.language();
        let name = language.name();
        let covering: Vec<&crate::lsp::Server> = self
            .lsp
            .servers()
            .iter()
            .filter(|s| s.languages.contains(&language))
            .collect();
        if covering.is_empty() {
            return match crate::lsp::config::resolve(language, &self.workspace.config, self.trust, &self.lsp.builtins) {
                _ if self.servers_dirty => Ok("the language server has not started yet".to_string()),
                crate::lsp::config::Resolved::AwaitingTrust => Err(format!(
                    "the workspace is not trusted yet, so its {name} language server is not running (answer the trust prompt in jed)"
                )),
                crate::lsp::config::Resolved::None => Err(format!("no language server for {name}")),
                crate::lsp::config::Resolved::Server(c) => Err(format!(
                    "the {} language server for {name} has exited (see the Tools view)",
                    c.name
                )),
            };
        }
        let state = |s: &crate::lsp::Server| match s.state {
            crate::lsp::State::Starting => "starting",
            crate::lsp::State::Running { .. } => "running",
            crate::lsp::State::Failed => "failed",
        };
        if let Some(s) = covering.iter().find(|s| matches!(s.state, crate::lsp::State::Starting)) {
            return Ok(format!("the {} language server is still starting", s.config.name));
        }
        if !covering
            .iter()
            .any(|s| matches!(s.state, crate::lsp::State::Running { .. }))
        {
            let s = covering[0];
            return Err(format!(
                "the {} language server is not running ({}; see the Tools view)",
                s.config.name,
                state(s)
            ));
        }
        match self.lsp.server_of(buffer) {
            Some((id, _, caps)) if !supported(caps) => {
                let server = self
                    .lsp
                    .servers()
                    .iter()
                    .find(|s| s.id == id)
                    .map_or("the", |s| &s.config.name);
                Err(format!("the {server} language server does not support {method}"))
            }
            _ => Ok("the language server has not opened the file yet".to_string()),
        }
    }

    /// Sends (or sends again) waiting query `n`'s request.
    fn claude_lsp_send(&mut self, n: u64) -> Result<(), String> {
        let Some(w) = self.claude_lsp.waiting.get_mut(&n) else {
            return Ok(());
        };
        w.retry_at = None;
        let (ask, buffer, at, declaration) = (w.ask, w.buffer, w.at, w.include_declaration);
        let (method, supported) = method(ask);
        let extra = match ask {
            LspAsk::References => json!({"context": {"includeDeclaration": declaration}}),
            _ => Value::Null,
        };
        let group = self.groups.active;
        if self.send_request_with(group, buffer, at, method, supported, Request::Claude { ask: n }, extra) {
            if let Some(w) = self.claude_lsp.waiting.get_mut(&n) {
                w.parked = None;
            }
            return Ok(());
        }
        if !self.lsp.started() {
            return Err("language servers are off".to_string());
        }
        let why = self.claude_lsp_why(buffer, method, supported)?;
        if let Some(w) = self.claude_lsp.waiting.get_mut(&n) {
            w.parked = Some(why);
        }
        Ok(())
    }

    /// A file's load finished (`App::opened`): remembers why one Claude waits for failed.
    /// `true`: the file is missing and only Claude's background load wants it, so no tab is
    /// made of the empty new file (`opened` drops the result).
    pub(super) fn claude_lsp_load_result(&mut self, path: &Path, result: &Result<Loaded, LoadError>) -> bool {
        if !self.claude_lsp.loading.values().any(|a| a.path == path) {
            return false;
        }
        let why = match result {
            Err(e) => e.to_string(),
            Ok(Loaded::Binary(_)) => "binary file".to_string(),
            Ok(Loaded::Text(b)) if b.disk().is_none() => "no such file".to_string(),
            Ok(Loaded::Text(_)) => return false,
        };
        self.claude_lsp.failed.insert(path.to_path_buf(), why);
        matches!(result, Ok(Loaded::Text(_)))
            && self
                .opening
                .get(path)
                .is_some_and(|r| r.background && r.edits.is_empty() && r.restore.is_none() && r.backup.is_none())
    }

    /// Answers waiting query `n` and forgets it.
    fn claude_lsp_finish(&mut self, n: u64, reply: Reply) {
        if let Some(w) = self.claude_lsp.waiting.remove(&n) {
            w.responder.answer(reply);
        }
    }

    /// Cancels query `n`'s request in flight (`$/cancelRequest`), if any.
    fn claude_lsp_cancel(&mut self, n: u64) {
        self.cancel_requests(|p| p.request == Request::Claude { ask: n });
    }

    /// After an input event or server event: answers the queries whose buffer changed or went
    /// away, whose server stopped (its request is gone) or whose client hung up.
    pub(super) fn claude_lsp_check(&mut self) {
        self.claude_lsp_check_loading();
        if self.claude_lsp.waiting.is_empty() {
            return;
        }
        let mut done: Vec<(u64, Option<&'static str>)> = Vec::new();
        let mut parked: Vec<u64> = Vec::new();
        for (&n, w) in &self.claude_lsp.waiting {
            let current = self.buffers.get(w.buffer).is_some_and(|b| b.revision() == w.revision);
            let in_flight = self.requests.values().any(|p| p.request == Request::Claude { ask: n });
            if w.responder.is_closed() {
                done.push((n, None));
            } else if !current {
                done.push((n, Some("the file changed while asking: ask again")));
            } else if w.parked.is_some() {
                parked.push(n);
            } else if w.retry_at.is_none() && !in_flight {
                done.push((n, Some("the language server stopped before answering")));
            }
        }
        for (n, why) in done {
            self.claude_lsp_cancel(n);
            match why {
                Some(why) => self.claude_lsp_finish(n, Reply::error(why)),
                None => {
                    tracing::debug!(n, "claude's language server query: nobody waits for it any more");
                    self.claude_lsp.waiting.remove(&n);
                }
            }
        }
        // The server may be ready now.
        for n in parked {
            if let Err(e) = self.claude_lsp_send(n) {
                self.claude_lsp_finish(n, Reply::error(e));
            }
        }
    }

    /// Queries waiting for a file's background tab: asked once it is open; answered when the
    /// load failed or the client hung up.
    fn claude_lsp_check_loading(&mut self) {
        if self.claude_lsp.loading.is_empty() {
            return;
        }
        let ns: Vec<u64> = self.claude_lsp.loading.keys().copied().collect();
        for n in ns {
            let a = &self.claude_lsp.loading[&n];
            if a.responder.is_closed() {
                self.claude_lsp.loading.remove(&n);
            } else if let Some(id) = self.buffers.find_path(&a.path) {
                if let Some(a) = self.claude_lsp.loading.remove(&n) {
                    self.claude_lsp_begin(n, a, id, true);
                }
            } else if !self.opening.contains_key(&a.path)
                && let Some(a) = self.claude_lsp.loading.remove(&n)
            {
                let why = self.claude_lsp.failed.get(&a.path).cloned();
                let why = why.unwrap_or_else(|| "it could not be loaded".to_string());
                a.responder.answer(Reply::error(format!(
                    "{} is not a text file jed can open: {why}",
                    a.path.display()
                )));
            }
        }
        if self.claude_lsp.loading.is_empty() {
            self.claude_lsp.failed.clear();
        }
    }

    /// The timer: queries past their deadline are cancelled and answered, retries sent.
    pub(super) fn claude_lsp_tick(&mut self, now: Instant) {
        let late: Vec<u64> = self
            .claude_lsp
            .loading
            .iter()
            .filter(|(_, a)| now >= a.deadline)
            .map(|(n, _)| *n)
            .collect();
        for n in late {
            if let Some(a) = self.claude_lsp.loading.remove(&n) {
                let after = duration_text(self.claude_lsp.timeout);
                a.responder.answer(Reply::error(format!(
                    "{} did not load in {after}: timed out",
                    a.path.display()
                )));
            }
        }
        let due: Vec<u64> = self
            .claude_lsp
            .waiting
            .iter()
            .filter(|(_, w)| now >= w.deadline || w.retry_at.is_some_and(|r| now >= r))
            .map(|(n, _)| *n)
            .collect();
        for n in due {
            let Some(w) = self.claude_lsp.waiting.get(&n) else {
                continue;
            };
            if now >= w.deadline {
                self.claude_lsp_cancel(n);
                let after = duration_text(self.claude_lsp.timeout);
                let why = match &self.claude_lsp.waiting[&n].parked {
                    Some(why) => format!(" ({why})"),
                    None => String::new(),
                };
                self.claude_lsp_finish(
                    n,
                    Reply::error(format!("the language server did not answer in {after}: timed out{why}")),
                );
            } else if let Err(e) = self.claude_lsp_send(n) {
                self.claude_lsp_finish(n, Reply::error(e));
            }
        }
    }

    /// A `Request::Claude`'s answer from the server.
    pub(super) fn claude_lsp_response(&mut self, p: Pending, n: u64, result: Result<Value, crate::lsp::rpc::RpcError>) {
        let Some(w) = self.claude_lsp.waiting.get_mut(&n) else {
            return;
        };
        if self.buffers.get(p.buffer).is_none_or(|b| b.revision() != w.revision) {
            return self.claude_lsp_finish(n, Reply::error("the file changed while asking: ask again"));
        }
        let value = match result {
            Ok(v) => v,
            Err(e) if e.is_routine() => {
                tracing::debug!(n, code = e.code, "claude's query: server not ready, asking again");
                w.retry_at = Some(Instant::now() + self.claude_lsp.retry);
                return;
            }
            Err(e) => {
                return self.claude_lsp_finish(n, Reply::error(format!("the language server failed: {}", e.message)));
            }
        };
        let (ask, place, dirty, opened) = (w.ask, w.place.clone(), w.dirty, w.opened);
        let Some(buf) = self.buffers.get(p.buffer) else {
            return;
        };
        let mut unread = Vec::new();
        let mut data = match ask {
            LspAsk::Hover => self.claude_hover(buf, &value, p.encoding),
            _ => {
                let (data, rest) = self.claude_locations(&value, p.encoding, ask == LspAsk::References);
                unread = rest;
                data
            }
        };
        data["at"] = place;
        data["dirty"] = json!(dirty);
        if opened {
            data["opened"] = json!(true);
        }
        let busy = self
            .lsp
            .servers()
            .iter()
            .find(|s| s.id == p.server)
            .and_then(|s| s.progress.latest().map(|w| w.label()));
        if let Some(label) = busy {
            data["server_busy"] = json!(label);
        }
        let Some(w) = self.claude_lsp.waiting.remove(&n) else {
            return;
        };
        if unread.is_empty() || tokio::runtime::Handle::try_current().is_err() {
            return w.responder.answer(Reply::data(data));
        }
        // Lines of files that aren't open: read off the UI thread, answered when it reports.
        let mut paths: Vec<PathBuf> = unread.iter().map(|(_, l)| l.path.clone()).collect();
        paths.sort();
        paths.dedup();
        self.claude_lsp.reading.insert(
            n,
            Reading {
                responder: w.responder,
                data,
                unread,
                encoding: p.encoding,
            },
        );
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let files = paths
                .into_iter()
                .filter_map(|path| {
                    let bytes = std::fs::read(&path).ok()?;
                    Some((path, Rope::from_str(&String::from_utf8_lossy(&bytes))))
                })
                .collect();
            let _ = tx.send(AppEvent::Ipc(IpcEvent::ClaudeLspRead { n, files }));
        });
    }

    /// The files read for answer `n`: fills in the lines and ranges of its locations (a file
    /// that couldn't be read keeps `text: null` and the server's own numbers) and answers.
    pub(super) fn claude_lsp_read(&mut self, n: u64, files: Vec<(PathBuf, Rope)>) {
        let Some(mut r) = self.claude_lsp.reading.remove(&n) else {
            return;
        };
        let tab_width = self.settings.tab_width;
        for (i, loc) in &r.unread {
            let Some((_, rope)) = files.iter().find(|(p, _)| *p == loc.path) else {
                continue;
            };
            let slot = &mut r.data["locations"][*i];
            for (k, v) in claude_range(rope, tab_width, loc.start, loc.end, r.encoding)
                .as_object()
                .into_iter()
                .flatten()
            {
                slot[k] = v.clone();
            }
            slot["text"] = json!(line_clip(rope, loc.start.line as usize));
        }
        r.responder.answer(Reply::data(r.data));
    }

    fn claude_hover(&self, buf: &Buffer, value: &Value, encoding: pos::PosEncoding) -> Value {
        let text = value.get("contents").map(hover_text).unwrap_or_default();
        if text.trim().is_empty() {
            return json!({"contents": null});
        }
        let (shown, truncated) = clip_head(&text, MAX_HOVER);
        let mut v = json!({"contents": shown, "truncated": truncated});
        let pos = |p: &Value| {
            Some(LspPos {
                line: p.get("line")?.as_u64()? as u32,
                character: p.get("character")?.as_u64()? as u32,
            })
        };
        if let Some(range) = value.get("range")
            && let (Some(s), Some(e)) = (range.get("start").and_then(pos), range.get("end").and_then(pos))
        {
            let tab_width = self.settings.for_buffer(buf).tab_width;
            v["range"] = claude_range(&buf.rope, tab_width, s, e.max(s), encoding);
        }
        v
    }

    /// The locations of a definition / references answer: at most `MAX_LOCATIONS`, references
    /// sorted by path then line; and those in files that aren't open, whose `text` is still null.
    fn claude_locations(
        &self,
        value: &Value,
        encoding: pos::PosEncoding,
        sort: bool,
    ) -> (Value, Vec<(usize, Location)>) {
        let mut found = locations::parse(value);
        if sort {
            found.sort_by(|a, b| (&a.path, a.start).cmp(&(&b.path, b.start)));
        }
        let count = found.len();
        let mut unread = Vec::new();
        let list: Vec<Value> = found
            .iter()
            .take(MAX_LOCATIONS)
            .enumerate()
            .map(|(i, l)| {
                let (v, open) = self.claude_location(l, encoding);
                if !open {
                    unread.push((i, l.clone()));
                }
                v
            })
            .collect();
        (
            json!({"locations": list, "count": count, "truncated": count > MAX_LOCATIONS}),
            unread,
        )
    }

    /// The location as an answer says it, and whether its file is open (else `text` is null and
    /// the numbers are the server's, until the file is read).
    fn claude_location(&self, loc: &Location, encoding: pos::PosEncoding) -> (Value, bool) {
        let path: PathBuf = loc
            .path
            .strip_prefix(&self.workspace.root)
            .unwrap_or(&loc.path)
            .to_path_buf();
        let path = path.display().to_string();
        let open = self.buffers.find_path(&loc.path).and_then(|id| self.buffers.get(id));
        let Some(buf) = open else {
            let v = json!({
                "path": path,
                "line": loc.start.line + 1,
                "col": loc.start.character + 1,
                "end_line": loc.end.line + 1,
                "end_col": loc.end.character + 1,
                "text": null,
            });
            return (v, false);
        };
        let tab_width = self.settings.for_buffer(buf).tab_width;
        let mut v = claude_range(&buf.rope, tab_width, loc.start, loc.end, encoding);
        v["path"] = json!(path);
        v["text"] = json!(line_clip(&buf.rope, loc.start.line as usize));
        (v, true)
    }
}

/// `{line, col, end_line, end_col}` (1-based, display columns) of a range in `rope`.
fn claude_range(rope: &Rope, tab_width: usize, start: LspPos, end: LspPos, encoding: pos::PosEncoding) -> Value {
    let place = |p: LspPos| {
        let at = pos::from_lsp_pos(rope, p, encoding);
        (pos::line_of(rope, at) + 1, pos::display_col(rope, at, tab_width).0 + 1)
    };
    let ((line, col), (end_line, end_col)) = (place(start), place(end));
    json!({"line": line, "col": col, "end_line": end_line, "end_col": end_col})
}

/// Line `line` (0-based, the last one if past the end) of `rope`, clipped to `MAX_TEXT` bytes.
fn line_clip(rope: &Rope, line: usize) -> String {
    let line = line.min(pos::line_count(rope).saturating_sub(1));
    clip_head(&pos::line_text(rope, line), MAX_TEXT).0.to_string()
}

/// A hover's `contents` as sent: a string, a `MarkupContent` / `MarkedString`, or an array of them.
fn hover_text(contents: &Value) -> String {
    match contents {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or_default().to_string(),
        Value::Array(a) => a
            .iter()
            .map(hover_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::ipc::Request as IpcRequest;
    use crate::lsp::fake::FakeLauncher;
    use crate::milestone9_tests::{location, lsp_project, open, put_cursor};
    use crate::syntax::Language;

    const A: &str = "int foo() { return 1; }\nint main() {\treturn foo() + foo(); }\n";

    /// A query's answer, once it came.
    struct Answer(Arc<Mutex<Option<Reply>>>);

    impl Answer {
        fn get(&self) -> Option<Reply> {
            self.0.lock().unwrap().clone()
        }
    }

    fn ask(app: &mut App, q: Query) -> Answer {
        let (responder, rx) = Responder::pair();
        let slot = Arc::new(Mutex::new(None));
        let put = slot.clone();
        tokio::spawn(async move {
            if let Ok(reply) = rx.await {
                *put.lock().unwrap() = Some(reply);
            }
        });
        app.ipc_request(IpcRequest::query(q, None), responder);
        Answer(slot)
    }

    fn query(ask: LspAsk, path: &Path, line: usize, symbol: Option<&str>, col: Option<usize>) -> Query {
        Query::Lsp {
            ask,
            path: path.to_path_buf(),
            line,
            symbol: symbol.map(String::from),
            col,
            include_declaration: true,
        }
    }

    async fn reply(app: &mut App, answer: &Answer) -> Reply {
        assert!(app.settle(|_| answer.get().is_some()).await, "no answer");
        answer.get().unwrap()
    }

    fn data(r: &Reply) -> &Value {
        assert_eq!(r.error, None);
        r.data.as_ref().expect("data")
    }

    /// No query of Claude's is waiting or in flight (other requests, a pull or semantic tokens, may be).
    fn none_left(app: &App) -> bool {
        app.claude_lsp.waiting.is_empty()
            && !app
                .requests
                .values()
                .any(|p| matches!(p.request, Request::Claude { .. }))
    }

    /// Lets events run for a moment (a late answer arrives meanwhile).
    async fn rest(app: &mut App) {
        let until = tokio::time::Instant::now() + Duration::from_millis(300);
        app.settle(|_| tokio::time::Instant::now() >= until).await;
    }

    fn flush(app: &App) {
        app.lsp
            .server_for(Language::Cpp)
            .unwrap()
            .notify("test/flush", Value::Null);
    }

    /// What the user sees: the cursor, the active tab, the back / forward entries.
    fn user_state(app: &App) -> (usize, crate::buffer::BufferId, usize) {
        let tab = app.group().active_tab().unwrap();
        (
            tab.view.selections.primary().head.0,
            tab.buffer,
            app.nav.history.entries().len(),
        )
    }

    async fn project() -> (tempfile::TempDir, App, Arc<FakeLauncher>, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (app, fake, root) = lsp_project(dir.path(), &[("a.cpp", A), ("b.cpp", "int bar;\n")], false).await;
        (dir, app, fake, root)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn definition_by_symbol_leaves_the_user_alone() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
        put_cursor(&mut app, 3);
        let before = user_state(&app);
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 2, Some("foo"), None));
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        assert_eq!(
            d["locations"],
            json!([{"path": "a.cpp", "line": 1, "col": 5, "end_line": 1, "end_col": 8, "text": "int foo() { return 1; }"}])
        );
        assert_eq!(d["count"], 1);
        assert_eq!(d["dirty"], false);
        // The first `foo` on line 2 (a tab is one character to the server).
        let sent = &fake.requests("textDocument/definition")[0]["position"];
        assert_eq!(sent, &json!({"line": 1, "character": 20}));
        assert_eq!(user_state(&app), before);
        assert!(app.message.is_none(), "nothing is notified");
        assert!(app.locations.is_none(), "no Locations list");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn col_picks_the_second_match_and_a_tab_widens_it() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/hover", json!(null));
        // `int main() {<tab>return foo() + foo(); }`: the second foo is character 28 for the server;
        // the tab fills columns 12..16, so on screen it is display column 31, 1-based 32.
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 2, Some("foo"), Some(32)));
        let r = reply(&mut app, &ans).await;
        assert_eq!(data(&r)["contents"], Value::Null);
        let sent = &fake.requests("textDocument/hover")[0]["position"];
        assert_eq!(sent["character"], 28);
        // No symbol, no column: the first non-blank.
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, None, None));
        reply(&mut app, &ans).await;
        assert_eq!(fake.requests("textDocument/hover")[1]["position"]["character"], 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn symbol_missing_from_the_line_quotes_it() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 1, Some("main"), None));
        let r = reply(&mut app, &ans).await;
        let e = r.error.unwrap();
        assert!(
            e.contains("`main` is not on line 1") && e.contains("int foo() { return 1; }"),
            "{e}"
        );
        // Whole words only: `fo` is inside `foo`.
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 1, Some("fo"), None));
        assert!(reply(&mut app, &ans).await.error.is_some());
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 9, None, None));
        assert!(reply(&mut app, &ans).await.error.unwrap().contains("past the end"));
        assert!(fake.requests("textDocument/definition").is_empty(), "nothing was asked");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dirty_buffer_answers_with_its_own_lines() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        put_cursor(&mut app, 0);
        app.type_text("// c\n");
        fake.respond("textDocument/definition", json!([location(&a, 1, 0, 3)]));
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 2, Some("int"), None));
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        assert_eq!(d["dirty"], true);
        assert_eq!(d["locations"][0]["text"], "int foo() { return 1; }");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn references_in_two_files_sorted_one_not_open() {
        let (_d, mut app, fake, root) = project().await;
        let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
        fake.respond(
            "textDocument/references",
            json!([location(&b, 0, 4, 7), location(&a, 1, 20, 23), location(&a, 0, 4, 7)]),
        );
        let mut q = query(LspAsk::References, &a, 1, Some("foo"), None);
        if let Query::Lsp {
            include_declaration, ..
        } = &mut q
        {
            *include_declaration = false;
        }
        let ans = ask(&mut app, q);
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        let places: Vec<(&str, u64)> = d["locations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| (l["path"].as_str().unwrap(), l["line"].as_u64().unwrap()))
            .collect();
        assert_eq!(places, [("a.cpp", 1), ("a.cpp", 2), ("b.cpp", 1)]);
        assert_eq!(d["count"], 3);
        assert_eq!(d["truncated"], false);
        assert_eq!(d["locations"][1]["text"], "int main() {\treturn foo() + foo(); }");
        assert_eq!(
            d["locations"][2]["text"], "int bar;",
            "a file that isn't open: its line read off-thread"
        );
        assert_eq!(d["locations"][2]["col"], 5);
        assert!(app.buffers.find_path(&b).is_none(), "asking doesn't open the file");
        let sent = &fake.requests("textDocument/references")[0];
        assert_eq!(sent["context"], json!({"includeDeclaration": false}));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hover_text_and_range() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond(
            "textDocument/hover",
            json!({"contents": {"kind": "markdown", "value": "```cpp\nint foo()\n```"},
                   "range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 7}}}),
        );
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        assert_eq!(d["contents"], "```cpp\nint foo()\n```");
        assert_eq!(d["range"], json!({"line": 1, "col": 5, "end_line": 1, "end_col": 8}));
        assert_eq!(d["truncated"], false);
        fake.respond(
            "textDocument/hover",
            json!({"contents": ["a", {"language": "cpp", "value": "b"}, "c"]}),
        );
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        assert_eq!(data(&reply(&mut app, &ans).await)["contents"], "a\n\nb\n\nc");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cursor_moves_and_tab_switches_do_not_cancel_and_several_run_at_once() {
        let (_d, mut app, fake, root) = project().await;
        let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
        fake.respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
        fake.respond("textDocument/hover", json!({"contents": "held"}));
        fake.hold("textDocument/definition", true);
        fake.hold("textDocument/hover", true);
        let first = ask(&mut app, query(LspAsk::Definition, &a, 1, Some("foo"), None));
        let second = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        let third = ask(&mut app, query(LspAsk::Definition, &a, 2, Some("main"), None));
        assert!(
            app.settle(|_| fake.requests("textDocument/definition").len() == 2
                && fake.requests("textDocument/hover").len() == 1)
                .await
        );
        put_cursor(&mut app, 10);
        open(&mut app, &b);
        app.pump();
        assert!(fake.cancelled().is_empty(), "{:?}", fake.cancelled());
        flush(&app);
        for answer in [&first, &second, &third] {
            assert_eq!(reply(&mut app, answer).await.error, None);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_edit_answers_ask_again_and_cancels() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/definition", json!([]));
        fake.hold("textDocument/definition", true);
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 1, Some("foo"), None));
        assert!(
            app.settle(|_| fake.requests("textDocument/definition").len() == 1)
                .await
        );
        put_cursor(&mut app, 0);
        app.type_text("x");
        let r = reply(&mut app, &ans).await;
        assert!(r.error.unwrap().contains("ask again"));
        assert!(app.settle(|_| !fake.cancelled().is_empty()).await);
        assert!(none_left(&app));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_silent_server_times_out_with_a_cancel() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/hover", json!(null));
        fake.hold("textDocument/hover", true);
        app.claude_lsp.timeout = Duration::from_millis(300);
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        let r = reply(&mut app, &ans).await;
        assert!(r.error.unwrap().contains("timed out"), "timeout error");
        assert!(
            app.settle(|_| !fake.cancelled().is_empty()).await,
            "$/cancelRequest sent"
        );
        assert!(none_left(&app));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn content_modified_is_asked_again() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        app.claude_lsp.retry = Duration::from_millis(50);
        fake.fail("textDocument/definition", -32801, "content modified");
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 1, Some("foo"), None));
        assert!(
            app.settle(|_| fake.requests("textDocument/definition").len() == 1)
                .await
        );
        assert!(ans.get().is_none(), "not answered with the routine error");
        fake.unfail("textDocument/definition");
        fake.respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
        let r = reply(&mut app, &ans).await;
        assert_eq!(data(&r)["count"], 1);
        assert_eq!(fake.requests("textDocument/definition").len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn other_errors_are_answered_and_a_closed_client_cancels() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.fail("textDocument/hover", -32603, "boom");
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        assert!(reply(&mut app, &ans).await.error.unwrap().contains("boom"));
        // The client hangs up while the server holds the answer.
        fake.unfail("textDocument/hover");
        fake.respond("textDocument/hover", json!(null));
        fake.hold("textDocument/hover", true);
        let (responder, rx) = Responder::pair();
        let q = query(LspAsk::Hover, &a, 1, Some("foo"), None);
        app.ipc_request(IpcRequest::query(q, None), responder);
        assert!(app.settle(|_| fake.requests("textDocument/hover").len() == 2).await);
        drop(rx);
        app.pump();
        assert!(app.settle(|_| !fake.cancelled().is_empty()).await);
        assert!(app.claude_lsp.waiting.is_empty());
    }

    /// An app on `a.cpp`, a server advertising what `options` say.
    async fn project_with(
        options: crate::lsp::fake::FakeOptions,
    ) -> (tempfile::TempDir, App, Arc<FakeLauncher>, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::milestone9_tests::project(
            dir.path(),
            &[("a.cpp", A), ("b.cpp", "int bar;\n"), ("n.txt", "plain\n")],
        );
        std::fs::write(root.join("bin.cpp"), [0u8, 1, 2, 0, 255]).unwrap();
        let fake = FakeLauncher::new(options);
        let mut app = crate::milestone9_tests::open_app(&root, fake.clone());
        crate::milestone9_tests::open_synced(&mut app, &fake, &root.join("a.cpp")).await;
        (dir, app, fake, root)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_that_isnt_open_loads_as_a_background_tab() {
        let (_d, mut app, fake, root) = project().await;
        let (a, b) = (root.join("a.cpp"), root.join("b.cpp"));
        fake.respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
        put_cursor(&mut app, 3);
        let before = user_state(&app);
        let tabs = app.group().tabs.len();
        let ans = ask(&mut app, query(LspAsk::Definition, &b, 1, Some("bar"), None));
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        assert_eq!(d["opened"], true);
        assert_eq!(d["at"], json!({"line": 1, "col": 5}));
        assert_eq!(d["locations"][0]["path"], "a.cpp");
        assert_eq!(app.group().tabs.len(), tabs + 1, "a background tab");
        assert!(app.buffers.find_path(&b).is_some());
        assert_eq!(
            user_state(&app),
            before,
            "the user's tab, cursor and history are as they were"
        );
        assert!(app.message.is_none(), "nothing is notified");
        let sent = &fake.requests("textDocument/definition")[0];
        assert!(
            sent["textDocument"]["uri"].as_str().unwrap().ends_with("/b.cpp"),
            "{sent}"
        );
        // Asked again, it is open already: no `opened`.
        let ans = ask(&mut app, query(LspAsk::Definition, &b, 1, Some("bar"), None));
        let r = reply(&mut app, &ans).await;
        assert!(data(&r).get("opened").is_none());
        assert_eq!(app.group().tabs.len(), tabs + 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_not_started_yet_is_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::milestone9_tests::project(dir.path(), &[("b.cpp", "int bar;\n")]);
        let fake = FakeLauncher::new(Default::default());
        let mut app = crate::milestone9_tests::open_app(&root, fake.clone());
        assert!(fake.launches().is_empty(), "no file open: no server");
        let b = root.join("b.cpp");
        fake.respond("textDocument/hover", json!(null));
        let ans = ask(&mut app, query(LspAsk::Hover, &b, 1, Some("bar"), None));
        let r = reply(&mut app, &ans).await;
        assert_eq!(data(&r)["opened"], true);
        assert_eq!(fake.launches(), ["clangd"]);
        assert!(none_left(&app));
        app.shutdown_servers().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_file_jed_cant_load_as_text_is_an_error() {
        let (_d, mut app, fake, root) = project_with(Default::default()).await;
        let ans = ask(&mut app, query(LspAsk::Hover, &root.join("missing.cpp"), 1, None, None));
        let e = reply(&mut app, &ans).await.error.unwrap();
        assert!(
            e.contains("missing.cpp") && e.contains("not a text file jed can open"),
            "{e}"
        );
        assert!(
            app.buffers.find_path(&root.join("missing.cpp")).is_none(),
            "no empty new-file tab"
        );
        let ans = ask(&mut app, query(LspAsk::Hover, &root.join("bin.cpp"), 1, None, None));
        let e = reply(&mut app, &ans).await.error.unwrap();
        assert!(e.contains("binary file"), "{e}");
        assert!(none_left(&app));
        assert!(app.claude_lsp.loading.is_empty());
        // Internal on purpose: a stale cause has no other effect (the next ask for the path
        // clears it), so a leak is only visible here.
        assert!(app.claude_lsp.failed.is_empty(), "no failure causes are kept");
        assert!(fake.requests("textDocument/hover").is_empty());
        // The hex tab of a binary file isn't opened either.
        assert!(app.group().find_hex(&root.join("bin.cpp")).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_server_for_the_language_and_a_missing_capability_name_the_cause() {
        let (_d, mut app, _fake, root) = project_with(crate::lsp::fake::FakeOptions {
            no_hover: true,
            ..Default::default()
        })
        .await;
        let ans = ask(
            &mut app,
            query(LspAsk::Hover, &root.join("a.cpp"), 1, Some("foo"), None),
        );
        let e = reply(&mut app, &ans).await.error.unwrap();
        assert!(
            e.contains("clangd") && e.contains("does not support textDocument/hover"),
            "{e}"
        );
        let ans = ask(&mut app, query(LspAsk::Hover, &root.join("n.txt"), 1, None, None));
        let e = reply(&mut app, &ans).await.error.unwrap();
        assert!(e.contains("no language server for Plain Text"), "{e}");
        assert!(none_left(&app));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_crashed_server_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::milestone9_tests::project(dir.path(), &[("a.cpp", A)]);
        let fake = FakeLauncher::new(crate::lsp::fake::FakeOptions {
            crash_after_init: true,
            ..Default::default()
        });
        let mut app = crate::milestone9_tests::open_app(&root, fake.clone());
        open(&mut app, &root.join("a.cpp"));
        assert!(app.settle(|a| a.message.is_some()).await, "the crash is reported");
        let ans = ask(
            &mut app,
            query(LspAsk::Hover, &root.join("a.cpp"), 1, Some("foo"), None),
        );
        let e = reply(&mut app, &ans).await.error.unwrap();
        assert!(
            e.contains("clangd") && e.contains("failed") && e.contains("Tools view"),
            "{e}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_that_never_starts_times_out_naming_why() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::milestone9_tests::project(dir.path(), &[("a.cpp", A)]);
        let fake = FakeLauncher::new(crate::lsp::fake::FakeOptions {
            hold_init: true,
            ..Default::default()
        });
        let mut app = crate::milestone9_tests::open_app(&root, fake.clone());
        let a = root.join("a.cpp");
        open(&mut app, &a);
        assert!(app.settle(|_| !fake.launches().is_empty()).await, "the server launched");
        app.claude_lsp.timeout = Duration::from_millis(300);
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        let e = reply(&mut app, &ans).await.error.unwrap();
        assert!(e.contains("timed out"), "{e}");
        assert!(e.contains("still starting"), "names the parked reason: {e}");
        assert!(none_left(&app));
        assert!(fake.requests("textDocument/hover").is_empty());
        app.shutdown_servers().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_user_open_of_the_file_claude_waits_for_wins() {
        let (_d, mut app, fake, root) = project().await;
        let b = root.join("b.cpp");
        fake.respond("textDocument/hover", json!(null));
        let ans = ask(&mut app, query(LspAsk::Hover, &b, 1, Some("bar"), None));
        // The user opens the same file before the load reports: theirs, not a background tab.
        app.open_file(b.clone(), true, None);
        let r = reply(&mut app, &ans).await;
        assert_eq!(data(&r)["opened"], true);
        let tab = app.group().active_tab().unwrap();
        assert_eq!(app.buffers.find_path(&b), Some(tab.buffer), "the user's open is shown");
        assert!(app.claude_lsp.loading.is_empty() && app.claude_lsp.failed.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_user_open_of_a_missing_file_claude_asked_about_makes_its_new_file_tab() {
        let (_d, mut app, _fake, root) = project().await;
        let m = root.join("missing.cpp");
        let ans = ask(&mut app, query(LspAsk::Hover, &m, 1, None, None));
        app.open_file(m.clone(), true, None);
        reply(&mut app, &ans).await;
        assert!(
            app.settle(|a| a.buffers.find_path(&m).is_some()).await,
            "the user's new-file tab"
        );
        let tab = app.group().active_tab().unwrap();
        assert_eq!(app.buffers.find_path(&m), Some(tab.buffer), "shown to the user");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_user_opening_a_missing_file_still_makes_a_new_file_tab() {
        let (_d, mut app, _fake, root) = project().await;
        let m = root.join("missing.cpp");
        app.open_file(m.clone(), true, None);
        assert!(app.settle(|a| a.buffers.find_path(&m).is_some()).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_busy_server_is_named_in_the_answer() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/hover", json!(null));
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        assert!(data(&reply(&mut app, &ans).await).get("server_busy").is_none());
        let server = app.lsp.server_for(Language::Cpp).unwrap();
        server.notify(
            "test/progress",
            json!({"token": "t", "value": {"kind": "begin", "title": "Indexing", "percentage": 40}}),
        );
        assert!(
            app.settle(|a| a.lsp.server_for(Language::Cpp).unwrap().progress.latest().is_some())
                .await
        );
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        let r = reply(&mut app, &ans).await;
        let busy = data(&r)["server_busy"].as_str().unwrap();
        assert!(busy.contains("Indexing"), "{busy}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_location_in_a_file_that_cant_be_read_has_no_text() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        let gone = root.join("gone.cpp");
        fake.respond(
            "textDocument/definition",
            json!([location(&gone, 2, 1, 4), location(&a, 0, 4, 7)]),
        );
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 2, Some("foo"), None));
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        assert_eq!(d["locations"][0]["text"], Value::Null);
        assert_eq!(d["locations"][0]["line"], 3, "the server's numbers stay");
        assert_eq!(d["locations"][1]["text"], "int foo() { return 1; }");
        assert_eq!(d["count"], 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hover_and_location_lists_are_capped() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/hover", json!({"contents": "x".repeat(4096 + 500)}));
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        assert_eq!(d["contents"].as_str().unwrap().len(), 4096);
        assert_eq!(d["truncated"], true);
        let many: Vec<Value> = (0..250).map(|i| location(&a, 0, i % 20, i % 20 + 1)).collect();
        fake.respond("textDocument/references", json!(many));
        let ans = ask(&mut app, query(LspAsk::References, &a, 1, Some("foo"), None));
        let r = reply(&mut app, &ans).await;
        let d = data(&r);
        assert_eq!(d["locations"].as_array().unwrap().len(), 200);
        assert_eq!(d["count"], 250);
        assert_eq!(d["truncated"], true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_long_line_is_clipped_in_a_location() {
        let dir = tempfile::tempdir().unwrap();
        let long = format!("int {};\n", "v".repeat(400));
        let (mut app, fake, root) = lsp_project(dir.path(), &[("a.cpp", long.as_str())], false).await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 1, None, None));
        let r = reply(&mut app, &ans).await;
        assert_eq!(data(&r)["locations"][0]["text"].as_str().unwrap().len(), 200);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_late_answer_after_the_edit_or_the_timeout_is_dropped() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/definition", json!([location(&a, 0, 4, 7)]));
        fake.hold("textDocument/definition", true);
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 1, Some("foo"), None));
        assert!(
            app.settle(|_| fake.requests("textDocument/definition").len() == 1)
                .await
        );
        put_cursor(&mut app, 0);
        app.type_text("x");
        let first = reply(&mut app, &ans).await;
        assert!(first.error.unwrap().contains("ask again"));
        // The server answers anyway: nothing changes, nobody is answered twice.
        flush(&app);
        rest(&mut app).await;
        assert!(none_left(&app));
        assert!(ans.get().unwrap().error.unwrap().contains("ask again"));
        // Same for a timeout.
        app.claude_lsp.timeout = Duration::from_millis(200);
        let ans = ask(&mut app, query(LspAsk::Definition, &a, 1, Some("foo"), None));
        assert!(reply(&mut app, &ans).await.error.unwrap().contains("timed out"));
        flush(&app);
        rest(&mut app).await;
        assert!(ans.get().unwrap().error.unwrap().contains("timed out"));
        assert!(app.claude_lsp.waiting.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stopped_server_answers_the_waiting_queries() {
        let (_d, mut app, fake, root) = project().await;
        let a = root.join("a.cpp");
        fake.respond("textDocument/hover", json!(null));
        fake.hold("textDocument/hover", true);
        let ans = ask(&mut app, query(LspAsk::Hover, &a, 1, Some("foo"), None));
        assert!(app.settle(|_| fake.requests("textDocument/hover").len() == 1).await);
        let server = app.lsp.server_for(Language::Cpp).unwrap().id;
        app.lsp_event(crate::lsp::Event::Exited {
            server,
            crashed: Some("died".into()),
        });
        let r = reply(&mut app, &ans).await;
        assert!(r.error.unwrap().contains("stopped"));
        assert!(app.claude_lsp.waiting.is_empty());
    }
}
