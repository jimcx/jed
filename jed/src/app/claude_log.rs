//! The MCP log (`Tool::Claude`): what `jed claude mcp` and the hooks forward as log lines over
//! the task socket (`ipc::LogLine`) becomes `→` / `←` / lifecycle lines in "claude (mcp)", and
//! the open logging connections are the Tools view row's session count. A session is a
//! connection that sent an "in" / "out" line; the server reports its close (`ipc::CLOSED`).

use serde_json::Value;

use super::App;
use crate::forge::http::clean_line;
use crate::ipc::{CLOSED, LogLine};
use crate::logs::{self, Level, Tool};

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
                    self.logs
                        .push(&tool, Level::Debug, &capped_json(&log.msg, logs::MAX_LINE));
                }
            }
        }
    }
}

/// `→ [mcp 12] tools/call get_command_output {"terminal":2}`: the method (a tool call's name
/// after it) and its parameters cut short; a line that wasn't JSON is a warning.
fn request_line(who: &str, msg: &Value) -> (Level, String) {
    let Some(method) = msg["method"].as_str() else {
        return (
            Level::Warn,
            format!("→ {who} unreadable: {}", capped_json(msg, PARAMS_CAP)),
        );
    };
    let params = &msg["params"];
    let (name, args) = match (method, params["name"].as_str()) {
        ("tools/call", Some(name)) => (format!("{method} {name}"), &params["arguments"]),
        _ => (method.to_string(), params),
    };
    let args = if args.is_null() {
        String::new()
    } else {
        format!(" {}", capped_json(args, PARAMS_CAP))
    };
    (Level::Info, format!("→ {who} {name}{args}"))
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

/// A writer that keeps the first `cap` bytes and fails after, so serializing a huge value
/// stops early rather than building all of it.
struct Capped {
    buf: Vec<u8>,
    cap: usize,
    cut: bool,
}

impl std::io::Write for Capped {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let room = self.cap - self.buf.len();
        if data.len() > room {
            self.buf.extend_from_slice(&data[..room]);
            self.cut = true;
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `value` as compact JSON, at most about `cap` bytes (`…` after a cut).
fn capped_json(value: &Value, cap: usize) -> String {
    let mut w = Capped {
        buf: Vec::new(),
        cap,
        cut: false,
    };
    let _ = serde_json::to_writer(&mut w, value);
    let mut s = match String::from_utf8(w.buf) {
        Ok(s) => s,
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            let mut b = e.into_bytes();
            b.truncate(valid);
            String::from_utf8(b).unwrap_or_default()
        }
    };
    if w.cut {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_lines_name_the_method_and_cut_the_params() {
        let msg = json!({"method": "tools/call", "params": {"name": "get_x", "arguments": {"terminal": 2}}});
        assert_eq!(
            request_line("[mcp 1]", &msg),
            (Level::Info, "→ [mcp 1] tools/call get_x {\"terminal\":2}".into())
        );
        let long = json!({"method": "m", "params": {"a": "é".repeat(500)}});
        let (_, line) = request_line("[mcp]", &long);
        assert!(line.len() < PARAMS_CAP + 30 && line.ends_with('…'), "{line}");
        assert_eq!(request_line("[mcp]", &json!("junk")).0, Level::Warn);
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

    #[test]
    fn capped_json_stops_at_the_cap_on_a_character_boundary() {
        let big = json!({"text": "é".repeat(100_000)});
        let s = capped_json(&big, 1000);
        assert!(s.len() <= 1000 + 3 && s.ends_with('…'));
        assert_eq!(capped_json(&json!({"a": 1}), 1000), "{\"a\":1}");
    }
}
