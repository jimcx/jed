//! JSON-RPC 2.0 messages and LSP's base protocol framing (`Content-Length` headers over a byte
//! stream).

use serde_json::{Value, json};

/// A request id: ours are numbers; a server's may be strings, and are echoed back as they came.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Id {
    Num(i64),
    Str(String),
}

impl Id {
    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.as_i64().map(Id::Num),
            Value::String(s) => Some(Id::Str(s.clone())),
            _ => None,
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Id::Num(n) => json!(n),
            Id::Str(s) => json!(s),
        }
    }
}

/// A response's `error`.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

pub const METHOD_NOT_FOUND: i64 = -32601;

/// One message, either way.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request { id: Id, method: String, params: Value },
    Response { id: Id, result: Result<Value, RpcError> },
    Notification { method: String, params: Value },
}

impl Message {
    pub fn request(id: i64, method: &str, params: Value) -> Self {
        Message::Request {
            id: Id::Num(id),
            method: method.into(),
            params,
        }
    }

    pub fn notification(method: &str, params: Value) -> Self {
        Message::Notification {
            method: method.into(),
            params,
        }
    }

    /// Classifies a decoded body; `None` when it is none of the three shapes.
    pub fn from_value(mut v: Value) -> Option<Self> {
        let obj = v.as_object_mut()?;
        let params = obj.remove("params").unwrap_or(Value::Null);
        let id = obj.get("id").and_then(Id::from_value);
        match (obj.remove("method"), id) {
            (Some(Value::String(method)), Some(id)) => Some(Message::Request { id, method, params }),
            (Some(Value::String(method)), None) => Some(Message::Notification { method, params }),
            (None, Some(id)) => {
                let result = match obj.remove("error") {
                    Some(e) => Err(RpcError {
                        code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: e.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                    }),
                    None => Ok(obj.remove("result").unwrap_or(Value::Null)),
                };
                Some(Message::Response { id, result })
            }
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Message::Request { id, method, params } => {
                json!({ "jsonrpc": "2.0", "id": id.to_value(), "method": method, "params": params })
            }
            Message::Notification { method, params } if params.is_null() => {
                json!({ "jsonrpc": "2.0", "method": method })
            }
            Message::Notification { method, params } => json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            Message::Response { id, result: Ok(r) } => json!({ "jsonrpc": "2.0", "id": id.to_value(), "result": r }),
            Message::Response { id, result: Err(e) } => json!({
                "jsonrpc": "2.0",
                "id": id.to_value(),
                "error": { "code": e.code, "message": e.message },
            }),
        }
    }

    /// The framed bytes: header, blank line, JSON body.
    #[cfg(test)] // the client frames its logged body with `encode_body`
    pub fn encode(&self) -> Vec<u8> {
        encode_body(self.to_value().to_string().as_bytes())
    }
}

pub fn encode_body(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// The stream can't be framed any more: the server is broken.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameError {
    MissingLength,
    BadLength(String),
    BadHeader(String),
    /// No blank line within `MAX_HEADER` bytes.
    HeaderTooLong,
    /// A `Content-Length` over `MAX_BODY`.
    TooBig(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::MissingLength => write!(f, "message without Content-Length"),
            FrameError::BadLength(v) => write!(f, "bad Content-Length {v:?}"),
            FrameError::BadHeader(h) => write!(f, "bad header line {h:?}"),
            FrameError::HeaderTooLong => write!(f, "header too long"),
            FrameError::TooBig(n) => write!(f, "message of {n} bytes is too big"),
        }
    }
}

const MAX_HEADER: usize = 8 * 1024;
const MAX_BODY: usize = 256 * 1024 * 1024;

/// Splits a byte stream into message bodies: feed it whatever reads return (`push`), then take
/// the complete bodies (`next`). Partial headers and bodies wait for more bytes.
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
    /// The body length once the current message's header is read (the header is then dropped).
    body_len: Option<usize>,
}

