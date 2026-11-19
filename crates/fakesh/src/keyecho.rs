//! `keyecho [flags]`: a program that asks for the kitty keyboard protocol, for the terminal tests
//! of jmux and jed. It puts the tty in raw mode, pushes `flags` (default 1), queries them and
//! prints `flags=<n>` from the reply, then prints each chunk of bytes it reads as `hex: 1b 5b ..`
//! until a lone `q`; then it pops the flags, restores the tty and prints `bye`.

use std::io::Write;

/// The flags argument: none means 1 (disambiguate); anything but a number 0..=31 is `None`.
pub fn parse_flags(args: &[String]) -> Option<u8> {
    match args.first() {
        None => Some(1),
        Some(a) => a.parse::<u8>().ok().filter(|f| *f < 32),
    }
}

/// `1b 5b 39 37 75`: `bytes` as lower-case hex, space separated.
pub fn hex_line(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// The flags in a `CSI ? <n> u` reply.
pub fn reply_flags(reply: &[u8]) -> Option<u8> {
    let text = std::str::from_utf8(reply).ok()?;
    text.strip_prefix("\x1b[?")?.strip_suffix('u')?.parse().ok()
}

/// One `read` from fd 0 (raw mode: whatever bytes are there); empty at end of input.
fn read_chunk() -> Vec<u8> {
    let mut buf = [0u8; 64];
    // SAFETY: `buf` is a live, writable buffer of the length given.
    let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
    if n > 0 { buf[..n as usize].to_vec() } else { Vec::new() }
}

/// Runs the command; the status for `$?`.
pub fn run(args: &[String], out: &mut impl Write) -> i32 {
    let Some(flags) = parse_flags(args) else {
        let _ = writeln!(out, "keyecho: bad flags");
        return 2;
    };
    // SAFETY: termios is plain data; fd 0 is the tty (tcgetattr failing just skips raw mode).
    let saved = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        (libc::tcgetattr(0, &mut t) == 0).then_some(t)
    };
    if let Some(mut raw) = saved {
        // SAFETY: `raw` is a valid termios.
        unsafe {
            libc::cfmakeraw(&mut raw);
            libc::tcsetattr(0, libc::TCSANOW, &raw);
        }
    }
    let _ = write!(out, "\x1b[>{flags}u\x1b[?u");
    let _ = out.flush();
    let mut reply = Vec::new();
    while !reply.ends_with(b"u") {
        let chunk = read_chunk();
        if chunk.is_empty() {
            break;
        }
        reply.extend(chunk);
    }
    match reply_flags(&reply) {
        Some(n) => {
            let _ = write!(out, "flags={n}\r\n");
        }
        None => {
            let _ = write!(out, "no reply: {}\r\n", hex_line(&reply));
        }
    }
    let _ = out.flush();
    loop {
        let chunk = read_chunk();
        if chunk.is_empty() || chunk == b"q" {
            break;
        }
        let _ = write!(out, "hex: {}\r\n", hex_line(&chunk));
        let _ = out.flush();
    }
    let _ = write!(out, "\x1b[<u");
    if let Some(t) = saved {
        // SAFETY: `t` is the termios read above.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
    }
    let _ = writeln!(out, "bye");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_hex_and_reply_parse() {
        assert_eq!(parse_flags(&[]), Some(1));
        assert_eq!(parse_flags(&["3".into()]), Some(3));
        assert_eq!(parse_flags(&["0".into()]), Some(0));
        assert_eq!(parse_flags(&["32".into()]), None);
        assert_eq!(parse_flags(&["x".into()]), None);
        assert_eq!(hex_line(b"\x1b[97;6u"), "1b 5b 39 37 3b 36 75");
        assert_eq!(hex_line(&[]), "");
        assert_eq!(reply_flags(b"\x1b[?3u"), Some(3));
        assert_eq!(reply_flags(b"\x1b[?0u"), Some(0));
        assert_eq!(reply_flags(b"\x1b[?x"), None);
    }
}
