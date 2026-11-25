//! The MCP log (`Tool::Claude`): what `jed claude mcp` and the hooks forward as log lines over
//! the task socket (`ipc::LogLine`) becomes `→` / `←` / lifecycle lines in "claude (mcp)", and
//! the open logging connections are the Tools view row's session count. A session is a
//! connection that sent an "in" / "out" line; the server reports its close (`ipc::CLOSED`).

use serde_json::Value;

use super::App;
use crate::forge::http::clean_line;
use crate::ipc::{CLOSED, LogLine};
use crate::logs::{Level, Tool};

/// One connected `jed claude mcp` process.
pub struct Session {
    conn: u64,
    pid: Option<u32>,
}

/// `[mcp 12]` (`[hook 12]`), without the pid when it isn't known.
fn who(kind: &str, pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("[{kind} {pid}]"),
        None => format!("[{kind}]"),
    }
}

/// `LogLine::dir` parsed (the wire carries a string).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    In,
    Out,
    Hook,
    Closed,
}

impl Dir {
    fn parse(dir: &str) -> Option<Dir> {
        Some(match dir {
            "in" => Dir::In,
            "out" => Dir::Out,
            "hook" => Dir::Hook,
            CLOSED => Dir::Closed,
            _ => return None,
        })
    }
}

/// Longest request parameters shown on a `→` line.
const PARAMS_CAP: usize = 200;

impl App {
    /// How many MCP sessions are connected (the Tools view row's count).
    pub fn claude_sessions(&self) -> usize {
        self.ipc.sessions.len()
    }

    /// A log line from a client: pushes it to the MCP log and tracks the sessions.
    pub(super) fn claude_log(&mut self, log: &LogLine, conn: u64, pid: Option<u32>) {
        let tool = Tool::Claude;
        let Some(dir) = Dir::parse(&log.dir) else {
            return;
        };
        match dir {
            Dir::Hook => {
                let text = cut(log.msg.as_str().unwrap_or_default(), PARAMS_CAP);
                self.logs
                    .push(&tool, Level::Info, &format!("{} {text}", who("hook", pid)));
            }
            Dir::Closed => {
                if let Some(i) = self.ipc.sessions.iter().position(|s| s.conn == conn) {
                    let s = self.ipc.sessions.remove(i);
                    self.logs
                        .push(&tool, Level::Info, &format!("{} disconnected", who("mcp", s.pid)));
                    self.needs_redraw = true;
                }
            }
            Dir::In | Dir::Out => {
                if !self.ipc.sessions.iter().any(|s| s.conn == conn) {
                    self.logs
                        .push(&tool, Level::Info, &format!("{} connected", who("mcp", pid)));
                    self.ipc.sessions.push(Session { conn, pid });
                    self.needs_redraw = true;
                }
                let who = who("mcp", pid);
                if dir == Dir::In {
                    let (level, line) = request_line(&who, &log.msg);
                    self.logs.push(&tool, level, &line);
                    if log.msg["method"] == "initialize"
                        && let Some(info) = log.msg["params"]["clientInfo"].as_object()
                    {
                        let name = cut(info.get("name").and_then(Value::as_str).unwrap_or("?"), 80);
                        let version = cut(info.get("version").and_then(Value::as_str).unwrap_or(""), 80);
                        self.logs
                            .push(&tool, Level::Info, format!("{who} client {name} {version}").trim_end());
                    }
                } else {
                    let (level, line) = response_line(&who, &log.msg, log.ms);
                    self.logs.push(&tool, level, &line);
                }
            }
        }
    }
}

/// `→ [mcp 12] get_command_output terminal 2 command 5`: a tool call's name and what it
/// touches (`targets`), any other method's name alone; a line without a method is a warning
/// naming at most its JSON-RPC id (never its text).
fn request_line(who: &str, msg: &Value) -> (Level, String) {
    let Some(method) = msg["method"].as_str() else {
        return (
            Level::Warn,
            match &msg["id"] {
                Value::Number(id) => format!("→ {who} unreadable (id {id})"),
                _ => format!("→ {who} unreadable"),
            },
        );
    };
    let params = &msg["params"];
    let line = match (method, params["name"].as_str()) {
        ("tools/call", Some(name)) => {
            let targets = targets(&params["arguments"]);
            format!("→ {who} {}{targets}", cut(name, PARAMS_CAP))
        }
        _ => format!("→ {who} {}", cut(method, PARAMS_CAP)),
    };
    (Level::Info, line)
}

