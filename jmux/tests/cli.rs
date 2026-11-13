//! The real `jmux` binary: a detached-style server process killed with `kill -9` (the stale socket
//! is then cleaned up by `jmux ls`), the CLI's answers, and the `$JMUX` refusal. Environment goes
//! to the children with `Command::env`, never `set_var`. Every wait has a deadline and every
//! process this starts is killed.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(10);

struct Env {
    root: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let root = tempfile::tempdir().unwrap();
        Env { root }
    }

    /// The socket directory the binary uses: `$XDG_RUNTIME_DIR/jmux`.
    fn sockets(&self) -> PathBuf {
        self.root.path().join("run").join("jmux")
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_jmux"));
        c.env("XDG_RUNTIME_DIR", self.root.path().join("run"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("SHELL", "/bin/sh")
            .env_remove("JMUX")
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    /// The server process, as `jmux new` starts it.
    fn server(&self, name: &str) -> Server {
        let run = self.root.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        let mut c = self.command();
        let child = c
            .args(["server", "--session", name, "--socket-dir"])
            .arg(self.sockets())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let socket = self.sockets().join(format!("{name}.sock"));
        let deadline = Instant::now() + WAIT;
        let server = Server { child, socket };
        while std::os::unix::net::UnixStream::connect(&server.socket).is_err() {
            assert!(Instant::now() < deadline, "the server did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        server
    }
}

struct Server {
    child: Child,
    socket: PathBuf,
}

impl Server {
    /// Waits for the process to exit.
    fn wait(&mut self) {
        let deadline = Instant::now() + WAIT;
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the server did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn exists(p: &Path) -> bool {
    p.exists()
}

#[test]
fn jmux_server_killing_the_server_leaves_a_stale_socket_that_ls_removes() {
    let env = Env::new();
    let mut server = env.server("work");
    let out = env.run(&["ls"]);
    let listing = text(&out.stdout);
    assert!(out.status.success());
    assert!(listing.starts_with("work: 1 terminal, "), "{listing}");
    assert!(listing.trim_end().ends_with(", detached"), "{listing}");

    // SAFETY: kill(2) of a child this test started.
    unsafe { libc::kill(server.child.id() as i32, libc::SIGKILL) };
    server.wait();
    assert!(exists(&server.socket), "kill -9 leaves the socket behind");
    let out = env.run(&["ls"]);
    assert_eq!(text(&out.stdout), "no sessions\n");
    assert!(!exists(&server.socket), "ls removes the stale socket");
}

#[test]
fn jmux_server_kill_session_and_rename_session_through_the_binary() {
    let env = Env::new();
    let mut server = env.server("work");
    let out = env.run(&["rename-session", "-t", "work", "play"]);
    assert_eq!(
        text(&out.stdout),
        "[session work renamed to play]\n",
        "{}",
        text(&out.stderr)
    );
    assert!(text(&env.run(&["ls"]).stdout).starts_with("play: "));
    let out = env.run(&["rename-session", "-t", "nothing", "x"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stderr), "jmux: no session nothing\n");
    let out = env.run(&["kill-session", "-t", "play"]);
    assert_eq!(text(&out.stdout), "[session play killed]\n");
    server.wait();
    assert_eq!(text(&env.run(&["ls"]).stdout), "no sessions\n");
}

#[test]
fn jmux_server_the_binary_refuses_to_nest_and_to_attach_to_nothing() {
    let env = Env::new();
    let _server = env.server("work");
    let inside = env.sockets().join("work.sock");
    for args in [&["new"][..], &["attach"], &[]] {
        let out = env.command().args(args).env("JMUX", &inside).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(
            text(&out.stderr),
            "jmux: already inside session work (unset JMUX to nest anyway)\n",
            "{args:?}"
        );
    }
    // `ls` still works inside.
    let out = env.command().arg("ls").env("JMUX", &inside).output().unwrap();
    assert!(text(&out.stdout).starts_with("work: "));
    let out = env.run(&["bogus"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn jmux_server_without_a_terminal_new_and_bare_jmux_start_nothing() {
    let env = Env::new();
    for args in [&["new", "-s", "x"][..], &[], &["attach", "-t", "x"]] {
        let out = env.run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&out.stderr), "jmux: not a terminal\n", "{args:?}");
        assert!(!exists(&env.sockets().join("x.sock")), "{args:?}");
        let any = std::fs::read_dir(env.sockets()).map_or(0, Iterator::count);
        assert_eq!(any, 0, "{args:?} left something in the socket directory");
    }
}
