//! The Claude Code CLI modes (`jed claude mcp`, `jed claude hook session-start|prompt`): they run
//! before logging and raw mode, with blocking std I/O, inside a jed terminal (`JED_IPC` names
//! the socket, `JED_TERMINAL` the terminal). Here is the formatting and the stdio loop over
//! the [`Jed`] trait, so tests use an in-memory reader / writer and a fake jed; the real one is
//! [`Socket`] over `ipc::client::Connection`.
//!
//! `mcp` is a minimal MCP server: newline-delimited JSON-RPC 2.0 on stdin / stdout (stdout is
//! the protocol channel, so diagnostics go to stderr). Every message read or written is also
//! forwarded to jed as a log-only line. Both hooks print nothing and exit 0 on any failure
//! (outside jed, a dead socket): they must never get in the way of a prompt.

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::ipc::client::Connection;
use crate::ipc::{self, LogLine, LspAsk, Query, Request};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    SessionStart,
    Prompt,
}

/// `jed claude …`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cli {
    Mcp,
    Hook(HookKind),
}

pub const USAGE: &str = "usage: jed claude mcp | jed claude hook session-start | jed claude hook prompt";

/// Parses what follows `claude` on the command line.
pub fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["mcp"] => Ok(Cli::Mcp),
        ["hook", "session-start"] => Ok(Cli::Hook(HookKind::SessionStart)),
        ["hook", "prompt"] => Ok(Cli::Hook(HookKind::Prompt)),
        _ => Err(USAGE.into()),
    }
}

/// What the CLI modes ask of jed.
pub trait Jed {
    /// A query's answer data, or why there is none (including "not inside a jed terminal").
    fn ask(&mut self, query: Query) -> Result<Value, String>;
    /// Forwards a protocol message to jed's log (`ms`: the time spent answering, on "out"), or
    /// a hook's result as `dir` "hook"; best effort.
    fn log(&mut self, dir: &str, msg: Value, ms: Option<u64>);
}

const NOT_IN_JED: &str = "not inside a jed terminal (JED_IPC is not set or jed is gone)";
/// Longest wait for one answer.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The real [`Jed`]: one connection for the process's lifetime, made lazily, remade once when
/// it breaks.
pub struct Socket {
    path: Option<PathBuf>,
    terminal: Option<u64>,
    conn: Option<Connection>,
}

impl Socket {
    /// From the values of `JED_IPC` and `JED_TERMINAL`.
    pub fn new(socket: Option<std::ffi::OsString>, terminal: Option<std::ffi::OsString>) -> Socket {
        Socket {
            path: socket.filter(|s| !s.is_empty()).map(PathBuf::from),
            terminal: terminal.and_then(|t| t.to_str()?.trim().parse().ok()),
            conn: None,
        }
    }

    pub fn from_env() -> Socket {
        Socket::new(std::env::var_os(ipc::ENV), std::env::var_os(ipc::TERMINAL_ENV))
    }

    /// Whether there is a socket to try.
    pub fn inside_jed(&self) -> bool {
        self.path.is_some()
    }

    /// Blocking: one round trip, reconnecting once when the connection is gone.
    fn send(&mut self, request: &Request) -> Result<ipc::Reply, String> {
        let path = self.path.clone().ok_or(NOT_IN_JED)?;
        let mut last = String::new();
        for _ in 0..2 {
            if self.conn.is_none() {
                match Connection::connect(&path, Some(TIMEOUT)) {
                    Ok(c) => self.conn = Some(c),
                    Err(e) => return Err(format!("{NOT_IN_JED}: {e}")),
                }
            }
            if let Some(conn) = self.conn.as_mut() {
                match conn.request(request) {
                    Ok(reply) => return Ok(reply),
                    Err(e) => {
                        last = e.to_string();
                        self.conn = None;
                    }
                }
            }
        }
        Err(format!("jed: {last}"))
    }
}

impl Jed for Socket {
    fn ask(&mut self, query: Query) -> Result<Value, String> {
        let reply = self.send(&Request::query(query, self.terminal))?;
        match reply.error {
            Some(e) => Err(e),
            None => Ok(reply.data.unwrap_or(Value::Null)),
        }
    }

    fn log(&mut self, dir: &str, msg: Value, ms: Option<u64>) {
        if !self.inside_jed() {
            return;
        }
        let request = Request {
            log: Some(LogLine {
                dir: dir.into(),
                msg,
                ms,
            }),
            terminal: self.terminal,
            pid: Some(std::process::id()),
            ..Request::default()
        };
        let _ = self.send(&request);
    }
}

// ---- MCP ----

/// The MCP revision answered when the client names none.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// The stateless protocol revision (no `initialize`; every request carries its version in `_meta`). Every method
/// here already works without a handshake, so `server/discover` can offer it.
const STATELESS_VERSION: &str = "2026-07-28";
const VERSION_META: &str = "io.modelcontextprotocol/protocolVersion";

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
    })
}

