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
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lsp_types::{DeclarationCapability, HoverProviderCapability, ImplementationProviderCapability, OneOf};
use lsp_types::{ServerCapabilities, TypeDefinitionProviderCapability};
use serde_json::{Value, json};

use super::App;
use super::claude::clip_head;
use super::lsp::{Pending, Request};
use crate::buffer::Buffer;
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
            },
        );
    }

    /// The earliest deadline or retry among the waiting queries.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.waiting
            .values()
            .map(|w| w.retry_at.map_or(w.deadline, |r| r.min(w.deadline)))
            .min()
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
        let Some(id) = self.buffers.find_path(path) else {
            return responder.answer(Reply::error(format!(
                "{} is not open in jed: open it first (open_file)",
                path.display()
            )));
        };
        let Some(buf) = self.buffers.get(id) else {
            return responder.answer(Reply::error(format!("{} is not open in jed", path.display())));
        };
        let tab_width = self.settings.for_buffer(buf).tab_width;
        let at = match resolve(buf, tab_width, line - 1, symbol.as_deref(), *col) {
            Ok(at) => at,
            Err(e) => return responder.answer(Reply::error(e)),
        };
        let now = Instant::now();
        let waiting = Waiting {
            responder,
            ask: *ask,
            buffer: id,
            revision: buf.revision(),
            at,
            include_declaration: *include_declaration,
            dirty: buf.is_dirty(),
            place: self.claude_place(buf, at),
            deadline: now + self.claude_lsp.timeout,
            retry_at: None,
        };
        self.claude_lsp.next += 1;
        let n = self.claude_lsp.next;
        self.claude_lsp.waiting.insert(n, waiting);
        if let Err(e) = self.claude_lsp_send(n) {
            self.claude_lsp_finish(n, Reply::error(e));
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
            return Ok(());
        }
        Err(if !self.lsp.started() {
            "language servers are off".to_string()
        } else {
            format!("no running language server with this file open supports {method} (see the Tools view)")
        })
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
        if self.claude_lsp.waiting.is_empty() {
            return;
        }
        let mut done: Vec<(u64, Option<&'static str>)> = Vec::new();
        for (&n, w) in &self.claude_lsp.waiting {
            let current = self.buffers.get(w.buffer).is_some_and(|b| b.revision() == w.revision);
            let in_flight = self.requests.values().any(|p| p.request == Request::Claude { ask: n });
            if w.responder.is_closed() {
                done.push((n, None));
            } else if !current {
                done.push((n, Some("the file changed while asking: ask again")));
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
    }

    /// The timer: queries past their deadline are cancelled and answered, retries sent.
    pub(super) fn claude_lsp_tick(&mut self, now: Instant) {
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
                self.claude_lsp_finish(
                    n,
                    Reply::error(format!("the language server did not answer in {after}: timed out")),
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
        let (ask, place, dirty) = (w.ask, w.place.clone(), w.dirty);
        let Some(buf) = self.buffers.get(p.buffer) else {
            return;
        };
        let mut data = match ask {
            LspAsk::Hover => self.claude_hover(buf, &value, p.encoding),
            _ => self.claude_locations(&value, p.encoding, ask == LspAsk::References),
        };
        data["at"] = place;
        data["dirty"] = json!(dirty);
        self.claude_lsp_finish(n, Reply::data(data));
    }

    /// `{path, line, col, end_line, end_col, text}` of a range in `buf`.
    fn claude_range_in(&self, buf: &Buffer, start: LspPos, end: LspPos, encoding: pos::PosEncoding) -> Value {
        let (a, b) = (
            pos::from_lsp_pos(&buf.rope, start, encoding),
            pos::from_lsp_pos(&buf.rope, end, encoding),
        );
        let (from, to) = (self.claude_place(buf, a), self.claude_place(buf, b));
        json!({"line": from["line"], "col": from["col"], "end_line": to["line"], "end_col": to["col"]})
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
            v["range"] = self.claude_range_in(buf, s, e.max(s), encoding);
        }
        v
    }

    /// The locations of a definition / references answer: at most `MAX_LOCATIONS`, references
    /// sorted by path then line. Lines of files that aren't open have no `text` yet.
    fn claude_locations(&self, value: &Value, encoding: pos::PosEncoding, sort: bool) -> Value {
        let mut found = locations::parse(value);
        if sort {
            found.sort_by(|a, b| (&a.path, a.start).cmp(&(&b.path, b.start)));
        }
        let count = found.len();
        let list: Vec<Value> = found
            .iter()
            .take(MAX_LOCATIONS)
            .map(|l| self.claude_location(l, encoding))
            .collect();
        json!({"locations": list, "count": count, "truncated": count > MAX_LOCATIONS})
    }

    fn claude_location(&self, loc: &Location, encoding: pos::PosEncoding) -> Value {
        let path: PathBuf = loc
            .path
            .strip_prefix(&self.workspace.root)
            .unwrap_or(&loc.path)
            .to_path_buf();
        let path = path.display().to_string();
        let open = self.buffers.find_path(&loc.path).and_then(|id| self.buffers.get(id));
        let Some(buf) = open else {
            return json!({
                "path": path,
                "line": loc.start.line + 1,
                "col": loc.start.character + 1,
                "end_line": loc.end.line + 1,
                "end_col": loc.end.character + 1,
                "text": null,
            });
        };
        let mut v = self.claude_range_in(buf, loc.start, loc.end, encoding);
        let line = (loc.start.line as usize).min(pos::line_count(&buf.rope).saturating_sub(1));
        let text = pos::line_text(&buf.rope, line);
        v["path"] = json!(path);
        v["text"] = json!(clip_head(&text, MAX_TEXT).0);
        v
    }
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
            d["locations"][2]["text"],
            Value::Null,
            "a file that isn't open: no text yet"
        );
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
        // A file that isn't open (the next subtask loads it).
        let ans = ask(&mut app, query(LspAsk::Hover, &root.join("b.cpp"), 1, None, None));
        assert!(reply(&mut app, &ans).await.error.unwrap().contains("not open"));
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
