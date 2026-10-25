//! Mouse events as a terminal sends them to the program in it (xterm's mouse protocol), the
//! counterpart of `keys`: the bytes `Run::write` takes.
//!
//! The program's modes come from its `vt100` screen (`mouse_protocol_mode` /
//! `mouse_protocol_encoding`, set by the `CSI ? h` sequences it prints). SGR (1006): press
//! `CSI < b ; x ; y M`, release `…m`. X10 (1000) and UTF-8 (1005, treated the same): `CSI M` and
//! three bytes, press-only in mode `Press` (X10's own, `CSI ? 9 h`) and a release as code 3 in
//! `PressRelease`. Motion is reported while a button is held (`ButtonMotion`, code 32) and always
//! in `AnyMotion` (code 35); the wheel is buttons 64–67 (also 66 / 67 sideways); Shift / Alt /
//! Ctrl add 4 / 8 / 16. Nothing while the program hasn't asked for the mouse.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

/// xterm's motion code (added to the button's number, no buttons held: 3).
const MOTION: u8 = 32;
/// xterm's wheel buttons: up, down, left, right.
const WHEEL: u8 = 64;
/// The largest X10 coordinate (its bytes are the cell plus 32).
const X10_MAX: u16 = 223;

/// The bytes for `event`, or `None` while the program hasn't asked for the mouse or doesn't
/// report this event in its mode. `event`'s column and row are 0-based cells in the run's screen
/// (the protocol's are 1-based).
pub fn encode(event: &MouseEvent, mode: MouseProtocolMode, encoding: MouseProtocolEncoding) -> Option<Vec<u8>> {
    let (code, release) = code(event, mode)?;
    let mods = modifiers(event.modifiers);
    let (x, y) = (u32::from(event.column) + 1, u32::from(event.row) + 1);
    Some(match encoding {
        MouseProtocolEncoding::Sgr => {
            let end = if release { 'm' } else { 'M' };
            format!("\x1b[<{};{x};{y}{end}", code + mods).into_bytes()
        }
        // X10's three bytes: the button (a release is 3) and the cell, each plus 32.
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let code = if release { 3 } else { code } + mods;
            let x = x.clamp(1, u32::from(X10_MAX)) as u8 + 32;
            let y = y.clamp(1, u32::from(X10_MAX)) as u8 + 32;
            vec![0x1b, b'[', b'M', code + MOTION, x, y]
        }
    })
}

/// The button number and whether it's a release, or `None` for an event the mode doesn't report.
fn code(event: &MouseEvent, mode: MouseProtocolMode) -> Option<(u8, bool)> {
    if mode == MouseProtocolMode::None {
        return None;
    }
    let motion = || matches!(mode, MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion);
    let press_release = || mode != MouseProtocolMode::Press;
    Some(match event.kind {
        MouseEventKind::Down(button) => (button_code(button), false),
        MouseEventKind::Up(button) if press_release() => (button_code(button), true),
        MouseEventKind::Drag(button) if motion() => (MOTION + button_code(button), false),
        MouseEventKind::Moved if mode == MouseProtocolMode::AnyMotion => (MOTION + 3, false),
        MouseEventKind::ScrollUp => (WHEEL, false),
        MouseEventKind::ScrollDown => (WHEEL + 1, false),
        MouseEventKind::ScrollLeft => (WHEEL + 2, false),
        MouseEventKind::ScrollRight => (WHEEL + 3, false),
        _ => return None,
    })
}

fn button_code(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        // The SGR codes. Back / forward never reach here at runtime: `App::mouse_event` consumes
        // them (navigation on the press, nothing on a drag / release) before a terminal tab encodes
        // anything.
        MouseButton::Back => 128,
        MouseButton::Forward => 129,
    }
}