/// The tools `tools/list` offers.
pub fn tools() -> Vec<Value> {
    let terminal = json!({"type": "integer", "description": "terminal number, from list_terminals"});
    vec![
        tool(
            "get_active_editor",
            "The file focused in jed: path, language, dirty flag, 1-based cursor, selection ranges and the selected text.",
            json!({}),
            &[],
        ),
        tool(
            "get_open_editors",
            "Every editor group's tabs in jed (files, diffs, shells), marking the active one.",
            json!({}),
            &[],
        ),
        tool(
            "get_diagnostics",
            "Language server diagnostics of one open file, else of every open file, errors first.",
            json!({"path": {"type": "string", "description": "file path; default: every open file"}}),
            &[],
        ),
        tool(
            "find_definition",
            "Ask jed's language server where a symbol is defined, without moving the user's cursor. Name the place by `line` and `symbol` (the name on that line). Returns locations {path, line, col, end_line, end_col, text}; `dirty: true` when the buffer has unsaved changes (the lines are the buffer's). The file must be open in jed.",
            json!({
                "path": {"type": "string", "description": "file path"},
                "line": {"type": "integer", "description": "1-based line"},
                "symbol": {"type": "string", "description": "the name on that line (first whole-word match); better than col"},
                "col": {"type": "integer", "description": "1-based column (as get_active_editor reports): picks the match at or after it when the name is there twice"},
                "kind": {"type": "string", "enum": ["definition", "type", "implementation", "declaration"], "description": "default definition"},
            }),
            &["path", "line"],
        ),
        tool(
            "find_references",
            "Ask jed's language server for the references to a symbol (up to 200, sorted by path then line), without moving the user's cursor. Place named as in find_definition.",
            json!({
                "path": {"type": "string", "description": "file path"},
                "line": {"type": "integer", "description": "1-based line"},
                "symbol": {"type": "string", "description": "the name on that line (first whole-word match); better than col"},
                "col": {"type": "integer", "description": "1-based column (as get_active_editor reports): picks the match at or after it when the name is there twice"},
                "include_declaration": {"type": "boolean", "description": "default true"},
            }),
            &["path", "line"],
        ),
        tool(
            "get_hover",
            "Ask jed's language server for the hover text (type, docs; markdown as sent, capped at 4 KB) and the range it covers, without showing a popup. Place named as in find_definition.",
            json!({
                "path": {"type": "string", "description": "file path"},
                "line": {"type": "integer", "description": "1-based line"},
                "symbol": {"type": "string", "description": "the name on that line (first whole-word match); better than col"},
                "col": {"type": "integer", "description": "1-based column (as get_active_editor reports): picks the match at or after it when the name is there twice"},
            }),
            &["path", "line"],
        ),
        tool(
            "get_environment",
            "jed's version, workspace, git branch and dirty count, open files, language servers, task templates and shells.",
            json!({}),
            &[],
        ),
        tool(
            "list_terminals",
            "jed's shell tabs and task runs: name, cwd, running or idle, last exit code, and which one is yours (self).",
            json!({}),
            &[],
        ),
        tool(
            "get_recent_commands",
            "The last commands run in a jed terminal (default: every shell but yours) with exit codes and ids.",
            json!({"terminal": terminal, "n": {"type": "integer", "description": "how many (default 10)"}}),
            &[],
        ),
        tool(
            "get_command_output",
            "The output of one command from get_recent_commands (its last lines).",
            json!({
                "terminal": terminal,
                "command_id": {"type": "integer", "description": "command id, from get_recent_commands"},
                "max_lines": {"type": "integer", "description": "default 200"},
            }),
            &["terminal", "command_id"],
        ),
        tool(
            "get_terminal_screen",
            "A terminal's visible screen: a TUI, or a half-typed prompt.",
            json!({"terminal": terminal}),
            &["terminal"],
        ),
        tool(
            "open_file",
            "Open a file in jed's active group, optionally at a 1-based line and column.",
            json!({
                "path": {"type": "string"},
                "line": {"type": "integer"},
                "col": {"type": "integer"},
            }),
            &["path"],
        ),
        tool(
            "get_review",
            "The code review of the checked-out branch's merge request, as jed last loaded it (no forge call): number, title, \
             URL, branches, state, unresolved count, whether local is ahead / behind / diverged, the files with +/-, the user's \
             pending drafts (yours are marked by_claude), when it was fetched, and why there is none. Start here to deal with \
             review feedback. `read_only: true` means the user forbids the write tools below (mcp_read_only in forges.toml).",
            json!({}),
            &[],
        ),
        tool(
            "get_review_threads",
            "The review threads to act on, so you can fix the code yourself: for each unresolved thread its id, file, side, \
             live 1-based line range in the file on disk (or outdated / missing with the original lines), every comment \
             (author, time, body), parsed suggestions (the lines replaced and the new lines), whether the user wrote the last \
             comment, and your reply draft if any. Edit the files with your own tools, then draft a reply per thread.",
            json!({
                "path": {"type": "string", "description": "only this file's threads (relative to the repository)"},
                "resolved": {"type": "boolean", "description": "also list resolved threads (default false)"},
            }),
            &[],
        ),
        tool(
            "review_refresh",
            "Ask jed to look the merge request up again (new comments, a push). Returns at once: call get_review afterwards.",
            json!({}),
            &[],
        ),
        tool(
            "review_apply_suggestion",
            "Apply a thread's suggested change to the file and save it (one undo step in jed). Refused when the file has unsaved \
             changes in jed, the suggestion is out of date, or the review is read-only for you. Nothing is sent to the forge.",
            json!({"thread": {"type": "string", "description": "thread id, from get_review_threads"}}),
            &["thread"],
        ),
        tool(
            "review_draft_reply",
            "Queue a reply to a review thread as a draft in jed's PENDING list, tagged claude (optionally also resolving the \
             thread). It is NOT posted: only the user's own Submit Review in jed sends it. One draft per thread: a second call \
             replaces it. Refused when the review is read-only for you.",
            json!({
                "thread": {"type": "string", "description": "thread id, from get_review_threads"},
                "body": {"type": "string", "description": "the reply text"},
                "resolve": {"type": "boolean", "description": "also resolve the thread when the user submits (default false)"},
            }),
            &["thread", "body"],
        ),
    ]
}

