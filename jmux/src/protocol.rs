//! The wire protocol between a jmux client and its session's server.
//!
//! A frame is a `u32` big-endian length of what follows, one kind byte, then the payload:
//! kind 0 is a JSON message, kind 1 is raw bytes (the server's [`ServerMsg::Output`], which is the
//! bulk of the traffic and needs no escaping). A length of 0 (no kind byte) or over
//! [`MAX_FRAME`] is an error, never an allocation.
//!
//! Client to server: [`ClientMsg`]; server to client: [`ServerMsg`]. Encode with
//! [`encode_client`] / [`encode_server`]; decode incrementally with a [`FrameReader`] (bytes in as
//! they arrive, whole messages out), or whole frames from a stream with the async
//! [`read_msg`] / [`write_all_msg`] or the blocking [`read_msg_blocking`].

use std::io::{self, Read};

use crossterm::event::Event;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Bumped when a message changes shape incompatibly: version 1 has `Hello::touch` optional (an
/// old server ignores it and draws the normal layout; an old client's `Hello` lacks it: false).
/// A server answers a different one with `Bye(VersionMismatch)`.
pub const PROTOCOL_VERSION: u32 = 1;

/// The largest frame body (kind byte + payload) accepted.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

const KIND_JSON: u8 = 0;
const KIND_RAW: u8 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message: the client's protocol version, screen size and terminal.
    Hello {
        version: u32,
        rows: u16,
        cols: u16,
        truecolor: bool,
        term: String,
        /// `--touch`: draw the touch layout for this client (absent from an old client: false).
        #[serde(default)]
        touch: bool,
    },
    /// An input event read from the client's terminal.
    Input(Event),
    Resize {
        rows: u16,
        cols: u16,
    },
    Detach,
    /// Management requests, accepted as a connection's first message instead of `Hello` (they
    /// never attach or take over): the server answers and closes the connection.
    /// `jmux ls`: answered with [`ServerMsg::Info`].
    Query,
    /// `jmux kill-session`: the session ends; answered with [`ServerMsg::Done`].
    Kill,
    /// `jmux rename-session`: answered with [`ServerMsg::Done`] or [`ServerMsg::Error`].
    Rename {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        session: String,
    },
    /// Bytes for the client's terminal (a raw frame, kind 1).
    Output(Vec<u8>),
    Bye {
        reason: ByeReason,
    },
    /// The answer to [`ClientMsg::Query`].
    Info {
        terminals: u32,
        /// Terminals running a command (not idle at their prompt).
        running: u32,
        /// Unix seconds the session started.
        created: u64,
        attached: bool,
    },
    /// The request (kill, rename) succeeded.
    Done,
    /// The request failed; the text is for the user.
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ByeReason {
    Detached,
    SessionEnded,
    TakenOver,
    VersionMismatch {
        server: u32,
    },
    /// The server cannot continue (its message is for the log; the client says the server failed).
    ServerError(String),
}

impl ByeReason {
    /// The one line the client prints after restoring its terminal.
    pub fn message(&self, session: &str) -> String {
        match self {
            ByeReason::Detached => format!("[detached from {session}]"),
            ByeReason::SessionEnded => format!("[session {session} ended]"),
            ByeReason::TakenOver => format!("[{session} taken over by another client]"),
            ByeReason::VersionMismatch { server } => format!(
                "[server of {session} speaks protocol {server}, this jmux {PROTOCOL_VERSION}: use the matching jmux]"
            ),
            ByeReason::ServerError(e) => format!("[server of {session} failed: {e}]"),
        }
    }
}

/// What can go wrong decoding a frame.
#[derive(Debug)]
pub enum ProtocolError {
    /// The length is 0 or over [`MAX_FRAME`].
    BadLength(usize),
    UnknownKind(u8),
    /// A raw frame where the message type has none (client messages are all JSON).
    UnexpectedKind(u8),
    Json(serde_json::Error),
    Io(io::Error),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::BadLength(n) => write!(f, "bad frame length {n}"),
            ProtocolError::UnknownKind(k) => write!(f, "unknown frame kind {k}"),
            ProtocolError::UnexpectedKind(k) => write!(f, "unexpected frame kind {k}"),
            ProtocolError::Json(e) => write!(f, "bad message: {e}"),
            ProtocolError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(e: io::Error) -> Self {
        ProtocolError::Io(e)
    }
}

/// A message type that travels in frames.
pub trait Message: Sized {
    /// The frame kind and payload.
    fn to_frame(&self) -> (u8, Vec<u8>);
    fn from_frame(kind: u8, payload: Vec<u8>) -> Result<Self, ProtocolError>;
}

fn json<T: Serialize>(msg: &T) -> (u8, Vec<u8>) {
    // Serializing these types cannot fail (no maps with non-string keys, no custom errors).
    (KIND_JSON, serde_json::to_vec(msg).unwrap_or_default())
}

fn from_json<T: for<'a> Deserialize<'a>>(kind: u8, payload: &[u8]) -> Result<T, ProtocolError> {
    match kind {
        KIND_JSON => serde_json::from_slice(payload).map_err(ProtocolError::Json),
        KIND_RAW => Err(ProtocolError::UnexpectedKind(KIND_RAW)),
        k => Err(ProtocolError::UnknownKind(k)),
    }
}

impl Message for ClientMsg {
    fn to_frame(&self) -> (u8, Vec<u8>) {
        json(self)
    }
    fn from_frame(kind: u8, payload: Vec<u8>) -> Result<Self, ProtocolError> {
        from_json(kind, &payload)
    }
}

impl Message for ServerMsg {
    fn to_frame(&self) -> (u8, Vec<u8>) {
        match self {
            ServerMsg::Output(bytes) => (KIND_RAW, bytes.clone()),
            other => json(other),
        }
    }
    fn from_frame(kind: u8, payload: Vec<u8>) -> Result<Self, ProtocolError> {
        match kind {
            KIND_RAW => Ok(ServerMsg::Output(payload)),
            k => from_json(k, &payload),
        }
    }
}

fn encode<M: Message>(msg: &M) -> Vec<u8> {
    let (kind, payload) = msg.to_frame();
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(&payload);
    out
}

pub fn encode_client(msg: &ClientMsg) -> Vec<u8> {
    encode(msg)
}

#[cfg(test)]
pub fn encode_server(msg: &ServerMsg) -> Vec<u8> {
    encode(msg)
}

/// Check a frame's length field and return the body size.
fn body_len(header: [u8; 4]) -> Result<usize, ProtocolError> {
    let n = u32::from_be_bytes(header) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(ProtocolError::BadLength(n));
    }
    Ok(n)
}

