//! jmux: a terminal multiplexer with detachable sessions (design in `jmux/PLAN.md`).

// Parts of these are for the client and CLI (jmux_server-c): `FrameReader`, `read_msg_blocking`,
// `session::list` & co., `server::spawn_server`.
#[allow(dead_code)]
mod protocol;
#[allow(dead_code)]
mod session;

mod app;
mod logging;
#[allow(dead_code)]
mod server;
mod theme;
mod ui;

#[cfg(test)]
mod jmux_server_tests;

use std::path::PathBuf;

/// The application name passed to jtui (directory names, sockets).
pub const APP: &str = "jmux";

/// The hidden `server` subcommand's arguments.
#[derive(Debug, PartialEq)]
struct ServerArgs {
    session: String,
    socket_dir: Option<PathBuf>,
}

/// Parses what follows `server`: `--session <name> [--socket-dir <dir>]`.
fn parse_server_args(args: impl IntoIterator<Item = String>) -> Result<ServerArgs, String> {
    let mut args = args.into_iter();
    let (mut session, mut socket_dir) = (None, None);
    while let Some(arg) = args.next() {
        let value = |args: &mut dyn Iterator<Item = String>| args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--session" => session = Some(value(&mut args)?),
            "--socket-dir" => socket_dir = Some(PathBuf::from(value(&mut args)?)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let session = session.ok_or("--session is required")?;
    if !session::valid_name(&session) {
        return Err(format!("invalid session name {session}"));
    }
    Ok(ServerArgs { session, socket_dir })
}

/// The server process: logs to its own file, runs until the session ends. Never prints.
fn run_server(args: ServerArgs) -> i32 {
    let log = logging::log_path(&logging::log_dir(), &args.session, std::process::id());
    let _ = logging::init(&log);
    let socket_dir = args.socket_dir.unwrap_or_else(|| jtui::socket::default_socket_dir(APP));
    let program = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map_or_else(|| PathBuf::from("/bin/sh"), PathBuf::from);
    let config = app::Config {
        name: args.session,
        socket: PathBuf::new(),
        program,
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "no runtime");
            return 1;
        }
    };
    let result = runtime.block_on(server::run(&socket_dir, config));
    // A reader thread blocked on a PTY must not hold the exit.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    match result {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!(error = %e, "server exited with an error");
            1
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("server") {
        match parse_server_args(args) {
            Ok(args) => std::process::exit(run_server(args)),
            Err(e) => {
                eprintln!("jmux server: {e}");
                std::process::exit(2);
            }
        }
    }
    eprintln!("jmux: not yet implemented");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<ServerArgs, String> {
        parse_server_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn server_args() {
        assert_eq!(
            parse(&["--session", "work", "--socket-dir", "/x"]).unwrap(),
            ServerArgs {
                session: "work".into(),
                socket_dir: Some("/x".into())
            }
        );
        assert_eq!(parse(&["--session", "a"]).unwrap().socket_dir, None);
        assert!(parse(&[]).is_err());
        assert!(parse(&["--session"]).is_err());
        assert!(parse(&["--session", "a/b"]).is_err());
        assert!(parse(&["--session", "a", "--bogus"]).is_err());
    }
}