fn arg_u64(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or(format!("`{key}` must be a non-negative integer")),
    }
}

fn need_u64(args: &Value, key: &str) -> Result<u64, String> {
    arg_u64(args, key)?.ok_or(format!("`{key}` is required"))
}

fn arg_bool(args: &Value, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or(format!("`{key}` must be true or false")),
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().map(Some).ok_or(format!("`{key}` must be a string")),
    }
}

/// The `Query::Lsp` a place-naming tool call asks (`path`, `line`, `symbol`, `col`).
fn lsp_place(args: &Value, ask: LspAsk, absolute: &dyn Fn(&str) -> PathBuf) -> Result<Query, String> {
    let line = need_u64(args, "line")?;
    if line == 0 {
        return Err("`line` is 1-based".into());
    }
    if arg_u64(args, "col")? == Some(0) {
        return Err("`col` is 1-based".into());
    }
    Ok(Query::Lsp {
        ask,
        path: absolute(arg_str(args, "path")?.ok_or("`path` is required")?),
        line: line as usize,
        symbol: arg_str(args, "symbol")?.filter(|s| !s.is_empty()).map(String::from),
        col: arg_u64(args, "col")?.map(|n| n as usize),
        include_declaration: arg_bool(args, "include_declaration")?.unwrap_or(true),
    })
}

/// The query a tool call asks: `None` for an unknown tool, `Err` for bad arguments. Relative
/// paths are made absolute against `cwd`.
pub(crate) fn tool_query(name: &str, args: &Value, cwd: &Path) -> Option<Result<Query, String>> {
    let absolute = |p: &str| {
        let p = Path::new(p);
        if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
    };
    let query = || -> Result<Option<Query>, String> {
        Ok(Some(match name {
            "get_active_editor" => Query::Context,
            "get_open_editors" => Query::Editors,
            "get_diagnostics" => Query::Diagnostics {
                path: arg_str(args, "path")?.map(absolute),
            },
            "find_definition" => {
                let ask = match arg_str(args, "kind")?.unwrap_or("definition") {
                    "definition" => LspAsk::Definition,
                    "type" => LspAsk::TypeDefinition,
                    "implementation" => LspAsk::Implementation,
                    "declaration" => LspAsk::Declaration,
                    other => {
                        return Err(format!(
                            "`kind` must be definition, type, implementation or declaration, not {other}"
                        ));
                    }
                };
                lsp_place(args, ask, &absolute)?
            }
            "find_references" => lsp_place(args, LspAsk::References, &absolute)?,
            "get_hover" => lsp_place(args, LspAsk::Hover, &absolute)?,
            "get_environment" => Query::Environment,
            "list_terminals" => Query::Terminals,
            "get_recent_commands" => Query::Commands {
                terminal: arg_u64(args, "terminal")?,
                n: arg_u64(args, "n")?.map(|n| n as usize),
            },
            "get_command_output" => Query::Output {
                terminal: need_u64(args, "terminal")?,
                command: need_u64(args, "command_id")?,
                max_lines: arg_u64(args, "max_lines")?.map(|n| n as usize),
            },
            "get_terminal_screen" => Query::Screen {
                terminal: need_u64(args, "terminal")?,
            },
            "open_file" => Query::Open {
                path: absolute(arg_str(args, "path")?.ok_or("`path` is required")?),
                line: arg_u64(args, "line")?.map(|n| n as usize),
                col: arg_u64(args, "col")?.map(|n| n as usize),
            },
            "get_review" => Query::Review,
            "get_review_threads" => Query::ReviewThreads {
                path: arg_str(args, "path")?.map(String::from),
                resolved: arg_bool(args, "resolved")?.unwrap_or(false),
            },
            "review_refresh" => Query::ReviewRefresh,
            "review_apply_suggestion" => Query::ReviewApply {
                thread: arg_str(args, "thread")?.ok_or("`thread` is required")?.to_string(),
            },
            "review_draft_reply" => Query::ReviewReply {
                thread: arg_str(args, "thread")?.ok_or("`thread` is required")?.to_string(),
                body: arg_str(args, "body")?.ok_or("`body` is required")?.to_string(),
                resolve: arg_bool(args, "resolve")?.unwrap_or(false),
            },
            _ => return Ok(None),
        }))
    };
    query().transpose()
}

