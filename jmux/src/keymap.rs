//! jmux's default keys, as data (`Keymap::from_list`); commands never look at keys. A user's
//! keybindings file is a later task (`jmux_config`).

use crossterm::event::{KeyCode, KeyModifiers};
use jtui::keymap::{Binding, KeyCombo, Keymap, Platform, Source};

use crate::command::{Command, Context};

pub type JmuxKeymap = Keymap<Context, Command>;

/// The default bindings. Jed's terminal keys: Ctrl+` and Ctrl+Shift+T open a terminal, Ctrl+Shift+W
/// closes it (plain Ctrl+W is the shell's; Cmd+W on macOS), Ctrl+PageDown / PageUp cycle, Alt+1..9
/// go to a tab (Ctrl+1..9 on macOS). Detach, Ctrl+Shift+D, works everywhere. The first binding of
/// a command is the one shown.
pub fn defaults(platform: Platform) -> Vec<Binding<Context, Command>> {
    let (ctrl, shift, alt, none) = (
        KeyModifiers::CONTROL,
        KeyModifiers::SHIFT,
        KeyModifiers::ALT,
        KeyModifiers::NONE,
    );
    let mac = platform == Platform::MacOs;
    let term = |code, mods, command| (code, mods, command, Some(Context::Terminal));
    let mut list = vec![
        term(KeyCode::Char('`'), ctrl, Command::NewTerminal),
        if mac {
            term(KeyCode::Char('t'), KeyModifiers::SUPER, Command::NewTerminal)
        } else {
            term(KeyCode::Char('t'), ctrl | shift, Command::NewTerminal)
        },
        if mac {
            term(KeyCode::Char('w'), KeyModifiers::SUPER, Command::CloseTerminal)
        } else {
            term(KeyCode::Char('w'), ctrl | shift, Command::CloseTerminal)
        },
        term(KeyCode::PageDown, ctrl, Command::NextTab),
        term(KeyCode::PageUp, ctrl, Command::PrevTab),
        (KeyCode::Char('d'), ctrl | shift, Command::Detach, None),
    ];
    let go = if mac { ctrl } else { alt };
    for n in 1..=9u8 {
        list.push(term(KeyCode::Char(char::from(b'0' + n)), go, Command::GoToTab(n)));
    }
    let confirm = |code, command| (code, none, command, Some(Context::Confirm));
    list.extend([
        confirm(KeyCode::Char('y'), Command::ConfirmYes),
        confirm(KeyCode::Enter, Command::ConfirmYes),
        confirm(KeyCode::Char('n'), Command::ConfirmNo),
        confirm(KeyCode::Esc, Command::ConfirmNo),
    ]);
    list.into_iter()
        .map(|(code, mods, command, context)| Binding {
            combo: KeyCombo { code, mods },
            context,
            command,
            source: Source::Default,
        })
        .collect()
}

pub fn keymap() -> JmuxKeymap {
    Keymap::from_list(defaults(Platform::current()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crossterm::event::KeyEvent;

    use super::*;

    const PLATFORMS: [Platform; 2] = [Platform::Linux, Platform::MacOs];

    #[test]
    fn every_command_has_a_default_key_and_none_is_bound_twice_in_a_context() {
        for platform in PLATFORMS {
            let list = defaults(platform);
            let mut seen = HashSet::new();
            for b in &list {
                assert!(seen.insert((b.combo, b.context)), "{platform:?}: {:?} twice", b.combo);
            }
            let map = Keymap::from_list(list);
            for c in Command::all() {
                assert!(map.binding_for(c).is_some(), "{platform:?}: {c:?} unbound");
            }
        }
    }

    #[test]
    fn keys_resolve_to_commands_in_their_context() {
        let map = Keymap::from_list(defaults(Platform::Linux));
        let ev = |code, mods| KeyEvent::new(code, mods);
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            map.lookup(&ev(KeyCode::Char('T'), cs), Context::Terminal),
            Some(Command::NewTerminal)
        );
        assert_eq!(
            map.lookup(&ev(KeyCode::Char('3'), KeyModifiers::ALT), Context::Terminal),
            Some(Command::GoToTab(3))
        );
        assert_eq!(
            map.lookup(&ev(KeyCode::Char('y'), KeyModifiers::NONE), Context::Confirm),
            Some(Command::ConfirmYes)
        );
        assert_eq!(
            map.lookup(&ev(KeyCode::Char('y'), KeyModifiers::NONE), Context::Terminal),
            None
        );
        // Plain Ctrl+D is the shell's (end of input); only Ctrl+Shift+D detaches.
        assert_eq!(
            map.lookup(&ev(KeyCode::Char('d'), KeyModifiers::CONTROL), Context::Terminal),
            None
        );
        // Detach works in both contexts.
        assert_eq!(
            map.lookup(&ev(KeyCode::Char('D'), cs), Context::Confirm),
            Some(Command::Detach)
        );
    }

    #[test]
    fn plain_ctrl_letters_reach_the_shell() {
        let map = Keymap::from_list(defaults(Platform::Linux));
        for c in 'a'..='z' {
            let ev = KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            assert_eq!(map.lookup(&ev, Context::Terminal), None, "Ctrl+{c}");
            // The confirm's y / n / Enter / Esc are plain keys: a Ctrl chord means nothing there.
            assert_eq!(map.lookup(&ev, Context::Confirm), None, "confirm Ctrl+{c}");
        }
    }
}
