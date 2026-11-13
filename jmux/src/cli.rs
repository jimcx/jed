//! The command line: parsing, and the session commands as functions of a socket directory (so
//! tests pass a temp one and the `$JMUX` value in). Everything here is blocking `std`; the one
//! async part is the attached client (`client`).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::protocol::{ClientMsg, ServerMsg, encode_client, read_msg_blocking};
use crate::{server, session};

pub const USAGE: &str = "usage: jmux [command]

  (none)                        attach to the most recently used session, or start one
  new [-s name] [-c dir]        start a session (shell in dir) and attach to it
  attach [-t name]              attach to a session (the most recently used by default)
  ls                            list the sessions
  kill-session -t name          end a session
  rename-session [-t name] new  rename a session (this one inside jmux)
  install-shell-integration     add jmux's shell integration (command history) to your shell's init file

Detach with Ctrl+Shift+D. Attaching takes the session over from another client.";

/// How long a management request waits for the server's answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Default,
    New { name: Option<String>, dir: Option<PathBuf> },
    Attach { target: Option<String> },
    Ls,
    Kill { target: String },
    Rename { target: Option<String>, new: String },
    InstallShellIntegration,
    Help,
}

/// Parses the arguments after the program name (not the hidden `server`).
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cmd, String> {
    let mut args = args.into_iter();
    let Some(cmd) = args.next() else {
        return Ok(Cmd::Default);
    };
    // The flags of a command: `-x value` pairs (and the positional ones it allows).
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut positional = Vec::new();
    let allowed: &[&str] = match cmd.as_str() {
        "new" => &["-s", "-c"],
        "attach" | "kill-session" | "rename-session" => &["-t"],
        "ls" | "install-shell-integration" => &[],
        "-h" | "--help" | "help" => return Ok(Cmd::Help),
        other => return Err(format!("unknown command {other}")),
    };
    while let Some(arg) = args.next() {
        if arg == "-h" || arg == "--help" {
            return Ok(Cmd::Help);
        }
        if allowed.contains(&arg.as_str()) {
            let value = args.next().ok_or(format!("{arg} needs a value"))?;
            flags.push((arg, value));
        } else if arg.starts_with('-') || cmd != "rename-session" {
            return Err(format!("unknown argument {arg}"));
        } else {
            positional.push(arg);
        }
    }
    let flag = |name: &str| flags.iter().rev().find(|(f, _)| f == name).map(|(_, v)| v.clone());
    Ok(match cmd.as_str() {
        "new" => Cmd::New {
            name: flag("-s"),
            dir: flag("-c").map(PathBuf::from),
        },
        "attach" => Cmd::Attach { target: flag("-t") },
        "ls" => Cmd::Ls,
        "install-shell-integration" => Cmd::InstallShellIntegration,
        "kill-session" => Cmd::Kill {
            target: flag("-t").ok_or("kill-session needs -t <name>")?,
        },
        _ => match positional.as_slice() {
            [new] => Cmd::Rename {
                target: flag("-t"),
                new: new.clone(),
            },
            _ => return Err("rename-session needs the new name".into()),
        },
    })
}

/// The session a `$JMUX` value (the socket path) names, when set and non-empty.
pub fn current_session(jmux_env: Option<&str>) -> Option<String> {
    let path = Path::new(jmux_env.filter(|s| !s.is_empty())?);
    Some(path.file_stem()?.to_string_lossy().into_owned())
}

/// The refusal for starting or attaching inside a jmux terminal.
pub fn nested(jmux_env: Option<&str>) -> Option<String> {
    current_session(jmux_env).map(|s| format!("jmux: already inside session {s} (unset JMUX to nest anyway)"))
}

/// `jmux install-shell-integration`: installs for the shell `$SHELL` names (`shell`, else `sh`)
/// and returns the line to print.
///
/// Blocking: reads and writes files.
pub fn install_shell_integration(dirs: &crate::integration::Dirs, shell: Option<&str>) -> Result<String, String> {
    let program = shell.filter(|s| !s.is_empty()).unwrap_or("/bin/sh");
    let base = jterm::shell::base(Path::new(program));
    crate::integration::install(dirs, &base).map(|done| done.message(&dirs.home))
}

/// Sends one management request to the server on `socket` and returns its answer.
///
/// Blocking: call from spawn_blocking
pub fn request(socket: &Path, msg: &ClientMsg) -> io::Result<ServerMsg> {
    use std::io::Write;
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    stream.write_all(&encode_client(msg))?;
    match read_msg_blocking::<ServerMsg, _>(&mut stream) {
        Ok(Some(answer)) => Ok(answer),
        Ok(None) => Err(io::Error::other("the server closed the connection")),
        Err(e) => Err(io::Error::other(e.to_string())),
    }
}