fn text_result(text: String, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The response to one JSON-RPC message; `None` for a notification (or a stray response).
pub fn handle(msg: &Value, jed: &mut dyn Jed, cwd: &Path) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str)?;
    let id = msg.get("id").cloned()?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "jed", "version": env!("JED_VERSION")},
            }))
        }
        "server/discover" => {
            let supported = [STATELESS_VERSION, PROTOCOL_VERSION];
            match params
                .pointer("/_meta")
                .and_then(|m| m.get(VERSION_META))
                .and_then(Value::as_str)
            {
                Some(v) if !supported.contains(&v) => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32022,
                        "message": "Unsupported protocol version",
                        "data": {"supported": supported, "requested": v},
                    },
                })),
                _ => ok(json!({
                    "resultType": "complete",
                    "supportedVersions": supported,
                    "capabilities": {"tools": {}},
                    "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "jed", "version": env!("JED_VERSION")}},
                })),
            }
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match tool_query(name, &args, cwd) {
                None => Some(rpc_error(id, -32602, &format!("unknown tool {name}"))),
                Some(Err(e)) => ok(text_result(e, true)),
                Some(Ok(query)) => ok(match jed.ask(query) {
                    Ok(data) => text_result(data.to_string(), false),
                    Err(e) => text_result(e, true),
                }),
            }
        }
        _ => Some(rpc_error(id, -32601, &format!("method not found: {method}"))),
    }
}

/// Blocking: serves MCP on `input` / `output` until the input ends.
pub fn serve(input: impl BufRead, mut output: impl Write, jed: &mut dyn Jed, cwd: &Path) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let mut ms = 0;
        let (msg, response) = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => {
                let started = std::time::Instant::now();
                let response = handle(&msg, jed, cwd);
                ms = started.elapsed().as_millis() as u64;
                (msg, response)
            }
            Err(e) => (
                Value::String(line),
                Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
            ),
        };
        // The reply goes out before the log round trips, so they never delay the client.
        if let Some(response) = &response {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
        jed.log("in", msg, None);
        if let Some(response) = response {
            jed.log("out", response, Some(ms));
        }
    }
    Ok(())
}

// ---- hooks ----

/// A hook's stdout: nothing on any failure.
pub fn hook_text(kind: HookKind, jed: &mut dyn Jed) -> String {
    let (name, text) = match kind {
        HookKind::SessionStart => ("session-start", session_start(jed)),
        HookKind::Prompt => ("prompt", prompt(jed)),
    };
    let note = match &text {
        Ok(t) => format!("{name} → {} lines", t.lines().count()),
        Err(e) => format!("{name} → nothing ({e})"),
    };
    jed.log("hook", Value::String(note), None);
    text.unwrap_or_default()
}

