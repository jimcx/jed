//! jmux: a terminal multiplexer with detachable sessions (design in `jmux/PLAN.md`).

mod app;

mod cli;
mod client;
mod command;
mod group;
mod history;
mod hit;
mod integration;
mod keymap;
mod logging;
mod menu;
mod protocol;
mod server;
mod session;
mod settings;
mod side;
mod tab_drag;
mod tabbar;
mod theme;
mod ui;
mod ui_menu;
mod ui_side;

#[cfg(test)]
mod command_driven_tests;
#[cfg(test)]
mod jmux_commands_tests;
#[cfg(test)]
mod jmux_config_tests;
#[cfg(test)]
mod jmux_layout_tests;
#[cfg(test)]
mod jmux_server_tests;
#[cfg(test)]
mod jmux_tabs_at_bottom_tests;
#[cfg(test)]
mod jmux_touch_menu_tests;
#[cfg(test)]
mod jmux_touch_terminal_tests;
#[cfg(test)]
mod jmux_touch_tests;
#[cfg(test)]
mod kitty_keys_passthrough_tests;
#[cfg(test)]
mod test_harness;

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
    let settings_path = settings::path(&jtui::paths::config_dir(APP));
    let (settings, mut settings_errors) = settings::load(&settings_path);
    let keybindings_path = jtui::paths::config_dir(APP).join(keymap::FILE_NAME);
    let (keymap, keymap_errors) = keymap::load(&keybindings_path);
    settings_errors.extend(keymap_errors);
    let program = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map_or_else(|| PathBuf::from("/bin/sh"), PathBuf::from);
    let config = app::Config {
        name: args.session,
        socket: PathBuf::new(),
        program,
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        open_url: jtui::url::open,
        dirs: Some(integration::Dirs::real()),
        settings,
        settings_path: Some(settings_path),
        settings_errors,
        keybindings_path: Some(keybindings_path),
        keymap: Some(keymap),
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

/// Why `cmd` must not run inside a jmux terminal (`$JMUX` set): starting or attaching nests.
/// Without a terminal (`tty` false) they are refused too, before anything is started.
/// `ls`, `kill-session` and `rename-session` work inside and without.
fn refusal(cmd: &cli::Cmd, jmux_env: Option<&str>, tty: bool) -> Option<String> {
    use cli::Cmd;
    match cmd {
        Cmd::New { .. } | Cmd::Attach { .. } | Cmd::Default { .. } => {
            cli::nested(jmux_env).or_else(|| (!tty).then(|| "jmux: not a terminal".to_string()))
        }
        _ => None,
    }
}

/// Runs a parsed command and returns the exit code. `jmux_env` is `$JMUX`.
fn run_cli(cmd: cli::Cmd, dir: &std::path::Path, jmux_env: Option<&str>, tty: bool) -> i32 {
    use cli::Cmd;
    let refuse = |why: String| {
        eprintln!("{why}");
        1
    };
    let attach = |touch: bool, (name, socket): (String, PathBuf)| client::run_attached(&socket, &name, touch);
    if let Some(why) = refusal(&cmd, jmux_env, tty) {
        return refuse(why);
    }
    match cmd {
        Cmd::Help => {
            println!("{}", cli::USAGE);
            0
        }
        Cmd::InstallShellIntegration => {
            let shell = std::env::var("SHELL").ok();
            match cli::install_shell_integration(&integration::Dirs::real(), shell.as_deref()) {
                Ok(line) => {
                    println!("{line}");
                    0
                }
                Err(e) => refuse(e),
            }
        }
        Cmd::Ls => {
            for line in cli::ls(dir) {
                println!("{line}");
            }
            0
        }
        Cmd::Kill { target } => match cli::kill(dir, &target) {
            Ok(line) => {
                println!("{line}");
                0
            }
            Err(e) => refuse(e),
        },
        Cmd::Rename { target, new } => match cli::rename(dir, target.as_deref(), &new, jmux_env) {
            Ok(line) => {
                println!("{line}");
                0
            }
            Err(e) => refuse(e),
        },
        Cmd::Attach { target, touch } => match cli::find(dir, target.as_deref()) {
            Ok(found) => attach(touch, found),
            Err(e) => refuse(e),
        },
        Cmd::New { name, dir: cwd, touch } => {
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("jmux"));
            match cli::start(&exe, dir, name.as_deref(), cwd.as_deref()) {
                Ok(started) => attach(touch, started),
                Err(e) => refuse(e),
            }
        }
        Cmd::Default { touch } => match cli::find(dir, None) {
            Ok(found) => attach(touch, found),
            Err(_) => run_cli(
                Cmd::New {
                    name: None,
                    dir: None,
                    touch,
                },
                dir,
                jmux_env,
                tty,
            ),
        },
    }
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("server") {
        args.next();
        match parse_server_args(args) {
            Ok(args) => std::process::exit(run_server(args)),
            Err(e) => {
                eprintln!("jmux server: {e}");
                std::process::exit(2);
            }
        }
    }
    match cli::parse(args) {
        Ok(cmd) => {
            let dir = jtui::socket::default_socket_dir(APP);
            let jmux_env = std::env::var("JMUX").ok();
            use std::io::IsTerminal;
            let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
            std::process::exit(run_cli(cmd, &dir, jmux_env.as_deref(), tty));
        }
        Err(e) => {
            eprintln!("jmux: {e}\n{}", cli::USAGE);
            std::process::exit(2);
        }
    }
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
