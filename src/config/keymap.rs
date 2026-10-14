//! Key combos to commands. Keymaps are data; commands never look at keys.
//! A binding applies in one context (editor, explorer) or everywhere.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::{Command, ExplorerCommand as X, Motion};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Editor,
    Explorer,
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
    /// `None` context = everywhere.
    bindings: HashMap<(KeyCombo, Option<Context>), Command>,
}

impl Keymap {
    /// Every binding: combo, context (`None` = everywhere) and command.
    #[cfg(test)]
    pub fn bindings(&self) -> impl Iterator<Item = (KeyCombo, Option<Context>, Command)> + '_ {
        self.bindings.iter().map(|((combo, ctx), c)| (*combo, *ctx, *c))
    }

    /// A binding for the current context wins over a global one.
    pub fn lookup(&self, ev: &KeyEvent, context: Context) -> Option<Command> {
        let combo = KeyCombo::from_event(ev);
        self.bindings
            .get(&(combo, Some(context)))
            .or_else(|| self.bindings.get(&(combo, None)))
            .copied()
    }

    pub fn default_for(platform: Platform) -> Self {
        let none = KeyModifiers::NONE;
        let shift = KeyModifiers::SHIFT;
        let ctrl = KeyModifiers::CONTROL;
        let alt = KeyModifiers::ALT;
        let mac = platform == Platform::MacOs;
        // Cmd on macOS, Ctrl on Linux.
        let cmd = if mac {
            KeyModifiers::SUPER
        } else {
            KeyModifiers::CONTROL
        };
        // Word-wise movement: Alt on macOS, Ctrl on Linux.
        let word = if mac { KeyModifiers::ALT } else { KeyModifiers::CONTROL };

        let mut editor: Vec<(KeyCode, KeyModifiers, Command)> = Vec::new();
        let mut motion = |code: KeyCode, mods: KeyModifiers, m: Motion| {
            editor.push((
                code,
                mods,
                Command::Move {
                    motion: m,
                    select: false,
                },
            ));
            editor.push((
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
        if mac {
            motion(KeyCode::Left, cmd, Motion::LineStart);
            motion(KeyCode::Right, cmd, Motion::LineEnd);
            motion(KeyCode::Up, cmd, Motion::DocStart);
            motion(KeyCode::Down, cmd, Motion::DocEnd);
        } else {
            motion(KeyCode::Home, ctrl, Motion::DocStart);
            motion(KeyCode::End, ctrl, Motion::DocEnd);
        }
        editor.extend([
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
            (KeyCode::Char('c'), cmd, Command::Copy),
            (KeyCode::Char('x'), cmd, Command::Cut),
        ]);
        if mac {
            editor.push((KeyCode::Backspace, cmd, Command::DeleteToLineStart));
        } else {
            editor.push((KeyCode::Char('y'), ctrl, Command::Redo));
            // macOS leaves Cmd+V to the terminal: it arrives as a bracketed paste.
            editor.push((KeyCode::Char('v'), ctrl, Command::Paste));
        }

        let mut explorer = vec![
            (KeyCode::Up, none, X::Up),
            (KeyCode::Down, none, X::Down),
            (KeyCode::PageUp, none, X::PageUp),
            (KeyCode::PageDown, none, X::PageDown),
            (KeyCode::Home, none, X::First),
            (KeyCode::End, none, X::Last),
            (KeyCode::Left, none, X::Left),
            (KeyCode::Right, none, X::Right),
            (KeyCode::Enter, none, X::Open),
            (KeyCode::Char(' '), none, X::Preview),
            (KeyCode::F(2), none, X::Rename),
            (KeyCode::Delete, none, X::Delete),
            (KeyCode::Backspace, cmd, X::Delete),
            (KeyCode::Char('n'), cmd | alt, X::NewFile),
            (KeyCode::Char('n'), cmd | alt | shift, X::NewFolder),
        ];
        if mac {
            explorer.push((KeyCode::Down, cmd, X::Open));
        }

        let mut global = vec![
            (KeyCode::Char('s'), cmd, Command::Save),
            (KeyCode::Char('q'), cmd, Command::Quit),
            // Works in any terminal, even without the kitty protocol.
            (KeyCode::Char('q'), ctrl, Command::Quit),
            (KeyCode::Char('w'), cmd, Command::CloseTab),
            (KeyCode::Char('b'), cmd, Command::ToggleSidebar),
            (KeyCode::Char('e'), cmd | shift, Command::FocusExplorer),
            (KeyCode::Char('1'), cmd, Command::FocusGroup(1)),
            (KeyCode::Char('2'), cmd, Command::FocusGroup(2)),
            (KeyCode::Char('3'), cmd, Command::FocusGroup(3)),
            (KeyCode::Char('\\'), cmd, Command::SplitRight),
            // No chords, so not VS Code's Cmd+K Cmd+\\.
            (KeyCode::Char('\\'), cmd | alt, Command::SplitDown),
            (KeyCode::PageDown, ctrl, Command::NextTab),
            (KeyCode::PageUp, ctrl, Command::PrevTab),
        ];
        if mac {
            global.push((KeyCode::Right, cmd | alt, Command::NextTab));
            global.push((KeyCode::Left, cmd | alt, Command::PrevTab));
        }

        let mut bindings = HashMap::new();
        let mut add = |code, mods, ctx, c| {
            bindings.insert((KeyCombo { code, mods }, ctx), c);
        };
        for (code, mods, c) in editor {
            add(code, mods, Some(Context::Editor), c);
        }
        for (code, mods, c) in explorer {
            add(code, mods, Some(Context::Explorer), Command::Explorer(c));
        }
        for (code, mods, c) in global {
            add(code, mods, None, c);
        }
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
        let ed = Context::Editor;
        assert_eq!(km.lookup(&key(KeyCode::Char('Z'), KeyModifiers::SUPER), ed), redo);
        assert_eq!(
            km.lookup(&key(KeyCode::Char('Z'), KeyModifiers::SUPER | KeyModifiers::SHIFT), ed),
            redo
        );
        assert_eq!(
            km.lookup(&key(KeyCode::Char('z'), KeyModifiers::SUPER | KeyModifiers::SHIFT), ed),
            redo
        );
    }

    #[test]
    fn platform_modifiers() {
        let mac = Keymap::default_for(Platform::MacOs);
        let linux = Keymap::default_for(Platform::Linux);
        let ed = Context::Editor;
        assert_eq!(
            mac.lookup(&key(KeyCode::Char('s'), KeyModifiers::SUPER), ed),
            Some(Command::Save)
        );
        assert_eq!(mac.lookup(&key(KeyCode::Char('s'), KeyModifiers::CONTROL), ed), None);
        assert_eq!(
            linux.lookup(&key(KeyCode::Char('s'), KeyModifiers::CONTROL), ed),
            Some(Command::Save)
        );
    }

    #[test]
    fn context_bindings_win_over_global_and_stay_in_context() {
        let km = Keymap::default_for(Platform::MacOs);
        let down = key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(km.lookup(&down, Context::Explorer), Some(Command::Explorer(X::Down)));
        assert_eq!(
            km.lookup(&down, Context::Editor),
            Some(Command::Move {
                motion: Motion::Down,
                select: false
            })
        );
        let save = key(KeyCode::Char('s'), KeyModifiers::SUPER);
        assert_eq!(km.lookup(&save, Context::Explorer), Some(Command::Save));
    }

    /// Canonical spelling used in docs/terminals/README.md: modifiers in the order
    /// cmd, ctrl, alt, shift; lowercase key names; `\` is `backslash`.
    fn canonical(combo: &KeyCombo) -> String {
        let mut parts = Vec::new();
        for (m, name) in [
            (KeyModifiers::SUPER, "cmd"),
            (KeyModifiers::CONTROL, "ctrl"),
            (KeyModifiers::ALT, "alt"),
            (KeyModifiers::SHIFT, "shift"),
        ] {
            if combo.mods.contains(m) {
                parts.push(name.to_string());
            }
        }
        parts.push(match combo.code {
            KeyCode::Char('\\') => "backslash".into(),
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_lowercase(),
        });
        parts.join("+")
    }

    /// The terminal docs must list exactly the macOS Cmd bindings, so the
    /// per-terminal snippets can't silently fall behind the keymap.
    #[test]
    fn terminal_docs_list_every_cmd_binding() {
        let readme = include_str!("../../docs/terminals/README.md");
        let documented: std::collections::BTreeSet<String> = readme
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .filter(|k| k.starts_with("cmd+"))
            .map(str::to_string)
            .collect();
        let bound: std::collections::BTreeSet<String> = Keymap::default_for(Platform::MacOs)
            .bindings()
            .filter(|(combo, ..)| combo.mods.contains(KeyModifiers::SUPER))
            .map(|(combo, ..)| canonical(&combo))
            .collect();
        let missing: Vec<_> = bound.difference(&documented).collect();
        let stale: Vec<_> = documented.difference(&bound).collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "docs/terminals/README.md is out of date (update it and every snippet).\nmissing: {missing:?}\nstale: {stale:?}"
        );
    }
}