fn strings(v: &Value) -> Vec<&str> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn session_start(jed: &mut dyn Jed) -> Result<String, String> {
    let env = jed.ask(Query::Environment)?;
    let mut out = String::new();
    let version = env["version"].as_str().unwrap_or("?");
    out.push_str(&format!(
        "You are running inside a terminal of jed {version}, a terminal text editor.\n"
    ));
    let roots = strings(&env["workspace"]);
    if !roots.is_empty() {
        out.push_str(&format!("Workspace: {}\n", roots.join(", ")));
    }
    if let Some(branch) = env["git"]["branch"].as_str() {
        let dirty = env["git"]["dirty"].as_u64().unwrap_or(0);
        out.push_str(&format!("Git: branch {branch}, {dirty} changed file(s)\n"));
    }
    let files = strings(&env["open_files"]);
    if !files.is_empty() {
        out.push_str(&format!("Open files: {}\n", files.join(", ")));
    }
    let servers: Vec<&str> = env["servers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["state"] == "running")
        .filter_map(|s| s["name"].as_str())
        .collect();
    if !servers.is_empty() {
        out.push_str(&format!("Language servers running: {}\n", servers.join(", ")));
    }
    let templates = strings(&env["task_templates"]);
    if !templates.is_empty() {
        out.push_str(&format!("Task templates: {}\n", templates.join(", ")));
    }
    if let Some(name) = env["task_environment"]["name"].as_str() {
        let vars = strings(&env["task_environment"]["variables"]);
        out.push_str(&format!("Task environment: {name} ({} variable(s))\n", vars.len()));
    }
    let shells: Vec<String> = env["shells"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            format!(
                "{} (terminal {}, {})",
                s["name"].as_str().unwrap_or("shell"),
                s["id"],
                s["cwd"].as_str().unwrap_or("")
            )
        })
        .collect();
    if !shells.is_empty() {
        out.push_str(&format!("Shells: {}\n", shells.join("; ")));
    }
    out.push_str(
        "The jed MCP tools show the editor's state (active file, selection, diagnostics, open tabs) and the \
         other terminals' commands and output, and open files in the editor. The review tools (get_review, \
         get_review_threads, review_apply_suggestion, review_draft_reply) deal with a merge request's feedback: fix the code \
         yourself; replies are only drafts the user submits in jed.\n",
    );
    Ok(out)
}

/// Selected text up to this size goes into the prompt context.
const MAX_SELECTED: usize = 2048;

fn prompt(jed: &mut dyn Jed) -> Result<String, String> {
    let mut out = String::new();
    let context = jed.ask(Query::Context)?;
    let editor = &context["editor"];
    if editor.is_object() {
        let path = editor["relative"]
            .as_str()
            .or(editor["path"].as_str())
            .or(editor["name"].as_str())
            .unwrap_or("?");
        let cursor = &editor["cursor"];
        out.push_str(&format!("Active file: {path}:{}:{}", cursor["line"], cursor["col"]));
        let selection = &editor["selection"];
        if selection["empty"] == false {
            let range = &editor["ranges"][0];
            let (a, b) = (&range["start"]["line"], &range["end"]["line"]);
            out.push_str(&format!(" (selection {a}\u{2013}{b})"));
        }
        out.push('\n');
        if let Some(text) = selection["text"].as_str()
            && selection["empty"] == false
            && selection["truncated"] == false
            && text.len() < MAX_SELECTED
        {
            out.push_str(&format!("Selected text:\n{text}\n"));
        }
    }
    let review = &context["review"];
    if review.is_object() {
        let n = |k: &str| review[k].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "Review: !{} \"{}\", {} unresolved thread{}, {} pending draft{}\n",
            review["number"],
            review["title"].as_str().unwrap_or(""),
            n("unresolved"),
            if n("unresolved") == 1 { "" } else { "s" },
            n("pending"),
            if n("pending") == 1 { "" } else { "s" },
        ));
    }
    let query = Query::Commands {
        terminal: None,
        n: Some(1),
    };
    if let Ok(commands) = jed.ask(query) {
        for term in commands["terminals"].as_array().into_iter().flatten() {
            let Some(last) = term["commands"].get(0) else { continue };
            let Some(line) = last["command"].as_str() else { continue };
            let state = if last["running"] == true {
                "running".to_string()
            } else {
                match last["exit"].as_i64() {
                    Some(code) => format!("exit {code}"),
                    None => "finished".to_string(),
                }
            };
            out.push_str(&format!(
                "Terminal {} ({}) last command: {line} ({state})\n",
                term["terminal"],
                term["name"].as_str().unwrap_or("shell")
            ));
        }
    }
    Ok(out)
}

