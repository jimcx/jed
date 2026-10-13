//! Key combos to commands. Keymaps are data; commands never look at keys.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::{Command, Motion};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyCombo {
    /// Uppercase letters become lowercase + Shift, so `Cmd+Shift+Z` matches however the terminal reports it.
    pub fn from_event(ev: &KeyEvent) -> Self {
        let mut mods =
            ev.modifiers & (KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let code = match ev.code {
            KeyCode::Char(c) if c.is_uppercase() => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Char(c.to_lowercase().next().unwrap_or(c))
            }
            KeyCode::BackTab => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            c => c,
        };
        Self { code, mods }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

pub struct Keymap {
    bindings: HashMap<KeyCombo, Command>,
}

impl Keymap {
    pub fn lookup(&self, ev: &KeyEvent) -> Option<Command> {
        self.bindings.get(&KeyCombo::from_event(ev)).copied()
    }

    pub fn default_for(platform: Platform) -> Self {
        let none = KeyModifiers::NONE;
        let shift = KeyModifiers::SHIFT;
        let ctrl = KeyModifiers::CONTROL;
        // Cmd on macOS, Ctrl on Linux.
        let cmd = match platform {
            Platform::MacOs => KeyModifiers::SUPER,
            Platform::Linux => KeyModifiers::CONTROL,
        };
        // Word-wise movement: Alt on macOS, Ctrl on Linux.
        let word = match platform {
            Platform::MacOs => KeyModifiers::ALT,
            Platform::Linux => KeyModifiers::CONTROL,
        };

        let mut b: Vec<(KeyCode, KeyModifiers, Command)> = Vec::new();
        let mut motion = |code: KeyCode, mods: KeyModifiers, m: Motion| {
            b.push((
                code,
                mods,
                Command::Move {
                    motion: m,
                    select: false,
                },
            ));
            b.push((
                code,
                mods | shift,
                Command::Move {
                    motion: m,
                    select: true,
                },
            ));
        };
        motion(KeyCode::Left, none, Motion::Left);
        motion(KeyCode::Right, none, Motion::Right);
        motion(KeyCode::Up, none, Motion::Up);
        motion(KeyCode::Down, none, Motion::Down);
        motion(KeyCode::Left, word, Motion::WordLeft);
        motion(KeyCode::Right, word, Motion::WordRight);
        motion(KeyCode::Home, none, Motion::LineStart);
        motion(KeyCode::End, none, Motion::LineEnd);
        motion(KeyCode::PageUp, none, Motion::PageUp);
        motion(KeyCode::PageDown, none, Motion::PageDown);
        match platform {
            Platform::MacOs => {
                motion(KeyCode::Left, cmd, Motion::LineStart);
                motion(KeyCode::Right, cmd, Motion::LineEnd);
                motion(KeyCode::Up, cmd, Motion::DocStart);
                motion(KeyCode::Down, cmd, Motion::DocEnd);
            }
            Platform::Linux => {
                motion(KeyCode::Home, ctrl, Motion::DocStart);
                motion(KeyCode::End, ctrl, Motion::DocEnd);
            }
        }

        b.extend([
            (KeyCode::Backspace, none, Command::DeleteLeft),
            (KeyCode::Backspace, shift, Command::DeleteLeft),
            (KeyCode::Delete, none, Command::DeleteRight),
            (KeyCode::Backspace, word, Command::DeleteWordLeft),
            (KeyCode::Delete, word, Command::DeleteWordRight),
            (KeyCode::Enter, none, Command::InsertNewline),
            (KeyCode::Tab, none, Command::InsertTab),
            (KeyCode::Esc, none, Command::CancelSelection),
            (KeyCode::Char('a'), cmd, Command::SelectAll),
            (KeyCode::Char('z'), cmd, Command::Undo),
            (KeyCode::Char('z'), cmd | shift, Command::Redo),
            (KeyCode::Char('s'), cmd, Command::Save),
            (KeyCode::Char('q'), cmd, Command::Quit),
            // Works in any terminal, even without the kitty protocol.
            (KeyCode::Char('q'), ctrl, Command::Quit),
        ]);
        match platform {
            Platform::MacOs => b.push((KeyCode::Backspace, cmd, Command::DeleteToLineStart)),
            Platform::Linux => b.push((KeyCode::Char('y'), ctrl, Command::Redo)),
        }

        let bindings = b
            .into_iter()
            .map(|(code, mods, c)| (KeyCombo { code, mods }, c))
            .collect();
        Self { bindings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn shifted_letters_normalize() {
        let km = Keymap::default_for(Platform::MacOs);
        let redo = Some(Command::Redo);
        assert_eq!(km.lookup(&key(KeyCode::Char('Z'), KeyModifiers::SUPER)), redo);
        assert_eq!(
            km.lookup(&key(KeyCode::Char('Z'), KeyModifiers::SUPER | KeyModifiers::SHIFT)),
            redo
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('z'), KeyModifiers::SUPER | KeyModifiers::SHIFT)),
            redo
        );
    }

    #[test]
    fn platform_modifiers() {
        let mac = Keymap::default_for(Platform::MacOs);
        let linux = Keymap::default_for(Platform::Linux);
        assert_eq!(
            mac.lookup(&key(KeyCode::Char('s'), KeyModifiers::SUPER)),
            Some(Command::Save)
        );
        assert_eq!(mac.lookup(&key(KeyCode::Char('s'), KeyModifiers::CONTROL)), None);
        assert_eq!(
            linux.lookup(&key(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Some(Command::Save)
        );
    }
}
