//! Keys and pastes as a terminal sends them to the program in it: the bytes `Run::write` takes.
//!
//! By default xterm's encoding (what `TERM=xterm-256color` promises): plain and Shift chars as
//! UTF-8; Ctrl+letter (and Ctrl+`@[\]^_`, Ctrl+Space) as C0 controls; Alt as an Esc prefix; Enter
//! `\r`, Tab, Shift+Tab `CSI Z`, Backspace DEL (Ctrl+Backspace BS), Esc; arrows / Home / End as
//! `CSI A` (`SS3 A` in application cursor mode), with modifiers `CSI 1;m A`; Insert / Delete /
//! PageUp / PageDown `CSI n ~`; F1–F12. Cmd (Super) never reaches the program in that mode: those
//! keys are the app's.
//!
//! A program that turned on the kitty keyboard protocol
//! (<https://sw.kovidgoyal.net/kitty/keyboard-protocol/>; `Run::kitty_flags`, 0 = off) gets its
//! encoding instead, by `KeyModes::kitty_flags`:
//! - 1 disambiguate: Esc, Alt / Ctrl / Super / Hyper / Meta combinations and keypad keys as
//!   `CSI code ; mods u` (mods = 1 + Shift 1, Alt 2, Ctrl 4, Super 8, Hyper 16, Meta 32, CapsLock
//!   64, NumLock 128, the last two from `KeyEventState`); text keys with no modifier but Shift
//!   stay text; Enter, Tab and Backspace stay legacy unless modified; arrows, Home, End, Insert,
//!   Delete, PageUp / PageDown and F1–F12 keep their legacy forms, with `;mods` when modified
//!   (F3 stays `SS3 R`, not kitty's `CSI 13~`); F13–F35, media keys, PrintScreen, Pause and Menu
//!   use the spec's numbers.
//! - 2 report event types: `:2` (repeat) and `:3` (release) after the modifiers; releases of text
//!   keys, Enter, Tab and Backspace are not sent (unless 8 is on).
//! - 4 alternate keys: `CSI 97:65 ; 2 u`, the shifted char from crossterm's `Char` (the
//!   base-layout key is never known, so never sent).
//! - 8 all keys as escape codes: text keys, Enter, Tab and Backspace too, and the lock and
//!   modifier keys themselves (reported only with this flag).
//! - 16 associated text (with 8): the `Char`'s codepoint as the third field.
//!
//! With no kitty flag the bytes are exactly xterm's, and a release gives `None`.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode, ModifierKeyCode};

/// Kitty flag: disambiguate escape codes.
pub const KITTY_DISAMBIGUATE: u8 = 1;
/// Kitty flag: report repeat and release events.
pub const KITTY_EVENTS: u8 = 2;
/// Kitty flag: report alternate keys (the shifted char).
pub const KITTY_ALTERNATES: u8 = 4;
/// Kitty flag: report all keys as escape codes.
pub const KITTY_ALL: u8 = 8;
/// Kitty flag: report associated text (with `KITTY_ALL`).
pub const KITTY_TEXT: u8 = 16;

/// What the program in the terminal asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// The program asked for `SS3` arrows (`Screen::application_cursor`).
    pub application_cursor: bool,
    /// The kitty keyboard protocol's flags (`Run::kitty_flags`); 0 is xterm's encoding.
    pub kitty_flags: u8,
}

/// The bytes for `key`, or `None` for a key a terminal wouldn't send (Cmd, media keys without the
/// protocol, a release nobody asked for).
pub fn encode(key: &KeyEvent, modes: KeyModes) -> Option<Vec<u8>> {
    if modes.kitty_flags == 0 {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        legacy(key, modes.application_cursor)
    } else {
        kitty(key, modes)
    }
}