/// `unix seconds` as the local "Nov 13 14:02".
fn format_time(secs: u64) -> String {
    let t = secs as libc::time_t;
    // SAFETY: `tm` is plain data, filled by localtime_r; strftime writes at most `buf.len()` bytes.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return secs.to_string();
        }
        let mut buf = [0u8; 32];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c"%b %e %H:%M".as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).replace("  ", " ")
    }
}

/// One `ls` line of a session.
pub fn describe(name: &str, info: &ServerMsg) -> String {
    match info {
        ServerMsg::Info {
            terminals,
            running,
            created,
            attached,
        } => format!(
            "{name}: {terminals} terminal{}, {running} running, created {}, {}",
            if *terminals == 1 { "" } else { "s" },
            format_time(*created),
            if *attached { "attached" } else { "detached" }
        ),
        _ => format!("{name}: no answer"),
    }
}

/// `jmux ls`: removes stale sockets, then one line per live session ("no sessions" when none).
///
/// Blocking: call from spawn_blocking
pub fn ls(dir: &Path) -> Vec<String> {
    session::remove_stale(dir);
    let lines: Vec<String> = session::list(dir)
        .into_iter()
        .filter(|e| e.live)
        .map(|e| match request(&e.path, &ClientMsg::Query) {
            Ok(info) => describe(&e.name, &info),
            Err(e2) => format!("{}: {e2}", e.name),
        })
        .collect();
    if lines.is_empty() {
        vec!["no sessions".into()]
    } else {
        lines
    }
}

/// The live session to attach to: `target`, else the most recently used. `Err` is the message.
///
/// Blocking: call from spawn_blocking
pub fn find(dir: &Path, target: Option<&str>) -> Result<(String, PathBuf), String> {
    session::remove_stale(dir);
    match target {
        Some(name) => session::list(dir)
            .into_iter()
            .find(|e| e.live && e.name == name)
            .map(|e| (e.name, e.path))
            .ok_or(format!("jmux: no session {name}")),
        None => session::most_recent(dir)
            .map(|e| (e.name, e.path))
            .ok_or("jmux: no sessions".into()),
    }
}

/// Starts a session: `name` (checked, and not live) or the next free number, its shell in `cwd`
/// (else the current directory). Returns the name and socket to attach to.
///
/// Blocking: call from spawn_blocking
pub fn start(exe: &Path, dir: &Path, name: Option<&str>, cwd: Option<&Path>) -> Result<(String, PathBuf), String> {
    if let Some(n) = name
        && !session::valid_name(n)
    {
        return Err(format!("jmux: invalid session name {n}"));
    }
    jtui::socket::prepare_dir(dir).map_err(|e| format!("jmux: cannot use {}: {e}", dir.display()))?;
    session::remove_stale(dir);
    let name = name.map_or_else(|| session::next_free_name(dir), str::to_string);
    let socket = session::socket_path(dir, &name);
    if session::list(dir).iter().any(|e| e.live && e.name == name) {
        return Err(format!("jmux: session {name} already exists"));
    }
    if let Some(cwd) = cwd
        && !cwd.is_dir()
    {
        return Err(format!("jmux: {} is not a directory", cwd.display()));
    }
    server::spawn_server(exe, dir, &name, cwd).map_err(|e| format!("jmux: cannot start session {name}: {e}"))?;
    Ok((name, socket))
}

/// `jmux kill-session -t name`.
///
/// Blocking: call from spawn_blocking
pub fn kill(dir: &Path, target: &str) -> Result<String, String> {
    let (name, path) = find(dir, Some(target))?;
    match request(&path, &ClientMsg::Kill) {
        Ok(ServerMsg::Done) => Ok(format!("[session {name} killed]")),
        Ok(other) => Err(format!("jmux: unexpected answer {other:?}")),
        Err(e) => Err(format!("jmux: cannot kill {name}: {e}")),
    }
}