fn modifiers(mods: KeyModifiers) -> u8 {
    4 * mods.contains(KeyModifiers::SHIFT) as u8
        + 8 * mods.contains(KeyModifiers::ALT) as u8
        + 16 * mods.contains(KeyModifiers::CONTROL) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const SGR: MouseProtocolEncoding = MouseProtocolEncoding::Sgr;
    const X10: MouseProtocolEncoding = MouseProtocolEncoding::Default;

    fn event(kind: MouseEventKind, column: u16, row: u16, mods: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: mods,
        }
    }

    /// `kind` at screen cell (2, 1) — protocol cell (3, 2) — in `mode` with `encoding`.
    fn bytes(kind: MouseEventKind, mode: MouseProtocolMode, encoding: MouseProtocolEncoding) -> Option<Vec<u8>> {
        encode(&event(kind, 2, 1, KeyModifiers::NONE), mode, encoding)
    }

    #[test]
    fn sgr_press_and_release() {
        let press = MouseEventKind::Down(MouseButton::Left);
        let release = MouseEventKind::Up(MouseButton::Left);
        assert_eq!(
            bytes(press, MouseProtocolMode::PressRelease, SGR).unwrap(),
            b"\x1b[<0;3;2M"
        );
        assert_eq!(
            bytes(release, MouseProtocolMode::PressRelease, SGR).unwrap(),
            b"\x1b[<0;3;2m"
        );
        assert_eq!(
            bytes(
                MouseEventKind::Down(MouseButton::Middle),
                MouseProtocolMode::PressRelease,
                SGR
            )
            .unwrap(),
            b"\x1b[<1;3;2M"
        );
        assert_eq!(
            bytes(
                MouseEventKind::Up(MouseButton::Right),
                MouseProtocolMode::PressRelease,
                SGR
            )
            .unwrap(),
            b"\x1b[<2;3;2m"
        );
        // X10's own mode sends presses only.
        assert_eq!(bytes(press, MouseProtocolMode::Press, SGR).unwrap(), b"\x1b[<0;3;2M");
        assert_eq!(bytes(release, MouseProtocolMode::Press, SGR), None);
        assert_eq!(bytes(press, MouseProtocolMode::None, SGR), None);
    }

    /// The back / forward buttons' SGR codes (128 / 129, drags 160 / 161) flow through the real
    /// encoder: `button_code`'s arms for them, though jed consumes the buttons before a terminal
    /// tab can encode one.
    #[test]
    fn sgr_back_and_forward() {
        assert_eq!(
            bytes(
                MouseEventKind::Down(MouseButton::Back),
                MouseProtocolMode::PressRelease,
                SGR
            )
            .unwrap(),
            b"\x1b[<128;3;2M"
        );
        assert_eq!(
            bytes(
                MouseEventKind::Up(MouseButton::Back),
                MouseProtocolMode::PressRelease,
                SGR
            )
            .unwrap(),
            b"\x1b[<128;3;2m"
        );
        assert_eq!(
            bytes(
                MouseEventKind::Down(MouseButton::Forward),
                MouseProtocolMode::PressRelease,
                SGR
            )
            .unwrap(),
            b"\x1b[<129;3;2M"
        );
        assert_eq!(
            bytes(
                MouseEventKind::Up(MouseButton::Forward),
                MouseProtocolMode::PressRelease,
                SGR
            )
            .unwrap(),
            b"\x1b[<129;3;2m"
        );
        assert_eq!(
            bytes(
                MouseEventKind::Drag(MouseButton::Back),
                MouseProtocolMode::ButtonMotion,
                SGR
            )
            .unwrap(),
            b"\x1b[<160;3;2M"
        );
        assert_eq!(
            bytes(
                MouseEventKind::Drag(MouseButton::Forward),
                MouseProtocolMode::ButtonMotion,
                SGR
            )
            .unwrap(),
            b"\x1b[<161;3;2M"
        );
    }

    #[test]
    fn sgr_motion_and_wheel() {
        let drag = MouseEventKind::Drag(MouseButton::Left);
        assert_eq!(
            bytes(drag, MouseProtocolMode::ButtonMotion, SGR).unwrap(),
            b"\x1b[<32;3;2M"
        );
        assert_eq!(
            bytes(drag, MouseProtocolMode::AnyMotion, SGR).unwrap(),
            b"\x1b[<32;3;2M"
        );
        assert_eq!(
            bytes(drag, MouseProtocolMode::PressRelease, SGR),
            None,
            "1000 has no motion"
        );
        assert_eq!(
            bytes(MouseEventKind::Moved, MouseProtocolMode::AnyMotion, SGR).unwrap(),
            b"\x1b[<35;3;2M"
        );
        assert_eq!(bytes(MouseEventKind::Moved, MouseProtocolMode::ButtonMotion, SGR), None);
        assert_eq!(
            bytes(MouseEventKind::ScrollUp, MouseProtocolMode::PressRelease, SGR).unwrap(),
            b"\x1b[<64;3;2M"
        );
        assert_eq!(
            bytes(MouseEventKind::ScrollDown, MouseProtocolMode::Press, SGR).unwrap(),
            b"\x1b[<65;3;2M"
        );
        assert_eq!(
            bytes(MouseEventKind::ScrollLeft, MouseProtocolMode::PressRelease, SGR).unwrap(),
            b"\x1b[<66;3;2M"
        );
        assert_eq!(
            bytes(MouseEventKind::ScrollRight, MouseProtocolMode::PressRelease, SGR).unwrap(),
            b"\x1b[<67;3;2M"
        );
    }

    #[test]
    fn sgr_modifier_bits() {
        let press = MouseEventKind::Down(MouseButton::Left);
        let at = |mods| encode(&event(press, 2, 1, mods), MouseProtocolMode::PressRelease, SGR).unwrap();
        assert_eq!(at(KeyModifiers::SHIFT), b"\x1b[<4;3;2M");
        assert_eq!(at(KeyModifiers::ALT), b"\x1b[<8;3;2M");
        assert_eq!(at(KeyModifiers::CONTROL), b"\x1b[<16;3;2M");
        assert_eq!(
            at(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL),
            b"\x1b[<28;3;2M"
        );
        // Also on the wheel and motion.
        let wheel = encode(
            &event(MouseEventKind::ScrollUp, 2, 1, KeyModifiers::CONTROL),
            MouseProtocolMode::PressRelease,
            SGR,
        )
        .unwrap();
        assert_eq!(wheel, b"\x1b[<80;3;2M");
    }

    #[test]
    fn x10_bytes() {
        let press = MouseEventKind::Down(MouseButton::Left);
        let release = MouseEventKind::Up(MouseButton::Left);
        // Cell (1, 1) and button 0: three more added 32s.
        assert_eq!(
            encode(
                &event(press, 0, 0, KeyModifiers::NONE),
                MouseProtocolMode::PressRelease,
                X10
            )
            .unwrap(),
            b"\x1b[M !!"
        );
        // A release is code 3, still at its cell.
        assert_eq!(
            encode(
                &event(release, 0, 0, KeyModifiers::NONE),
                MouseProtocolMode::PressRelease,
                X10
            )
            .unwrap(),
            b"\x1b[M#!!"
        );
        assert_eq!(bytes(press, MouseProtocolMode::Press, X10).unwrap(), b"\x1b[M #\"");
        assert_eq!(
            bytes(release, MouseProtocolMode::Press, X10),
            None,
            "no release in mode 9"
        );
        let at_1_1 = |kind| {
            encode(
                &event(kind, 0, 0, KeyModifiers::NONE),
                MouseProtocolMode::ButtonMotion,
                X10,
            )
        };
        assert_eq!(at_1_1(MouseEventKind::Drag(MouseButton::Left)).unwrap(), b"\x1b[M@!!");
        assert_eq!(
            encode(
                &event(MouseEventKind::Moved, 0, 0, KeyModifiers::NONE),
                MouseProtocolMode::AnyMotion,
                X10
            )
            .unwrap(),
            b"\x1b[MC!!"
        );
        assert_eq!(
            encode(
                &event(MouseEventKind::ScrollUp, 0, 0, KeyModifiers::NONE),
                MouseProtocolMode::PressRelease,
                X10
            )
            .unwrap(),
            b"\x1b[M`!!"
        );
        assert_eq!(
            encode(
                &event(MouseEventKind::ScrollDown, 0, 0, KeyModifiers::NONE),
                MouseProtocolMode::PressRelease,
                X10
            )
            .unwrap(),
            b"\x1b[Ma!!"
        );
        // A release keeps the modifier bit: 3 + 4, plus X10's 32.
        assert_eq!(
            encode(
                &event(release, 0, 0, KeyModifiers::SHIFT),
                MouseProtocolMode::PressRelease,
                X10
            )
            .unwrap(),
            b"\x1b[M'!!"
        );
        // SGR says the button it released, with its modifiers.
        assert_eq!(
            encode(
                &event(release, 0, 0, KeyModifiers::SHIFT),
                MouseProtocolMode::PressRelease,
                SGR
            )
            .unwrap(),
            b"\x1b[<4;1;1m"
        );
        // UTF-8 mode is encoded as X10.
        assert_eq!(
            encode(
                &event(press, 0, 0, KeyModifiers::NONE),
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Utf8
            )
            .unwrap(),
            b"\x1b[M !!"
        );
        assert_eq!(bytes(press, MouseProtocolMode::None, X10), None);
        // Past X10's largest cell: clamped.
        let far = event(press, 500, 500, KeyModifiers::NONE);
        let bytes = encode(&far, MouseProtocolMode::PressRelease, X10).unwrap();
        assert_eq!(&bytes[3..], [32, 255, 255]);
    }

    #[test]
    fn sgr_coordinates_are_not_clamped() {
        // SGR spells the cell in decimal digits, so any size passes through; only X10's single
        // byte is clamped to 223.
        let far = encode(
            &event(MouseEventKind::Down(MouseButton::Left), 500, 500, KeyModifiers::NONE),
            MouseProtocolMode::PressRelease,
            SGR,
        )
        .unwrap();
        assert_eq!(far, b"\x1b[<0;501;501M");
    }
}