impl Framer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete body, `None` until one is buffered. After an error the stream is lost.
    pub fn next(&mut self) -> Option<Result<Vec<u8>, FrameError>> {
        if self.body_len.is_none() {
            let Some(end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                return (self.buf.len() > MAX_HEADER).then_some(Err(FrameError::HeaderTooLong));
            };
            let len = match parse_header(&self.buf[..end]) {
                Ok(len) => len,
                Err(e) => return Some(Err(e)),
            };
            self.buf.drain(..end + 4);
            self.body_len = Some(len);
        }
        let len = self.body_len?;
        if self.buf.len() < len {
            return None;
        }
        self.body_len = None;
        let rest = self.buf.split_off(len);
        Some(Ok(std::mem::replace(&mut self.buf, rest)))
    }
}

/// The `Content-Length` of a header block (lines without the final blank one). Other headers
/// (`Content-Type`) are ignored; names are case-insensitive.
fn parse_header(header: &[u8]) -> Result<usize, FrameError> {
    let text = String::from_utf8_lossy(header);
    let mut len = None;
    for line in text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            return Err(FrameError::BadHeader(line.to_string()));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let value = value.trim();
            let n: usize = value.parse().map_err(|_| FrameError::BadLength(value.to_string()))?;
            if n > MAX_BODY {
                return Err(FrameError::TooBig(n));
            }
            len = Some(n);
        }
    }
    len.ok_or(FrameError::MissingLength)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bodies(framer: &mut Framer) -> Vec<String> {
        std::iter::from_fn(|| framer.next())
            .map(|b| String::from_utf8(b.unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn partial_reads_wait_for_the_rest() {
        let bytes = encode_body(r#"{"a":"é"}"#.as_bytes());
        let mut framer = Framer::default();
        for (i, b) in bytes.iter().enumerate() {
            assert!(framer.next().is_none(), "complete after {i} bytes");
            framer.push(&[*b]);
        }
        assert_eq!(bodies(&mut framer), vec![r#"{"a":"é"}"#]);
        assert!(framer.next().is_none());
    }

    #[test]
    fn several_messages_in_one_chunk() {
        let mut bytes = encode_body(b"1");
        bytes.extend(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: 2\r\n\r\n22");
        bytes.extend(encode_body(b"333"));
        bytes.extend(b"Content-Length: 4\r\n\r\n44"); // half of a fourth
        let mut framer = Framer::default();
        framer.push(&bytes);
        assert_eq!(bodies(&mut framer), vec!["1", "22", "333"]);
        framer.push(b"44");
        assert_eq!(bodies(&mut framer), vec!["4444"]);
    }

    #[test]
    fn header_errors() {
        let err = |bytes: &[u8]| {
            let mut framer = Framer::default();
            framer.push(bytes);
            framer.next().unwrap().unwrap_err()
        };
        assert_eq!(err(b"Content-Type: x\r\n\r\n{}"), FrameError::MissingLength);
        assert_eq!(err(b"Content-Length: ten\r\n\r\n"), FrameError::BadLength("ten".into()));
        assert_eq!(err(b"Content-Length: -1\r\n\r\n"), FrameError::BadLength("-1".into()));
        assert_eq!(err(b"garbage\r\n\r\n"), FrameError::BadHeader("garbage".into()));
        assert_eq!(
            err(b"Content-Length: 999999999999\r\n\r\n"),
            FrameError::TooBig(999999999999)
        );
        assert_eq!(err(&[b'x'; MAX_HEADER + 1]), FrameError::HeaderTooLong);
    }

    #[test]
    fn messages_round_trip() {
        let all = [
            Message::request(7, "initialize", json!({ "x": 1 })),
            Message::Request {
                id: Id::Str("s1".into()),
                method: "workspace/configuration".into(),
                params: json!({ "items": [] }),
            },
            Message::notification("exit", Value::Null),
            Message::notification("initialized", json!({})),
            Message::Response {
                id: Id::Num(7),
                result: Ok(json!({ "capabilities": {} })),
            },
            Message::Response {
                id: Id::Str("s1".into()),
                result: Err(RpcError {
                    code: METHOD_NOT_FOUND,
                    message: "no".into(),
                }),
            },
        ];
        for m in all {
            let mut framer = Framer::default();
            framer.push(&m.encode());
            let body = framer.next().unwrap().unwrap();
            let v: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["jsonrpc"], "2.0");
            assert_eq!(Message::from_value(v), Some(m));
        }
        assert_eq!(Message::from_value(json!({ "jsonrpc": "2.0" })), None);
        assert_eq!(Message::from_value(json!([1])), None);
    }
}
