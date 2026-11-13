//! The attached client: what `jmux new` / `attach` run in the user's terminal. It holds no
//! session state: it sends the terminal's input events and size and writes the bytes the server
//! sends back.
//!
//! [`attach`] is the protocol side (a stream, a channel of events, a writer, a signal future), so
//! tests drive it without a terminal. [`run_attached`] wraps it with raw mode, the panic hook,
//! the signals and the one line printed after the terminal is restored.

use std::io::{self, Write};
use std::path::Path;

use crossterm::event::{Event, EventStream};
use futures_util::{Stream, StreamExt};
use jtui::terminal::{self, CursorShape, CursorStyle, Identity};
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;

use crate::protocol::{ByeReason, ClientMsg, FrameReader, PROTOCOL_VERSION, ServerMsg, write_all_msg};

/// What the client tells the server about its terminal.
#[derive(Debug, Clone)]
pub struct Terminal {
    pub rows: u16,
    pub cols: u16,
    pub truecolor: bool,
    pub term: String,
}

/// Why [`attach`] returned.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The server said goodbye.
    Bye(ByeReason),
    /// The connection ended without a `Bye`: the server died.
    ServerGone,
    /// SIGHUP / SIGTERM, or the terminal's input ended. The server sees the connection close,
    /// which detaches.
    Hangup,
}

impl Outcome {
    /// The one line printed after the terminal is restored.
    pub fn message(&self, session: &str) -> String {
        match self {
            Outcome::Bye(reason) => reason.message(session),
            Outcome::ServerGone => format!("[server of {session} went away]"),
            Outcome::Hangup => format!("[detached from {session}]"),
        }
    }

    /// The process's exit code: 0 when the user left or the session ended, 1 for a failure.
    pub fn exit_code(&self) -> i32 {
        match self {
            Outcome::Bye(ByeReason::Detached | ByeReason::SessionEnded | ByeReason::TakenOver) | Outcome::Hangup => 0,
            Outcome::Bye(_) | Outcome::ServerGone => 1,
        }
    }
}

/// Sends `Hello` and then forwards `input` (resizes as `Resize`, the rest as `Input`) while
/// writing the server's output to `out` until the server says `Bye`, the connection ends, `input`
/// ends or `signal` completes.
pub async fn attach(
    stream: UnixStream,
    terminal: &Terminal,
    mut input: impl Stream<Item = io::Result<Event>> + Unpin,
    out: &mut impl Write,
    signal: impl Future<Output = ()>,
) -> io::Result<Outcome> {
    let (mut read, mut write) = stream.into_split();
    let hello = ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        rows: terminal.rows,
        cols: terminal.cols,
        truecolor: terminal.truecolor,
        term: terminal.term.clone(),
    };
    if write_all_msg(&mut write, &hello).await.is_err() {
        return Ok(Outcome::ServerGone);
    }
    let mut frames = FrameReader::<ServerMsg>::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut signal = std::pin::pin!(signal);
    loop {
        tokio::select! {
            // `read` is cancel-safe: a partial frame stays in `frames`.
            n = read.read(&mut buf) => {
                match n {
                    Ok(0) | Err(_) => return Ok(Outcome::ServerGone),
                    Ok(n) => frames.push(&buf[..n]),
                }
                loop {
                    match frames.next() {
                        Ok(Some(ServerMsg::Output(bytes))) => {
                            out.write_all(&bytes)?;
                            out.flush()?;
                        }
                        Ok(Some(ServerMsg::Bye { reason })) => return Ok(Outcome::Bye(reason)),
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "bad frame from the server");
                            return Ok(Outcome::ServerGone);
                        }
                    }
                }
            }
            event = input.next() => {
                let msg = match event {
                    Some(Ok(Event::Resize(cols, rows))) => ClientMsg::Resize { rows, cols },
                    Some(Ok(event)) => ClientMsg::Input(event),
                    Some(Err(_)) | None => return Ok(Outcome::Hangup),
                };
                if write_all_msg(&mut write, &msg).await.is_err() {
                    return Ok(Outcome::ServerGone);
                }
            }
            () = &mut signal => return Ok(Outcome::Hangup),
        }
    }
}

/// Resolves on SIGHUP or SIGTERM.
async fn hangup_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut hup), Ok(mut term)) = (signal(SignalKind::hangup()), signal(SignalKind::terminate())) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = hup.recv() => {}
        _ = term.recv() => {}
    }
}