/// xterm's encoding.
fn legacy(key: &KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
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

/// The `CSI u` number of a keypad `Char`.
fn keypad_char(c: char) -> Option<u32> {
    Some(match c {
        '0'..='9' => 57399 + (c as u32 - '0' as u32),
        '.' => 57409,
        '/' => 57410,
        '*' => 57411,
        '-' => 57412,
        '+' => 57413,
        '=' => 57415,
        ',' => 57416,
        _ => return None,
    })
}

/// The `CSI u` number of a navigation key (or Enter) on the keypad.
fn keypad_nav(code: KeyCode) -> Option<u32> {
    Some(match code {
        KeyCode::Enter => 57414,
        KeyCode::Left => 57417,
        KeyCode::Right => 57418,
        KeyCode::Up => 57419,
        KeyCode::Down => 57420,
        KeyCode::PageUp => 57421,
        KeyCode::PageDown => 57422,
        KeyCode::Home => 57423,
        KeyCode::End => 57424,
        KeyCode::Insert => 57425,
        KeyCode::Delete => 57426,
        _ => return None,
    })
}

fn media_code(m: MediaKeyCode) -> u32 {
    57428
        + match m {
            MediaKeyCode::Play => 0,
            MediaKeyCode::Pause => 1,
            MediaKeyCode::PlayPause => 2,
            MediaKeyCode::Reverse => 3,
            MediaKeyCode::Stop => 4,
            MediaKeyCode::FastForward => 5,
            MediaKeyCode::Rewind => 6,
            MediaKeyCode::TrackNext => 7,
            MediaKeyCode::TrackPrevious => 8,
            MediaKeyCode::Record => 9,
            MediaKeyCode::LowerVolume => 10,
            MediaKeyCode::RaiseVolume => 11,
            MediaKeyCode::MuteVolume => 12,
        }
}

fn modifier_code(m: ModifierKeyCode) -> u32 {
    57441
        + match m {
            ModifierKeyCode::LeftShift => 0,
            ModifierKeyCode::LeftControl => 1,
            ModifierKeyCode::LeftAlt => 2,
            ModifierKeyCode::LeftSuper => 3,
            ModifierKeyCode::LeftHyper => 4,
            ModifierKeyCode::LeftMeta => 5,
            ModifierKeyCode::RightShift => 6,
            ModifierKeyCode::RightControl => 7,
            ModifierKeyCode::RightAlt => 8,
            ModifierKeyCode::RightSuper => 9,
            ModifierKeyCode::RightHyper => 10,
            ModifierKeyCode::RightMeta => 11,
            ModifierKeyCode::IsoLevel3Shift => 12,
            ModifierKeyCode::IsoLevel5Shift => 13,
        }
}

/// The kitty protocol's encoding of `key` for `modes.kitty_flags` (non-zero).
fn kitty(key: &KeyEvent, modes: KeyModes) -> Option<Vec<u8>> {
    let flags = modes.kitty_flags;
    let all = flags & KITTY_ALL != 0;
    let event: u8 = match key.kind {
        KeyEventKind::Press => 1,
        KeyEventKind::Repeat if flags & KITTY_EVENTS != 0 => 2,
        KeyEventKind::Repeat => 1,
        KeyEventKind::Release if flags & KITTY_EVENTS != 0 => 3,
        KeyEventKind::Release => return None,
    };
    let m = key.modifiers;
    let bit = |flag: KeyModifiers, v: u16| if m.contains(flag) { v } else { 0 };
    let mut real = bit(KeyModifiers::SHIFT, 1)
        | bit(KeyModifiers::ALT, 2)
        | bit(KeyModifiers::CONTROL, 4)
        | bit(KeyModifiers::SUPER, 8)
        | bit(KeyModifiers::HYPER, 16)
        | bit(KeyModifiers::META, 32);
    if key.code == KeyCode::BackTab {
        real |= 1;
    }
    let locks = if key.state.contains(KeyEventState::CAPS_LOCK) {
        64
    } else {
        0
    } | if key.state.contains(KeyEventState::NUM_LOCK) {
        128
    } else {
        0
    };
    let param = 1 + real + locks;
    let keypad = key.state.contains(KeyEventState::KEYPAD);
    // `;mods[:event]`, empty when both are the defaults.
    let fields = if event != 1 {
        format!(";{param}:{event}")
    } else if param > 1 {
        format!(";{param}")
    } else {
        String::new()
    };
    let csi_u = |code: u32, alternate: Option<u32>, text: Option<char>| -> Vec<u8> {
        let mut out = format!("\x1b[{code}");
        if let Some(alt) = alternate {
            out.push_str(&format!(":{alt}"));
        }
        if let Some(t) = text {
            // The mods field can't be skipped when text follows.
            if fields.is_empty() {
                out.push_str(";;");
            } else {
                out.push_str(&fields);
                out.push(';');
            }
            out.push_str(&(t as u32).to_string());
        } else {
            out.push_str(&fields);
        }
        out.push('u');
        out.into_bytes()
    };
    // Cursor keys and F1–F4: the legacy form unmodified, else `CSI 1 ; mods letter`.
    let letter = |l: char, plain: String| -> Vec<u8> {
        if fields.is_empty() {
            plain.into_bytes()
        } else {
            format!("\x1b[1{fields}{l}").into_bytes()
        }
    };
    let cursor = |l: char| {
        letter(
            l,
            if modes.application_cursor {
                format!("\x1bO{l}")
            } else {
                format!("\x1b[{l}")
            },
        )
    };
    let tilde = |n: u8| -> Vec<u8> { format!("\x1b[{n}{fields}~").into_bytes() };
    // Enter, Tab and Backspace: legacy while unmodified (and not a release), else `CSI u`.
    let editing = |code: u32, legacy: u8| -> Option<Vec<u8>> {
        if all || real != 0 {
            Some(csi_u(code, None, None))
        } else if event == 3 {
            None
        } else {
            Some(vec![legacy])
        }
    };
    if keypad && let Some(code) = keypad_nav(key.code) {
        return Some(csi_u(code, None, None));
    }
    // Alt, Ctrl, Super, Hyper, Meta: no text with these, and a text key with one is no longer text.
    let beyond_shift = real & !1 != 0;
    Some(match key.code {
        KeyCode::Char(c) => {
            if keypad && let Some(code) = keypad_char(c) {
                let text = (flags & KITTY_TEXT != 0 && all && event != 3).then_some(c);
                return Some(csi_u(code, None, text));
            }
            if !all && !beyond_shift {
                return (event != 3).then(|| c.to_string().into_bytes());
            }
            let mut lower = c.to_lowercase();
            let code = match (lower.next(), lower.next()) {
                (Some(l), None) => l,
                _ => c,
            };
            let alternate = (flags & KITTY_ALTERNATES != 0 && real & 1 != 0 && code != c).then_some(c as u32);
            let text = (flags & KITTY_TEXT != 0 && all && event != 3 && !beyond_shift).then_some(c);
            csi_u(code as u32, alternate, text)
        }
        KeyCode::Enter => editing(13, b'\r')?,
        KeyCode::Tab | KeyCode::BackTab => editing(9, b'\t')?,
        KeyCode::Backspace => editing(127, 0x7f)?,
        KeyCode::Esc => csi_u(27, None, None),
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let l = (b'P' + n - 1) as char;
            letter(l, format!("\x1bO{l}"))
        }
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        KeyCode::F(n @ 13..=35) => csi_u(57376 + u32::from(n) - 13, None, None),
        KeyCode::KeypadBegin => csi_u(57427, None, None),
        KeyCode::PrintScreen => csi_u(57361, None, None),
        KeyCode::Pause => csi_u(57362, None, None),
        KeyCode::Menu => csi_u(57363, None, None),
        KeyCode::CapsLock if all => csi_u(57358, None, None),
        KeyCode::ScrollLock if all => csi_u(57359, None, None),
        KeyCode::NumLock if all => csi_u(57360, None, None),
        KeyCode::Media(m) => csi_u(media_code(m), None, None),
        KeyCode::Modifier(m) if all => csi_u(modifier_code(m), None, None),
        _ => return None,
    })
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

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    fn key(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode(&KeyEvent::new(code, mods), KeyModes::default()).unwrap()
    }

    /// `code` + `mods` encoded under kitty `flags`, as a string.
    fn kit(flags: u8, code: KeyCode, mods: KeyModifiers) -> Option<String> {
        kit_ev(flags, KeyEvent::new(code, mods))
    }

    fn kit_ev(flags: u8, ev: KeyEvent) -> Option<String> {
        let modes = KeyModes {
            application_cursor: false,
            kitty_flags: flags,
        };
        encode(&ev, modes).map(|b| String::from_utf8(b).unwrap().replace('\x1b', "ESC"))
    }

    fn with_kind(code: KeyCode, mods: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, mods, kind)
    }

    fn with_state(code: KeyCode, mods: KeyModifiers, state: KeyEventState) -> KeyEvent {
        KeyEvent::new_with_kind_and_state(code, mods, KeyEventKind::Press, state)
    }

    #[test]
    fn chars_and_controls() {
        assert_eq!(key(KeyCode::Char('y'), NONE), b"y");
        assert_eq!(key(KeyCode::Char('Y'), SHIFT), b"Y");
        assert_eq!(key(KeyCode::Char('é'), NONE), "é".as_bytes());
        assert_eq!(key(KeyCode::Char('c'), CTRL), [3]);
        assert_eq!(key(KeyCode::Char('D'), CTRL | SHIFT), [4]);
        assert_eq!(key(KeyCode::Char(' '), CTRL), [0]);
        assert_eq!(key(KeyCode::Char('b'), ALT), b"\x1bb");
        assert_eq!(key(KeyCode::Enter, NONE), b"\r");
        assert_eq!(key(KeyCode::Backspace, NONE), [0x7f]);
        assert_eq!(key(KeyCode::Esc, NONE), [0x1b]);
        assert_eq!(key(KeyCode::Tab, SHIFT), b"\x1b[Z");
        let none = KeyModes::default();
        assert_eq!(
            encode(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER), none),
            None
        );
        assert_eq!(encode(&KeyEvent::new(KeyCode::Char('1'), CTRL), none), None);
    }

    #[test]
    fn cursor_and_editing_keys() {
        assert_eq!(key(KeyCode::Up, NONE), b"\x1b[A");
        let app = KeyModes {
            application_cursor: true,
            kitty_flags: 0,
        };
        assert_eq!(encode(&KeyEvent::new(KeyCode::Up, NONE), app).unwrap(), b"\x1bOA");
        assert_eq!(key(KeyCode::Right, CTRL), b"\x1b[1;5C");
        assert_eq!(key(KeyCode::Home, SHIFT), b"\x1b[1;2H");
        assert_eq!(key(KeyCode::Delete, NONE), b"\x1b[3~");
        assert_eq!(key(KeyCode::PageDown, ALT), b"\x1b[6;3~");
        assert_eq!(key(KeyCode::F(1), NONE), b"\x1bOP");
        assert_eq!(key(KeyCode::F(5), NONE), b"\x1b[15~");
        assert_eq!(key(KeyCode::F(12), NONE), b"\x1b[24~");
    }

    #[test]
    fn flags_zero_ignore_the_kind_and_drop_releases() {
        let none = KeyModes::default();
        let press = with_kind(KeyCode::Char('a'), NONE, KeyEventKind::Repeat);
        assert_eq!(encode(&press, none).unwrap(), b"a");
        let release = with_kind(KeyCode::Char('a'), NONE, KeyEventKind::Release);
        assert_eq!(encode(&release, none), None);
    }

    /// Flag 1: what becomes `CSI u` and what stays legacy.
    #[test]
    fn kitty_disambiguate() {
        let d = KITTY_DISAMBIGUATE;
        let cases: &[(KeyCode, KeyModifiers, Option<&str>)] = &[
            (KeyCode::Esc, NONE, Some("ESC[27u")),
            (KeyCode::Esc, ALT, Some("ESC[27;3u")),
            (KeyCode::Char('a'), NONE, Some("a")),
            (KeyCode::Char('A'), SHIFT, Some("A")),
            (KeyCode::Char('é'), NONE, Some("é")),
            (KeyCode::Char('a'), ALT, Some("ESC[97;3u")),
            (KeyCode::Char('A'), ALT | SHIFT, Some("ESC[97;4u")),
            (KeyCode::Char('c'), CTRL, Some("ESC[99;5u")),
            (KeyCode::Char('D'), CTRL | SHIFT, Some("ESC[100;6u")),
            (KeyCode::Char('i'), CTRL | KeyModifiers::SUPER, Some("ESC[105;13u")),
            (
                KeyCode::Char('x'),
                KeyModifiers::HYPER | KeyModifiers::META,
                Some("ESC[120;49u"),
            ),
            (KeyCode::Char(' '), CTRL, Some("ESC[32;5u")),
            (KeyCode::Enter, NONE, Some("\r")),
            (KeyCode::Tab, NONE, Some("\t")),
            (KeyCode::Backspace, NONE, Some("\u{7f}")),
            (KeyCode::Enter, CTRL, Some("ESC[13;5u")),
            (KeyCode::Enter, SHIFT, Some("ESC[13;2u")),
            (KeyCode::Tab, SHIFT, Some("ESC[9;2u")),
            (KeyCode::BackTab, SHIFT, Some("ESC[9;2u")),
            (KeyCode::Backspace, ALT, Some("ESC[127;3u")),
            (KeyCode::Up, NONE, Some("ESC[A")),
            (KeyCode::Up, CTRL | SHIFT, Some("ESC[1;6A")),
            (KeyCode::Home, NONE, Some("ESC[H")),
            (KeyCode::End, ALT, Some("ESC[1;3F")),
            (KeyCode::Delete, NONE, Some("ESC[3~")),
            (KeyCode::PageUp, CTRL, Some("ESC[5;5~")),
            (KeyCode::F(1), NONE, Some("ESCOP")),
            (KeyCode::F(3), CTRL, Some("ESC[1;5R")),
            (KeyCode::F(5), NONE, Some("ESC[15~")),
            (KeyCode::F(12), SHIFT, Some("ESC[24;2~")),
            (KeyCode::F(13), NONE, Some("ESC[57376u")),
            (KeyCode::F(35), CTRL, Some("ESC[57398;5u")),
            (KeyCode::F(36), NONE, None),
            (KeyCode::PrintScreen, NONE, Some("ESC[57361u")),
            (KeyCode::Menu, SHIFT, Some("ESC[57363;2u")),
            (KeyCode::KeypadBegin, NONE, Some("ESC[57427u")),
            (KeyCode::Media(MediaKeyCode::PlayPause), NONE, Some("ESC[57430u")),
            (KeyCode::Media(MediaKeyCode::MuteVolume), NONE, Some("ESC[57440u")),
            // Not reported without flag 8.
            (KeyCode::CapsLock, NONE, None),
            (KeyCode::Modifier(ModifierKeyCode::LeftShift), SHIFT, None),
            (KeyCode::Null, NONE, None),
        ];
        for (code, mods, want) in cases {
            assert_eq!(kit(d, *code, *mods).as_deref(), *want, "{code:?} + {mods:?}");
        }
    }

    #[test]
    fn kitty_application_cursor_keeps_ss3_unmodified() {
        let modes = KeyModes {
            application_cursor: true,
            kitty_flags: KITTY_DISAMBIGUATE,
        };
        let up = encode(&KeyEvent::new(KeyCode::Up, NONE), modes).unwrap();
        assert_eq!(up, b"\x1bOA");
        let ctrl_up = encode(&KeyEvent::new(KeyCode::Up, CTRL), modes).unwrap();
        assert_eq!(ctrl_up, b"\x1b[1;5A");
    }

    #[test]
    fn kitty_keypad_and_locks() {
        let d = KITTY_DISAMBIGUATE;
        let kp = |code, mods| with_state(code, mods, KeyEventState::KEYPAD);
        assert_eq!(kit_ev(d, kp(KeyCode::Char('5'), NONE)).as_deref(), Some("ESC[57404u"));
        assert_eq!(kit_ev(d, kp(KeyCode::Char('+'), NONE)).as_deref(), Some("ESC[57413u"));
        assert_eq!(kit_ev(d, kp(KeyCode::Char('.'), CTRL)).as_deref(), Some("ESC[57409;5u"));
        assert_eq!(kit_ev(d, kp(KeyCode::Enter, NONE)).as_deref(), Some("ESC[57414u"));
        assert_eq!(kit_ev(d, kp(KeyCode::Left, NONE)).as_deref(), Some("ESC[57417u"));
        assert_eq!(kit_ev(d, kp(KeyCode::Delete, SHIFT)).as_deref(), Some("ESC[57426;2u"));
        // Off the keypad the same keys are plain.
        assert_eq!(kit(d, KeyCode::Char('5'), NONE).as_deref(), Some("5"));
        assert_eq!(kit(d, KeyCode::Enter, NONE).as_deref(), Some("\r"));
        // Lock state shows in the modifiers once something is reported, not in plain text.
        let state = KeyEventState::KEYPAD | KeyEventState::NUM_LOCK;
        assert_eq!(
            kit_ev(d, with_state(KeyCode::Char('5'), NONE, state)).as_deref(),
            Some("ESC[57404;129u")
        );
        let caps = KeyEventState::CAPS_LOCK;
        assert_eq!(
            kit_ev(d, with_state(KeyCode::Char('a'), NONE, caps)).as_deref(),
            Some("a")
        );
        assert_eq!(
            kit_ev(d, with_state(KeyCode::Char('a'), CTRL, caps)).as_deref(),
            Some("ESC[97;69u")
        );
        assert_eq!(
            kit_ev(d, with_state(KeyCode::Esc, NONE, caps)).as_deref(),
            Some("ESC[27;65u")
        );
    }

    /// Flag 2: `:2` / `:3` after the modifiers; no release for text, Enter, Tab, Backspace.
    #[test]
    fn kitty_event_types() {
        let f = KITTY_DISAMBIGUATE | KITTY_EVENTS;
        let ev = |code, mods, kind| kit_ev(f, with_kind(code, mods, kind)).map(|s| s.replace('\u{7f}', "DEL"));
        use KeyEventKind::{Press, Release, Repeat};
        assert_eq!(ev(KeyCode::Char('a'), CTRL, Press).as_deref(), Some("ESC[97;5u"));
        assert_eq!(ev(KeyCode::Char('a'), CTRL, Repeat).as_deref(), Some("ESC[97;5:2u"));
        assert_eq!(ev(KeyCode::Char('a'), CTRL, Release).as_deref(), Some("ESC[97;5:3u"));
        assert_eq!(ev(KeyCode::Esc, NONE, Release).as_deref(), Some("ESC[27;1:3u"));
        assert_eq!(ev(KeyCode::Up, NONE, Release).as_deref(), Some("ESC[1;1:3A"));
        assert_eq!(ev(KeyCode::Up, SHIFT, Repeat).as_deref(), Some("ESC[1;2:2A"));
        assert_eq!(ev(KeyCode::Delete, NONE, Release).as_deref(), Some("ESC[3;1:3~"));
        assert_eq!(ev(KeyCode::F(2), NONE, Release).as_deref(), Some("ESC[1;1:3Q"));
        assert_eq!(ev(KeyCode::F(13), NONE, Repeat).as_deref(), Some("ESC[57376;1:2u"));
        // Text repeats as text; its release isn't sent.
        assert_eq!(ev(KeyCode::Char('a'), NONE, Repeat).as_deref(), Some("a"));
        assert_eq!(ev(KeyCode::Char('a'), NONE, Release), None);
        // Enter, Tab and Backspace: legacy press / repeat, no release; modified they report it.
        assert_eq!(ev(KeyCode::Enter, NONE, Press).as_deref(), Some("\r"));
        assert_eq!(ev(KeyCode::Enter, NONE, Release), None);
        assert_eq!(ev(KeyCode::Tab, NONE, Release), None);
        assert_eq!(ev(KeyCode::Backspace, NONE, Release), None);
        assert_eq!(ev(KeyCode::Enter, CTRL, Release).as_deref(), Some("ESC[13;5:3u"));
        // Without flag 2, repeat is a press and release is dropped.
        let d = KITTY_DISAMBIGUATE;
        assert_eq!(
            kit_ev(d, with_kind(KeyCode::Esc, NONE, Repeat)).as_deref(),
            Some("ESC[27u")
        );
        assert_eq!(kit_ev(d, with_kind(KeyCode::Esc, NONE, Release)), None);
    }

    /// Flag 4: the shifted char after the key's own code.
    #[test]
    fn kitty_alternate_keys() {
        let f = KITTY_DISAMBIGUATE | KITTY_ALTERNATES;
        assert_eq!(
            kit(f, KeyCode::Char('A'), CTRL | SHIFT).as_deref(),
            Some("ESC[97:65;6u")
        );
        assert_eq!(kit(f, KeyCode::Char('A'), ALT | SHIFT).as_deref(), Some("ESC[97:65;4u"));
        // No shift or an unchanged char: no alternate.
        assert_eq!(kit(f, KeyCode::Char('a'), CTRL).as_deref(), Some("ESC[97;5u"));
        assert_eq!(kit(f, KeyCode::Char('1'), CTRL | SHIFT).as_deref(), Some("ESC[49;6u"));
        // Plain text stays text.
        assert_eq!(kit(f, KeyCode::Char('A'), SHIFT).as_deref(), Some("A"));
        // With flag 8 the shifted letter is an escape code with an alternate.
        let all = f | KITTY_ALL;
        assert_eq!(kit(all, KeyCode::Char('A'), SHIFT).as_deref(), Some("ESC[97:65;2u"));
        assert_eq!(kit(all, KeyCode::Char('a'), NONE).as_deref(), Some("ESC[97u"));
    }

    /// Flags 8 and 16: every key as an escape code, the modifier keys, the text.
    #[test]
    fn kitty_all_keys_and_text() {
        let all = KITTY_DISAMBIGUATE | KITTY_ALL;
        assert_eq!(kit(all, KeyCode::Char('a'), NONE).as_deref(), Some("ESC[97u"));
        assert_eq!(kit(all, KeyCode::Char('é'), NONE).as_deref(), Some("ESC[233u"));
        assert_eq!(kit(all, KeyCode::Enter, NONE).as_deref(), Some("ESC[13u"));
        assert_eq!(kit(all, KeyCode::Tab, NONE).as_deref(), Some("ESC[9u"));
        assert_eq!(kit(all, KeyCode::Backspace, NONE).as_deref(), Some("ESC[127u"));
        assert_eq!(kit(all, KeyCode::Up, NONE).as_deref(), Some("ESC[A"));
        assert_eq!(kit(all, KeyCode::CapsLock, NONE).as_deref(), Some("ESC[57358u"));
        assert_eq!(kit(all, KeyCode::ScrollLock, NONE).as_deref(), Some("ESC[57359u"));
        assert_eq!(kit(all, KeyCode::NumLock, NONE).as_deref(), Some("ESC[57360u"));
        let mod_key = |m, mods| kit(all, KeyCode::Modifier(m), mods);
        assert_eq!(
            mod_key(ModifierKeyCode::LeftShift, SHIFT).as_deref(),
            Some("ESC[57441;2u")
        );
        assert_eq!(
            mod_key(ModifierKeyCode::LeftControl, CTRL).as_deref(),
            Some("ESC[57442;5u")
        );
        assert_eq!(mod_key(ModifierKeyCode::RightAlt, ALT).as_deref(), Some("ESC[57449;3u"));
        assert_eq!(
            mod_key(ModifierKeyCode::IsoLevel5Shift, NONE).as_deref(),
            Some("ESC[57454u")
        );
        let released = with_kind(
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            NONE,
            KeyEventKind::Release,
        );
        assert_eq!(kit_ev(all | KITTY_EVENTS, released).as_deref(), Some("ESC[57441;1:3u"));
        // Text (16): the codepoint, after an (empty) modifiers field.
        let text = all | KITTY_TEXT;
        assert_eq!(kit(text, KeyCode::Char('a'), NONE).as_deref(), Some("ESC[97;;97u"));
        assert_eq!(kit(text, KeyCode::Char('A'), SHIFT).as_deref(), Some("ESC[97;2;65u"));
        assert_eq!(kit(text, KeyCode::Char('é'), NONE).as_deref(), Some("ESC[233;;233u"));
        assert_eq!(kit(text, KeyCode::Char('a'), CTRL).as_deref(), Some("ESC[97;5u"));
        assert_eq!(kit(text, KeyCode::Enter, NONE).as_deref(), Some("ESC[13u"));
        let release = with_kind(KeyCode::Char('a'), NONE, KeyEventKind::Release);
        assert_eq!(kit_ev(text | KITTY_EVENTS, release).as_deref(), Some("ESC[97;1:3u"));
        // Flag 8 sends Enter, Tab and Backspace releases (flag 2 alone does not).
        let enter_up = with_kind(KeyCode::Enter, NONE, KeyEventKind::Release);
        assert_eq!(kit_ev(all | KITTY_EVENTS, enter_up).as_deref(), Some("ESC[13;1:3u"));
        // 16 without 8 sends no text field.
        assert_eq!(
            kit(KITTY_DISAMBIGUATE | KITTY_TEXT, KeyCode::Char('a'), CTRL).as_deref(),
            Some("ESC[97;5u")
        );
    }

    #[test]
    fn pastes() {
        assert_eq!(paste("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