/// Incremental decoder: `push` bytes as they arrive, `next` yields whole messages. After an error
/// the stream is out of step and the connection should be dropped.
pub struct FrameReader<M> {
    buf: Vec<u8>,
    _m: std::marker::PhantomData<M>,
}

impl<M: Message> Default for FrameReader<M> {
    fn default() -> Self {
        FrameReader {
            buf: Vec::new(),
            _m: std::marker::PhantomData,
        }
    }
}

impl<M: Message> FrameReader<M> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, `None` when more bytes are needed.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<M>, ProtocolError> {
        let Some(header) = self.buf.first_chunk::<4>() else {
            return Ok(None);
        };
        let n = body_len(*header)?;
        if self.buf.len() < 4 + n {
            return Ok(None);
        }
        let kind = self.buf[4];
        let payload = self.buf[5..4 + n].to_vec();
        self.buf.drain(..4 + n);
        M::from_frame(kind, payload).map(Some)
    }
}

/// Write one message (a single `write_all`, then flush).
pub async fn write_all_msg<M: Message, W: AsyncWrite + Unpin>(w: &mut W, msg: &M) -> io::Result<()> {
    w.write_all(&encode(msg)).await?;
    w.flush().await
}

/// Read one message; `Ok(None)` at a clean end of stream between frames (an end inside a frame,
/// even inside the length, is an `UnexpectedEof` error).
pub async fn read_msg<M: Message, R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<M>, ProtocolError> {
    let mut header = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut header[got..]).await? {
            0 if got == 0 => return Ok(None),
            0 => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
            n => got += n,
        }
    }
    let n = body_len(header)?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await?;
    let kind = body[0];
    body.remove(0);
    M::from_frame(kind, body).map(Some)
}