/// Whether the terminal takes 24-bit colour (`COLORTERM`), as the server draws for it.
fn truecolor(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some("truecolor" | "24bit"))
}

/// Attaches to the session `name` listening on `socket` in this process's terminal: raw mode,
/// then every way out restores the terminal before the one line saying why is printed. Returns the
/// exit code. Needs no runtime (it starts its own).
pub fn run_attached(socket: &Path, name: &str) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("jmux: cannot start: {e}");
            return 1;
        }
    };
    let code = runtime.block_on(async {
        let stream = match UnixStream::connect(socket).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("jmux: cannot connect to session {name}: {e}");
                return 1;
            }
        };
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let info = Terminal {
            rows,
            cols,
            truecolor: truecolor(std::env::var("COLORTERM").ok().as_deref()),
            term: std::env::var("TERM").unwrap_or_default(),
        };
        install_panic_hook();
        let kitty = terminal::kitty_supported();
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let identity = Identity::new(crate::APP, Path::new(name), None, &home);
        let cursor = CursorShape {
            style: CursorStyle::Block,
            blinking: true,
        };
        let _term = match terminal::enter(kitty, cursor, &identity) {
            Ok(t) => t,
            Err(e) => {
                terminal::restore();
                eprintln!("jmux: cannot set up the terminal: {e}");
                return 1;
            }
        };
        let result = attach(stream, &info, EventStream::new(), &mut io::stdout(), hangup_signal()).await;
        finish(result, name, terminal::restore, &mut io::stdout(), &mut io::stderr())
    });
    // The event stream's reader thread must not hold the exit.
    runtime.shutdown_background();
    code
}

/// Every way out of an attached client: the terminal is restored first, then the one line saying
/// why (or the error) is written. Returns the exit code.
fn finish(
    result: io::Result<Outcome>,
    name: &str,
    restore: impl FnOnce(),
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    restore();
    match result {
        Ok(outcome) => {
            let _ = writeln!(out, "{}", outcome.message(name));
            outcome.exit_code()
        }
        Err(e) => {
            let _ = writeln!(err, "jmux: {e}");
            1
        }
    }
}

/// A panic puts the terminal back first, so its message is readable.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        terminal::restore();
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_exit_codes() {
        assert_eq!(Outcome::Bye(ByeReason::Detached).message("w"), "[detached from w]");
        assert_eq!(Outcome::ServerGone.message("w"), "[server of w went away]");
        assert_eq!(Outcome::Hangup.exit_code(), 0);
        assert_eq!(Outcome::ServerGone.exit_code(), 1);
        assert_eq!(Outcome::Bye(ByeReason::SessionEnded).exit_code(), 0);
        assert_eq!(Outcome::Bye(ByeReason::ServerError("x".into())).exit_code(), 1);
    }

    /// A writer that records whether the terminal had been restored when it was written to.
    struct Order<'a>(&'a std::cell::Cell<bool>, &'a std::cell::Cell<Option<bool>>);

    impl Write for Order<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.1.set(Some(self.0.get()));
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn finish_restores_before_printing_on_every_outcome() {
        let outcomes = [
            Ok(Outcome::Bye(ByeReason::Detached)),
            Ok(Outcome::ServerGone),
            Ok(Outcome::Hangup),
            Err(io::Error::other("broken pipe")),
        ];
        for result in outcomes {
            let failed = result.is_err();
            let restored = std::cell::Cell::new(false);
            let (out_saw, err_saw) = (std::cell::Cell::new(None), std::cell::Cell::new(None));
            let code = finish(
                result,
                "w",
                || restored.set(true),
                &mut Order(&restored, &out_saw),
                &mut Order(&restored, &err_saw),
            );
            assert!(restored.get());
            let saw = if failed { &err_saw } else { &out_saw };
            assert_eq!(saw.get(), Some(true), "the line was written before the restore");
            assert_eq!(code == 1, failed || code != 0);
        }
    }

    #[test]
    fn truecolor_from_colorterm() {
        assert!(truecolor(Some("truecolor")));
        assert!(truecolor(Some("24bit")));
        assert!(!truecolor(Some("yes")));
        assert!(!truecolor(None));
    }
}