/// The files, shells and review threads a tool call's arguments name (` src/a.rs:12`,
/// ` terminal 2 command 5`, ` thread abc`), each cut and cleaned; queries, bodies and the rest
/// are left out.
fn targets(args: &Value) -> String {
    let text = |key: &str| match &args[key] {
        Value::String(s) => Some(cut(s, PARAMS_CAP)),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let mut out = String::new();
    if let Some(path) = text("path") {
        out.push(' ');
        out.push_str(&path);
        if let Some(line) = text("line").or_else(|| text("start")) {
            out.push(':');
            out.push_str(&line);
        }
    }
    for (key, label) in [
        ("terminal", "terminal"),
        ("command_id", "command"),
        ("thread", "thread"),
        ("draft", "draft"),
    ] {
        if let Some(value) = text(key) {
            out.push_str(&format!(" {label} {value}"));
        }
    }
    out
}

/// `← [mcp 12] 182 lines, 7.9 KB, 2 ms` for a tool result (its text's size), or the error:
/// a JSON-RPC `error` or a result with `isError`, at Error level (they count on the badge).
fn response_line(who: &str, msg: &Value, ms: Option<u64>) -> (Level, String) {
    let time = ms.map_or(String::new(), |ms| format!(", {ms} ms"));
    if let Some(error) = msg.get("error") {
        let code = error["code"].as_i64().unwrap_or(0);
        let text = error["message"].as_str().unwrap_or("");
        return (
            Level::Error,
            format!("← {who} error {code}: {}{time}", cut(text, PARAMS_CAP)),
        );
    }
    let content = msg["result"]["content"].as_array();
    let texts = || content.into_iter().flatten().filter_map(|c| c["text"].as_str());
    if msg["result"]["isError"] == true {
        let first = texts().next().unwrap_or("");
        return (
            Level::Error,
            format!("← {who} tool error: {}{time}", cut(first, PARAMS_CAP)),
        );
    }
    let (lines, bytes) = texts().fold((0, 0), |(l, b), t| (l + t.lines().count(), b + t.len()));
    let size = if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    };
    let what = if content.is_some() {
        format!("{lines} lines, {size}")
    } else {
        "ok".to_string()
    };
    (Level::Info, format!("← {who} {what}{time}"))
}

/// `s`'s first line cut to `cap` bytes at a character boundary, `…` after a cut, control
/// characters removed (`forge::http::clean_line`): text from a client can't start a line of
/// its own or move the cursor.
fn cut(s: &str, cap: usize) -> String {
    let s = s.split(['\n', '\r']).next().unwrap_or("");
    if s.len() <= cap {
        return clean_line(s);
    }
    let mut out = clean_line(&s[..s.floor_char_boundary(cap)]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_lines_name_the_tool_and_only_what_it_touches() {
        let call = |name: &str, args: Value| {
            request_line(
                "[mcp 1]",
                &json!({"method": "tools/call", "params": {"name": name, "arguments": args}}),
            )
            .1
        };
        assert_eq!(
            call(
                "get_command_output",
                json!({"terminal": 2, "command_id": 5, "max_lines": 50})
            ),
            "→ [mcp 1] get_command_output terminal 2 command 5"
        );
        assert_eq!(
            call(
                "find_references",
                json!({"path": "src/a.rs", "line": 12, "symbol": "x"})
            ),
            "→ [mcp 1] find_references src/a.rs:12"
        );
        assert_eq!(
            call(
                "review_draft_reply",
                json!({"thread": "t1", "body": "long text", "resolve": true})
            ),
            "→ [mcp 1] review_draft_reply thread t1"
        );
        assert_eq!(
            call(
                "review_draft_comment",
                json!({"path": "b.rs", "side": "new", "start": 3, "end": 4, "body": "x"})
            ),
            "→ [mcp 1] review_draft_comment b.rs:3"
        );
        assert_eq!(
            call("find_workspace_symbols", json!({"query": "Foo"})),
            "→ [mcp 1] find_workspace_symbols"
        );
        assert_eq!(
            call("open_file", json!({"path": "a\n→ forged"})),
            "→ [mcp 1] open_file a"
        );
        let init = json!({"method": "initialize", "params": {"clientInfo": {"name": "c"}}});
        assert_eq!(request_line("[mcp]", &init), (Level::Info, "→ [mcp] initialize".into()));
        assert_eq!(
            request_line("[mcp]", &json!("secret junk")),
            (Level::Warn, "→ [mcp] unreadable".into())
        );
        assert_eq!(
            request_line("[mcp]", &json!({"id": 7, "params": {"body": "secret"}})),
            (Level::Warn, "→ [mcp] unreadable (id 7)".into())
        );
    }

    #[test]
    fn cut_keeps_one_clean_line() {
        assert_eq!(cut("a\nb", 10), "a");
        assert_eq!(cut("a\r\x1b[2Jb", 10), "a");
        assert_eq!(cut("a\x1b[2Jb", 10), "a[2Jb");
        assert_eq!(cut("abcdef", 3), "abc…");
        assert_eq!(cut("", 3), "");
    }

    #[test]
    fn response_lines_summarise_results_and_flag_errors() {
        let ok = json!({"result": {"content": [{"type": "text", "text": "a\nb\nc"}], "isError": false}});
        assert_eq!(
            response_line("[mcp]", &ok, Some(2)),
            (Level::Info, "← [mcp] 3 lines, 5 B, 2 ms".into())
        );
        let rpc = json!({"error": {"code": -32601, "message": "no such method"}});
        assert_eq!(response_line("[mcp]", &rpc, None).0, Level::Error);
        let tool = json!({"result": {"content": [{"text": "boom"}], "isError": true}});
        let (level, line) = response_line("[mcp]", &tool, Some(1));
        assert_eq!((level, line.as_str()), (Level::Error, "← [mcp] tool error: boom, 1 ms"));
    }
}