/// `jmux rename-session [-t name] new`: without `-t`, the current session inside jmux (`jmux_env`
/// is `$JMUX`), else the most recently used.
///
/// Blocking: call from spawn_blocking
pub fn rename(dir: &Path, target: Option<&str>, new: &str, jmux_env: Option<&str>) -> Result<String, String> {
    let current = current_session(jmux_env);
    let (name, path) = find(dir, target.or(current.as_deref()))?;
    match request(&path, &ClientMsg::Rename { name: new.into() }) {
        Ok(ServerMsg::Done) => Ok(format!("[session {name} renamed to {new}]")),
        Ok(ServerMsg::Error { message }) => Err(format!("jmux: {message}")),
        Ok(other) => Err(format!("jmux: unexpected answer {other:?}")),
        Err(e) => Err(format!("jmux: cannot rename {name}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cmd, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(p(&[]), Ok(Cmd::Default));
        assert_eq!(
            p(&["new", "-s", "work", "-c", "/tmp"]),
            Ok(Cmd::New {
                name: Some("work".into()),
                dir: Some("/tmp".into())
            })
        );
        assert_eq!(p(&["new"]), Ok(Cmd::New { name: None, dir: None }));
        assert_eq!(p(&["attach"]), Ok(Cmd::Attach { target: None }));
        assert_eq!(
            p(&["attach", "-t", "w"]),
            Ok(Cmd::Attach {
                target: Some("w".into())
            })
        );
        assert_eq!(p(&["ls"]), Ok(Cmd::Ls));
        assert_eq!(p(&["kill-session", "-t", "w"]), Ok(Cmd::Kill { target: "w".into() }));
        assert_eq!(
            p(&["rename-session", "x"]),
            Ok(Cmd::Rename {
                target: None,
                new: "x".into()
            })
        );
        assert_eq!(
            p(&["rename-session", "-t", "w", "x"]),
            Ok(Cmd::Rename {
                target: Some("w".into()),
                new: "x".into()
            })
        );
        assert_eq!(p(&["-h"]), Ok(Cmd::Help));
        assert_eq!(p(&["ls", "--help"]), Ok(Cmd::Help));
    }

    #[test]
    fn rejects_bad_command_lines() {
        assert!(p(&["bogus"]).is_err());
        assert!(p(&["new", "-s"]).is_err());
        assert!(p(&["new", "-x"]).is_err());
        assert!(p(&["new", "extra"]).is_err());
        assert!(p(&["ls", "-t", "w"]).is_err());
        assert!(p(&["kill-session"]).is_err());
        assert!(p(&["rename-session"]).is_err());
        assert!(p(&["rename-session", "a", "b"]).is_err());
    }

    #[test]
    fn install_shell_integration_parses_and_takes_no_arguments() {
        assert_eq!(
            parse(["install-shell-integration".to_string()]),
            Ok(Cmd::InstallShellIntegration)
        );
        assert!(parse(["install-shell-integration".to_string(), "-t".into(), "x".into()]).is_err());
    }

    #[test]
    fn install_shell_integration_uses_the_given_dirs_and_shell() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::integration::Dirs {
            home: tmp.path().join("home"),
            config_home: tmp.path().join("home/.config"),
            state: tmp.path().join("state"),
        };
        let line = install_shell_integration(&dirs, Some("/usr/bin/zsh")).unwrap();
        assert!(line.starts_with("Added to ~/.zshrc"), "{line}");
        assert!(tmp.path().join("home/.zshrc").exists() && tmp.path().join("state/shell-integration.zsh").exists());
        let e = install_shell_integration(&dirs, None).unwrap_err();
        assert!(e.contains("'sh'"), "{e}");
    }

    #[test]
    fn nesting_is_read_from_jmux() {
        assert_eq!(
            current_session(Some("/run/user/1/jmux/work.sock")).as_deref(),
            Some("work")
        );
        assert_eq!(current_session(Some("")), None);
        assert_eq!(current_session(None), None);
        assert!(
            nested(Some("/x/work.sock"))
                .unwrap()
                .contains("already inside session work")
        );
        assert_eq!(nested(None), None);
    }

    #[test]
    fn describes_a_session_on_one_line() {
        let line = describe(
            "work",
            &ServerMsg::Info {
                terminals: 1,
                running: 0,
                created: 1_700_000_000,
                attached: true,
            },
        );
        assert!(line.starts_with("work: 1 terminal, 0 running, created "), "{line}");
        assert!(line.ends_with(", attached") && !line.contains('\n'), "{line}");
    }

    #[test]
    fn no_sessions_in_an_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(ls(dir.path()), ["no sessions"]);
        assert_eq!(find(dir.path(), None).unwrap_err(), "jmux: no sessions");
        assert_eq!(find(dir.path(), Some("x")).unwrap_err(), "jmux: no session x");
    }

    #[test]
    fn start_refuses_bad_names_and_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = &tmp.path().join("run");
        let exe = Path::new("/nonexistent/jmux");
        assert!(
            start(exe, dir, Some("a/b"), None)
                .unwrap_err()
                .contains("invalid session name")
        );
        assert!(
            start(exe, dir, Some("a"), Some(Path::new("/nonexistent")))
                .unwrap_err()
                .contains("not a directory")
        );
    }
}