/// [`read_msg`] for a blocking reader.
///
/// Blocking: call from spawn_blocking
pub fn read_msg_blocking<M: Message, R: Read>(r: &mut R) -> Result<Option<M>, ProtocolError> {
    let mut header = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut header[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let n = body_len(header)?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    let kind = body[0];
    body.remove(0);
    M::from_frame(kind, body).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn events() -> Vec<Event> {
        let key = KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let mouse = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 7,
                row: 3,
                modifiers: KeyModifiers::ALT,
            })
        };
        vec![
            Event::Key(key),
            Event::Key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            mouse(MouseEventKind::Down(MouseButton::Left)),
            mouse(MouseEventKind::Down(MouseButton::Back)),
            mouse(MouseEventKind::Up(MouseButton::Forward)),
            mouse(MouseEventKind::ScrollDown),
            Event::Paste("héllo\nworld \u{1f600}".into()),
            Event::FocusGained,
            Event::FocusLost,
            Event::Resize(80, 24),
        ]
    }

    fn client_msgs() -> Vec<ClientMsg> {
        let mut v = vec![
            ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                rows: 24,
                cols: 80,
                truecolor: true,
                term: "xterm-256color".into(),
                touch: false,
            },
            ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                rows: 24,
                cols: 80,
                truecolor: true,
                term: "xterm-256color".into(),
                touch: true,
            },
            ClientMsg::Resize { rows: 50, cols: 200 },
            ClientMsg::Detach,
            ClientMsg::Query,
            ClientMsg::Kill,
            ClientMsg::Rename { name: "new".into() },
        ];
        v.extend(events().into_iter().map(ClientMsg::Input));
        v
    }

    fn server_msgs() -> Vec<ServerMsg> {
        vec![
            ServerMsg::Welcome { session: "work".into() },
            ServerMsg::Output(vec![0xff, 0x00, 0x1b, b'[', 0xc3, 0x28, b'm']),
            ServerMsg::Output(Vec::new()),
            ServerMsg::Bye {
                reason: ByeReason::Detached,
            },
            ServerMsg::Bye {
                reason: ByeReason::SessionEnded,
            },
            ServerMsg::Bye {
                reason: ByeReason::TakenOver,
            },
            ServerMsg::Bye {
                reason: ByeReason::VersionMismatch { server: 9 },
            },
            ServerMsg::Bye {
                reason: ByeReason::ServerError("boom".into()),
            },
            ServerMsg::Info {
                terminals: 1,
                running: 0,
                created: 1_700_000_000,
                attached: true,
            },
            ServerMsg::Done,
            ServerMsg::Error {
                message: "taken".into(),
            },
        ]
    }

    #[test]
    fn a_hello_without_touch_is_read_as_not_touch() {
        // An old client's `Hello` (protocol version 1 before `touch`).
        let body = br#"{"Hello":{"version":1,"rows":24,"cols":80,"truecolor":true,"term":"xterm"}}"#;
        let msg: ClientMsg = serde_json::from_slice(body).unwrap();
        assert_eq!(
            msg,
            ClientMsg::Hello {
                version: 1,
                rows: 24,
                cols: 80,
                truecolor: true,
                term: "xterm".into(),
                touch: false,
            }
        );
        // And the new `Hello` carries the field (an old server ignores it: no `deny_unknown_fields`).
        let new = serde_json::to_string(&client_msgs()[1]).unwrap();
        assert!(new.contains("\"touch\":true"), "{new}");
    }

    #[test]
    fn round_trips_every_message() {
        for m in client_msgs() {
            let mut r = FrameReader::<ClientMsg>::new();
            r.push(&encode_client(&m));
            assert_eq!(r.next().unwrap(), Some(m));
            assert_eq!(r.next().unwrap(), None);
        }
        for m in server_msgs() {
            let mut r = FrameReader::<ServerMsg>::new();
            r.push(&encode_server(&m));
            assert_eq!(r.next().unwrap(), Some(m));
            assert_eq!(r.next().unwrap(), None);
        }
    }

    #[test]
    fn output_is_a_raw_frame() {
        let bytes = encode_server(&ServerMsg::Output(vec![1, 2, 3]));
        assert_eq!(bytes, vec![0, 0, 0, 4, KIND_RAW, 1, 2, 3]);
        assert_eq!(encode_client(&ClientMsg::Detach)[4], KIND_JSON);
    }

    #[test]
    fn frame_split_byte_by_byte() {
        let msgs = client_msgs();
        let bytes: Vec<u8> = msgs.iter().flat_map(encode_client).collect();
        let mut r = FrameReader::<ClientMsg>::new();
        let mut got = Vec::new();
        for b in bytes {
            r.push(&[b]);
            while let Some(m) = r.next().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, msgs);
    }

    #[test]
    fn two_frames_in_one_push() {
        let mut bytes = encode_server(&ServerMsg::Welcome { session: "a".into() });
        bytes.extend(encode_server(&ServerMsg::Output(b"hi".to_vec())));
        let mut r = FrameReader::<ServerMsg>::new();
        r.push(&bytes);
        assert_eq!(r.next().unwrap(), Some(ServerMsg::Welcome { session: "a".into() }));
        assert_eq!(r.next().unwrap(), Some(ServerMsg::Output(b"hi".to_vec())));
        assert_eq!(r.next().unwrap(), None);
    }

    #[test]
    fn bad_frames_are_errors() {
        let mut r = FrameReader::<ServerMsg>::new();
        r.push(&((MAX_FRAME as u32 + 1).to_be_bytes()));
        assert!(matches!(r.next(), Err(ProtocolError::BadLength(_))));

        let mut r = FrameReader::<ServerMsg>::new();
        r.push(&[0, 0, 0, 0]);
        assert!(matches!(r.next(), Err(ProtocolError::BadLength(0))));

        let mut r = FrameReader::<ServerMsg>::new();
        r.push(&[0, 0, 0, 2, 7, 0]);
        assert!(matches!(r.next(), Err(ProtocolError::UnknownKind(7))));

        let mut r = FrameReader::<ClientMsg>::new();
        r.push(&[0, 0, 0, 2, KIND_RAW, 0]);
        assert!(matches!(r.next(), Err(ProtocolError::UnexpectedKind(1))));

        let mut r = FrameReader::<ClientMsg>::new();
        r.push(&[0, 0, 0, 4, KIND_JSON, b'{', b'x', b'}']);
        assert!(matches!(r.next(), Err(ProtocolError::Json(_))));
    }

    #[test]
    fn oversized_length_does_not_wait_for_the_body() {
        let mut r = FrameReader::<ClientMsg>::new();
        r.push(&u32::MAX.to_be_bytes());
        assert!(r.next().is_err());
    }

    #[test]
    fn bye_messages() {
        assert_eq!(ByeReason::Detached.message("work"), "[detached from work]");
        assert_eq!(ByeReason::SessionEnded.message("work"), "[session work ended]");
        assert!(ByeReason::TakenOver.message("work").contains("taken over"));
        assert!(
            ByeReason::VersionMismatch { server: 2 }
                .message("w")
                .contains("protocol 2")
        );
    }

    #[tokio::test]
    async fn over_a_unix_socket() {
        let (mut a, mut b) = tokio::net::UnixStream::pair().unwrap();
        let sent = client_msgs();
        for m in &sent {
            write_all_msg(&mut a, m).await.unwrap();
        }
        for m in &sent {
            assert_eq!(read_msg::<ClientMsg, _>(&mut b).await.unwrap().as_ref(), Some(m));
        }
        write_all_msg(&mut b, &ServerMsg::Output(vec![0xfe, 0xff]))
            .await
            .unwrap();
        assert_eq!(
            read_msg::<ServerMsg, _>(&mut a).await.unwrap(),
            Some(ServerMsg::Output(vec![0xfe, 0xff]))
        );
        drop(a);
        assert!(read_msg::<ClientMsg, _>(&mut b).await.unwrap().is_none());
    }

    #[test]
    fn blocking_reader() {
        let mut bytes = encode_client(&ClientMsg::Detach);
        bytes.extend(encode_client(&ClientMsg::Resize { rows: 1, cols: 2 }));
        let mut cur = io::Cursor::new(bytes);
        assert_eq!(
            read_msg_blocking::<ClientMsg, _>(&mut cur).unwrap(),
            Some(ClientMsg::Detach)
        );
        assert_eq!(
            read_msg_blocking::<ClientMsg, _>(&mut cur).unwrap(),
            Some(ClientMsg::Resize { rows: 1, cols: 2 })
        );
        assert_eq!(read_msg_blocking::<ClientMsg, _>(&mut cur).unwrap(), None);
        let mut bad = io::Cursor::new(vec![0xff, 0xff, 0xff, 0xff]);
        assert!(read_msg_blocking::<ClientMsg, _>(&mut bad).is_err());
    }

    fn is_eof(e: &ProtocolError) -> bool {
        matches!(e, ProtocolError::Io(e) if e.kind() == io::ErrorKind::UnexpectedEof)
    }

    #[test]
    fn eof_inside_a_frame_is_an_error_not_a_clean_end() {
        let full = encode_client(&ClientMsg::Resize { rows: 1, cols: 2 });
        // Every proper prefix but the empty one ends mid-frame (partial length or partial body).
        for cut in 1..full.len() {
            let mut cur = io::Cursor::new(full[..cut].to_vec());
            let e = read_msg_blocking::<ClientMsg, _>(&mut cur).unwrap_err();
            assert!(is_eof(&e), "cut {cut}: {e}");
        }
        let mut cur = io::Cursor::new(Vec::new());
        assert!(read_msg_blocking::<ClientMsg, _>(&mut cur).unwrap().is_none());
    }

    #[tokio::test]
    async fn async_eof_inside_a_frame_is_an_error() {
        let full = encode_client(&ClientMsg::Detach);
        for cut in 1..full.len() {
            let mut slice = &full[..cut];
            let e = read_msg::<ClientMsg, _>(&mut slice).await.unwrap_err();
            assert!(is_eof(&e), "cut {cut}: {e}");
        }
        let mut empty: &[u8] = &[];
        assert!(read_msg::<ClientMsg, _>(&mut empty).await.unwrap().is_none());
        let mut bad: &[u8] = &[0, 0, 0, 0];
        assert!(matches!(
            read_msg::<ClientMsg, _>(&mut bad).await,
            Err(ProtocolError::BadLength(0))
        ));
    }

    #[test]
    fn partial_frame_waits_then_completes() {
        let full = encode_server(&ServerMsg::Welcome { session: "w".into() });
        let mut r = FrameReader::<ServerMsg>::new();
        r.push(&full[..full.len() - 1]);
        assert_eq!(r.next().unwrap(), None);
        r.push(&full[full.len() - 1..]);
        assert_eq!(r.next().unwrap(), Some(ServerMsg::Welcome { session: "w".into() }));
    }

    #[test]
    fn bye_messages_name_the_session_and_error() {
        assert!(ByeReason::TakenOver.message("work").contains("work"));
        assert!(ByeReason::ServerError("boom".into()).message("w").contains("boom"));
    }
}
