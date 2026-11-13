//! jmux: a terminal multiplexer with detachable sessions (design in `jmux/PLAN.md`).

// The protocol and session modules are used by the server and the client (jmux_server-b/c).
#[allow(dead_code)]
mod protocol;
#[allow(dead_code)]
mod session;

/// The application name passed to jtui (directory names, sockets).
pub const APP: &str = "jmux";

fn main() {
    eprintln!("jmux: not yet implemented");
    std::process::exit(2);
}
