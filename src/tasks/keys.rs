//! Keys and pastes as a terminal sends them to the program in it (xterm's encoding, what
//! `TERM=xterm-256color` promises): the bytes `Run::write` takes.
//!
//! Plain and Shift chars as UTF-8; Ctrl+letter (and Ctrl+`@[\]^_`, Ctrl+Space) as C0 controls;
//! Alt as an Esc prefix; Enter `\r`, Tab, Shift+Tab `CSI Z`, Backspace DEL (Ctrl+Backspace BS),
//! Esc; arrows / Home / End as `CSI A` (`SS3 A` in application cursor mode), with modifiers
//! `CSI 1;m A`; Insert / Delete / PageUp / PageDown `CSI n ~`; F1–F12. Cmd (Super) never reaches
//! the program: those keys are red's.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The bytes for `key`, or `None` for a key a terminal wouldn't send (Cmd, media keys).
/// `application_cursor`: the program asked for `SS3` arrows (`Screen::application_cursor`).
pub fn encode(key: &KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    if mods.contains(KeyModifiers::SUPER) || mods.contains(KeyModifiers::HYPER) || mods.contains(KeyModifiers::META) {
        return None;
    }
    let shift = mods.contains(KeyModifiers::SHIFT);
    let alt = mods.contains(KeyModifiers::ALT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    // xterm's modifier parameter: 1 + Shift 1 + Alt 2 + Ctrl 4.
    let param = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let csi_letter = |letter: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{letter}").into_bytes()
        } else if application_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[{n};{param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let esc_prefix = |mut bytes: Vec<u8>| -> Vec<u8> {
        if alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };
    let bytes = match key.code {
        KeyCode::Char(c) if ctrl => esc_prefix(vec![control(c)?]),
        KeyCode::Char(c) => esc_prefix(c.to_string().into_bytes()),
        KeyCode::Enter => esc_prefix(b"\r".to_vec()),
        KeyCode::Tab if shift => b"\x1b[Z".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Tab => esc_prefix(b"\t".to_vec()),
        KeyCode::Backspace if ctrl => esc_prefix(vec![0x08]),
        KeyCode::Backspace => esc_prefix(vec![0x7f]),
        KeyCode::Esc => esc_prefix(vec![0x1b]),
        KeyCode::Up => csi_letter('A'),
        KeyCode::Down => csi_letter('B'),
        KeyCode::Right => csi_letter('C'),
        KeyCode::Left => csi_letter('D'),
        KeyCode::Home => csi_letter('H'),
        KeyCode::End => csi_letter('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let letter = (b'P' + n - 1) as char;
            if param > 1 {
                format!("\x1b[1;{param}{letter}").into_bytes()
            } else {
                format!("\x1bO{letter}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        _ => return None,
    };
    Some(bytes)
}

/// Ctrl+`c`'s control character: letters (either case), `@` / Space (NUL), `[ \ ] ^ _`, `?` (DEL).
fn control(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// A paste: line breaks as Enter (`\r`, as terminals send them), wrapped in `ESC [200~` …
/// `ESC [201~` when the program asked for bracketed paste (any end marker inside dropped).
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let inner = text.replace("\x1b[201~", "");
        format!("\x1b[200~{inner}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode(&KeyEvent::new(code, mods), false).unwrap()
    }

    #[test]
    fn chars_and_controls() {
        let none = KeyModifiers::NONE;
        assert_eq!(key(KeyCode::Char('y'), none), b"y");
        assert_eq!(key(KeyCode::Char('Y'), KeyModifiers::SHIFT), b"Y");
        assert_eq!(key(KeyCode::Char('é'), none), "é".as_bytes());
        assert_eq!(key(KeyCode::Char('c'), KeyModifiers::CONTROL), [3]);
        assert_eq!(
            key(KeyCode::Char('D'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            [4]
        );
        assert_eq!(key(KeyCode::Char(' '), KeyModifiers::CONTROL), [0]);
        assert_eq!(key(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
        assert_eq!(key(KeyCode::Enter, none), b"\r");
        assert_eq!(key(KeyCode::Backspace, none), [0x7f]);
        assert_eq!(key(KeyCode::Esc, none), [0x1b]);
        assert_eq!(key(KeyCode::Tab, KeyModifiers::SHIFT), b"\x1b[Z");
        assert_eq!(
            encode(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER), false),
            None
        );
        assert_eq!(
            encode(&KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL), false),
            None
        );
    }

    #[test]
    fn cursor_and_editing_keys() {
        let none = KeyModifiers::NONE;
        assert_eq!(key(KeyCode::Up, none), b"\x1b[A");
        let app = encode(&KeyEvent::new(KeyCode::Up, none), true).unwrap();
        assert_eq!(app, b"\x1bOA");
        assert_eq!(key(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C");
        assert_eq!(key(KeyCode::Home, KeyModifiers::SHIFT), b"\x1b[1;2H");
        assert_eq!(key(KeyCode::Delete, none), b"\x1b[3~");
        assert_eq!(key(KeyCode::PageDown, KeyModifiers::ALT), b"\x1b[6;3~");
        assert_eq!(key(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(key(KeyCode::F(5), none), b"\x1b[15~");
        assert_eq!(key(KeyCode::F(12), none), b"\x1b[24~");
    }

    #[test]
    fn pastes() {
        assert_eq!(paste("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