/// Blocking: runs a CLI mode; the exit status.
pub fn run(cli: Cli) -> std::process::ExitCode {
    let mut jed = Socket::from_env();
    match cli {
        Cli::Mcp => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let stdin = io::stdin();
            match serve(stdin.lock(), io::stdout().lock(), &mut jed, &cwd) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("jed claude mcp: {e}");
                    std::process::ExitCode::FAILURE
                }
            }
        }
        Cli::Hook(kind) => {
            if jed.inside_jed() {
                // The event Claude Code sends: not needed, but don't leave it unread.
                let stdin = io::stdin();
                if !stdin.is_terminal() {
                    let _ = stdin.lock().take(1 << 20).read_to_end(&mut Vec::new());
                }
                let text = hook_text(kind, &mut jed);
                if !text.is_empty() {
                    let mut out = io::stdout().lock();
                    let _ = out.write_all(text.as_bytes());
                    let _ = out.flush();
                }
            }
            std::process::ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fake jed: answers by query, records queries and logs.
    #[derive(Default)]
    pub struct Fake {
        pub answers: Vec<(Query, Result<Value, String>)>,
        pub asked: Vec<Query>,
        pub logs: Vec<(String, Value)>,
    }

    impl Jed for Fake {
        fn ask(&mut self, query: Query) -> Result<Value, String> {
            self.asked.push(query.clone());
            self.answers
                .iter()
                .find(|(q, _)| *q == query)
                .map(|(_, r)| r.clone())
                .unwrap_or(Err(NOT_IN_JED.into()))
        }
        fn log(&mut self, dir: &str, msg: Value, _ms: Option<u64>) {
            self.logs.push((dir.into(), msg));
        }
    }

    fn lines_of(out: Vec<u8>) -> Vec<Value> {
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    pub(crate) fn run_mcp(lines: &[Value], jed: &mut dyn Jed) -> Vec<Value> {
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, jed, Path::new("/work")).unwrap();
        lines_of(out)
    }

    pub(crate) fn call(id: u64, name: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
    }

    #[test]
    fn parses_the_subcommands() {
        let p = |a: &[&str]| parse_cli(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(p(&["mcp"]), Ok(Cli::Mcp));
        assert_eq!(p(&["hook", "prompt"]), Ok(Cli::Hook(HookKind::Prompt)));
        assert_eq!(p(&["hook", "session-start"]), Ok(Cli::Hook(HookKind::SessionStart)));
        assert!(p(&[]).is_err() && p(&["hook"]).is_err() && p(&["mcp", "x"]).is_err());
    }

    #[test]
    fn lsp_tools_map_to_queries() {
        let q = |name: &str, args: Value| tool_query(name, &args, Path::new("/work")).unwrap();
        assert_eq!(
            q(
                "find_definition",
                json!({"path": "a.rs", "line": 3, "symbol": "foo", "kind": "type"})
            ),
            Ok(Query::Lsp {
                ask: LspAsk::TypeDefinition,
                path: PathBuf::from("/work/a.rs"),
                line: 3,
                symbol: Some("foo".into()),
                col: None,
                include_declaration: true,
            })
        );
        assert_eq!(
            q(
                "find_references",
                json!({"path": "/x/a.rs", "line": 1, "col": 4, "include_declaration": false})
            ),
            Ok(Query::Lsp {
                ask: LspAsk::References,
                path: PathBuf::from("/x/a.rs"),
                line: 1,
                symbol: None,
                col: Some(4),
                include_declaration: false,
            })
        );
        assert!(matches!(
            q("get_hover", json!({"path": "a", "line": 2})),
            Ok(Query::Lsp { ask: LspAsk::Hover, .. })
        ));
        assert!(q("get_hover", json!({"path": "a"})).unwrap_err().contains("line"));
        assert!(q("get_hover", json!({"path": "a", "line": 0})).is_err());
        assert!(q("find_definition", json!({"path": "a", "line": 1, "kind": "x"})).is_err());
    }

    #[test]
    fn handshake_list_and_notifications() {
        let mut jed = Fake::default();
        let out = run_mcp(
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}}),
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "nope"}),
            ],
            &mut jed,
        );
        assert_eq!(out.len(), 4, "the notification gets no reply");
        assert_eq!(out[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "jed");
        assert!(out[0]["result"]["capabilities"]["tools"].is_object());
        assert_eq!(out[1]["id"], 2);
        let names: Vec<&str> = out[2]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "get_active_editor",
                "get_open_editors",
                "get_diagnostics",
                "find_definition",
                "find_references",
                "get_hover",
                "get_environment",
                "list_terminals",
                "get_recent_commands",
                "get_command_output",
                "get_terminal_screen",
                "open_file",
                "get_review",
                "get_review_threads",
                "review_refresh",
                "review_apply_suggestion",
                "review_draft_reply"
            ]
        );
        assert_eq!(out[3]["error"]["code"], -32601);
        assert!(jed.asked.is_empty(), "the handshake asks jed nothing");
    }

    #[test]
    fn server_discover_lists_versions_and_rejects_an_unknown_one() {
        let mut jed = Fake::default();
        let req = |id: u64, v: &str| json!({"jsonrpc": "2.0", "id": id, "method": "server/discover", "params": {"_meta": {VERSION_META: v}}});
        let out = run_mcp(&[req(1, STATELESS_VERSION), req(2, "1900-01-01")], &mut jed);
        assert_eq!(out[0]["result"]["resultType"], "complete");
        assert_eq!(
            out[0]["result"]["supportedVersions"],
            json!([STATELESS_VERSION, PROTOCOL_VERSION])
        );
        assert!(out[0]["result"]["capabilities"]["tools"].is_object());
        assert_eq!(
            out[0]["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "jed"
        );
        assert_eq!(out[1]["error"]["code"], -32022);
        assert_eq!(out[1]["error"]["data"]["requested"], "1900-01-01");
        assert!(jed.asked.is_empty());
    }

    #[test]
    fn tools_call_asks_jed_and_wraps_the_answer() {
        let mut jed = Fake {
            answers: vec![
                (Query::Terminals, Ok(json!({"terminals": []}))),
                (
                    Query::Output {
                        terminal: 2,
                        command: 14,
                        max_lines: Some(5),
                    },
                    Err("no command 14".into()),
                ),
                (
                    Query::Open {
                        path: "/work/src/a.rs".into(),
                        line: Some(3),
                        col: None,
                    },
                    Ok(json!({"opened": true})),
                ),
            ],
            ..Fake::default()
        };
        let out = run_mcp(
            &[
                call(1, "list_terminals", json!({})),
                call(
                    2,
                    "get_command_output",
                    json!({"terminal": 2, "command_id": 14, "max_lines": 5}),
                ),
                call(3, "open_file", json!({"path": "src/a.rs", "line": 3})),
                call(4, "get_terminal_screen", json!({})),
                call(5, "nope", json!({})),
            ],
            &mut jed,
        );
        assert_eq!(out[0]["result"]["isError"], false);
        assert_eq!(out[0]["result"]["content"][0]["text"], r#"{"terminals":[]}"#);
        assert_eq!(out[1]["result"]["isError"], true);
        assert_eq!(out[1]["result"]["content"][0]["text"], "no command 14");
        assert_eq!(out[2]["result"]["isError"], false, "relative path made absolute");
        assert_eq!(out[3]["result"]["isError"], true);
        assert!(
            out[3]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("terminal")
        );
        assert_eq!(out[4]["error"]["code"], -32602);
        assert_eq!(jed.asked.len(), 3, "bad arguments never reach jed");
    }

    #[test]
    fn stdout_is_one_json_line_per_request_with_the_id_verbatim() {
        let mut jed = Fake {
            answers: vec![(
                Query::Terminals,
                Ok(json!({"text": "a\nb\u{2028}\"c\"", "big": "x".repeat(200_000)})),
            )],
            ..Fake::default()
        };
        let input = [
            r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#,
            "{broken",
            r#"[{"jsonrpc":"2.0","id":9,"method":"ping"}]"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"list_terminals"}}"#,
            "",
            r#"{"jsonrpc":"2.0","id":11,"method":"ping"}"#,
        ]
        .join("\n"); // no trailing newline: EOF ends the loop cleanly
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, &mut jed, Path::new("/work")).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.ends_with('\n'));
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let ids: Vec<&Value> = lines.iter().map(|l| &l["id"]).collect();
        assert_eq!(
            ids,
            [
                &json!("abc"),
                &Value::Null,
                &Value::Null,
                &json!(1.5),
                &json!(8),
                &json!(10),
                &json!(11)
            ]
        );
        assert_eq!(
            lines[1]["error"]["code"], -32700,
            "a malformed line is answered, then serving goes on"
        );
        assert_eq!(lines[2]["result"], json!({}), "a null id is echoed");
        assert_eq!(lines[4]["error"]["code"], -32602, "tools/call without a name");
        let body = lines[5]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(body.len() > 200_000 && lines[5]["result"]["content"][0]["type"] == "text");
    }

    #[test]
    fn tool_schemas_are_valid_objects_with_known_required_properties() {
        for t in tools() {
            let name = t["name"].as_str().unwrap();
            assert!(!t["description"].as_str().unwrap_or("").is_empty(), "{name}");
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object", "{name}");
            let props = schema["properties"].as_object().expect(name);
            for r in schema["required"].as_array().into_iter().flatten() {
                assert!(
                    props.contains_key(r.as_str().unwrap()),
                    "{name}: required {r} not a property"
                );
            }
        }
    }

    #[test]
    fn outside_jed_tool_calls_are_errors_but_the_handshake_works() {
        let mut jed = Socket::new(None, None);
        assert!(!jed.inside_jed());
        let out = run_mcp(
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
                call(2, "get_environment", json!({})),
            ],
            &mut jed,
        );
        assert!(out[0]["result"]["tools"].is_array());
        assert_eq!(out[1]["result"]["isError"], true);
        let text = out[1]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not inside a jed terminal"));
    }

    #[test]
    fn every_message_in_and_out_is_forwarded_to_the_log() {
        let mut jed = Fake::default();
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        let out = run_mcp(std::slice::from_ref(&req), &mut jed);
        assert_eq!(
            jed.logs,
            vec![("in".to_string(), req), ("out".to_string(), out[0].clone())]
        );
        let mut jed = Fake::default();
        let mut sink = Vec::new();
        serve("not json\n".as_bytes(), &mut sink, &mut jed, Path::new("/")).unwrap();
        assert!(String::from_utf8(sink).unwrap().contains("-32700"));
        assert_eq!(jed.logs.len(), 2);
    }

    #[test]
    fn hooks_are_silent_when_jed_cannot_answer() {
        let mut jed = Fake::default();
        assert_eq!(hook_text(HookKind::SessionStart, &mut jed), "");
        assert_eq!(hook_text(HookKind::Prompt, &mut jed), "");
        let mut socket = Socket::new(None, None);
        assert_eq!(hook_text(HookKind::Prompt, &mut socket), "");
        let mut dead = Socket::new(Some("/nonexistent/jed.sock".into()), Some("3".into()));
        assert_eq!(hook_text(HookKind::SessionStart, &mut dead), "");
    }

    #[test]
    fn session_start_lists_the_environment() {
        let mut jed = Fake {
            answers: vec![(
                Query::Environment,
                Ok(json!({
                    "version": "1.2",
                    "workspace": ["/work"],
                    "git": {"branch": "main", "dirty": 3, "loaded": true},
                    "open_files": ["a.rs", "b.rs"],
                    "servers": [{"name": "rust-analyzer", "state": "running"}, {"name": "x", "state": "failed"}],
                    "task_templates": ["build"],
                    "task_environment": {"name": "dev", "variables": ["A"]},
                    "shells": [{"id": 2, "name": "sh", "cwd": "/work"}],
                })),
            )],
            ..Fake::default()
        };
        let text = hook_text(HookKind::SessionStart, &mut jed);
        for want in [
            "jed 1.2",
            "Workspace: /work",
            "branch main, 3 changed",
            "a.rs, b.rs",
            "rust-analyzer",
            "build",
            "dev (1 variable",
            "terminal 2",
            "MCP tools",
        ] {
            assert!(text.contains(want), "{want} in {text}");
        }
        assert!(!text.contains(", x"));
    }

    #[test]
    fn prompt_hook_has_the_cursor_selection_and_other_shells() {
        let mut jed = Fake {
            answers: vec![
                (
                    Query::Context,
                    Ok(json!({"editor": {
                        "relative": "src/a.rs", "cursor": {"line": 42, "col": 5},
                        "selection": {"empty": false, "text": "let x;", "truncated": false},
                        "ranges": [{"start": {"line": 40, "col": 1}, "end": {"line": 48, "col": 2}}],
                    }})),
                ),
                (
                    Query::Commands {
                        terminal: None,
                        n: Some(1),
                    },
                    Ok(json!({"terminals": [
                        {"terminal": 3, "name": "sh", "commands": [{"command": "cargo test", "running": false, "exit": 101}]},
                        {"terminal": 4, "name": "idle", "commands": []},
                    ]})),
                ),
            ],
            ..Fake::default()
        };
        let text = hook_text(HookKind::Prompt, &mut jed);
        assert!(
            text.contains("Active file: src/a.rs:42:5 (selection 40\u{2013}48)"),
            "{text}"
        );
        assert!(text.contains("Selected text:\nlet x;"));
        assert!(text.contains("Terminal 3 (sh) last command: cargo test (exit 101)"));
        assert!(!text.contains("Terminal 4"));
    }

    #[test]
    fn review_tools_map_to_queries_and_the_prompt_hook_has_a_counts_only_review_line() {
        let cwd = Path::new("/w");
        let q = |name: &str, args: Value| tool_query(name, &args, cwd).unwrap().unwrap();
        assert_eq!(q("get_review", json!({})), Query::Review);
        assert_eq!(
            q("get_review_threads", json!({"path": "a.rs", "resolved": true})),
            Query::ReviewThreads {
                path: Some("a.rs".into()),
                resolved: true
            }
        );
        assert_eq!(q("review_refresh", json!({})), Query::ReviewRefresh);
        assert_eq!(
            q("review_apply_suggestion", json!({"thread": "11"})),
            Query::ReviewApply { thread: "11".into() }
        );
        assert_eq!(
            q(
                "review_draft_reply",
                json!({"thread": "11", "body": "Done", "resolve": true})
            ),
            Query::ReviewReply {
                thread: "11".into(),
                body: "Done".into(),
                resolve: true
            }
        );
        assert!(
            tool_query("review_draft_reply", &json!({"thread": "1"}), cwd)
                .unwrap()
                .is_err()
        );
        assert!(
            tool_query("review_submit", &json!({}), cwd).is_none(),
            "there is no submit tool"
        );
        let mut jed = Fake {
            answers: vec![(
                Query::Context,
                Ok(
                    json!({"editor": null, "review": {"number": 42, "title": "Add review panel", "unresolved": 3, "pending": 1}}),
                ),
            )],
            ..Fake::default()
        };
        let text = hook_text(HookKind::Prompt, &mut jed);
        assert!(
            text.contains("Review: !42 \"Add review panel\", 3 unresolved threads, 1 pending draft"),
            "{text}"
        );
    }
}
