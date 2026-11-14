//! jmux's default keys, as data (`Keymap::from_list`); commands never look at keys. A user's
//! `keybindings.toml` goes over them (`load` at startup, `reload` on Reload Settings; jed's file format).

use crossterm::event::{KeyCode, KeyModifiers};
use jtui::keymap::{Binding, KeyCombo, Keymap, Platform, Source};

use crate::command::{Command, Context, SearchCommand as Sc, SideCommand as Sd};
use crate::group::Side;
use jtui::field::{FieldCommand as Fc, FieldMotion as Fm};

pub type JmuxKeymap = Keymap<Context, Command>;

/// The default bindings. Jed's terminal keys: Ctrl+` and Ctrl+Shift+T open a terminal, Ctrl+Shift+W
/// closes it (plain Ctrl+W is the shell's; Cmd+W on macOS), Ctrl+PageDown / PageUp cycle, Alt+1..9
/// go to a tab (Ctrl+1..9 on macOS). Detach, Ctrl+Shift+D, works everywhere; Ctrl+Shift+I installs the shell integration. The first binding of
/// a command is the one shown.
pub fn defaults(platform: Platform) -> Vec<Binding<Context, Command>> {
    let (ctrl, shift, alt, none) = (
        KeyModifiers::CONTROL,
        KeyModifiers::SHIFT,
        KeyModifiers::ALT,
        KeyModifiers::NONE,
    );
    let mac = platform == Platform::MacOs;
    // Cmd on macOS, Ctrl elsewhere (jed's `cmd`).
    let cmd = if mac { KeyModifiers::SUPER } else { ctrl };
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
        // Jed's split and group keys. Ctrl+Shift+, / . (terminals report `<` / `>`, normalised by
        // `KeyCombo::from_event`) step through the groups.
        term(KeyCode::Char('\\'), cmd, Command::SplitRight),
        term(KeyCode::Char('\\'), cmd | alt, Command::SplitDown),
        term(KeyCode::Char(','), ctrl | shift, Command::FocusPreviousGroup),
        term(KeyCode::Char('.'), ctrl | shift, Command::FocusNextGroup),
        // Jed's terminal scrollback keys. Copy is VS Code's Linux key (plain Ctrl+C stays SIGINT);
        // on macOS the outer terminal's own Cmd+C copies, and one that sends it on gets this.
        term(KeyCode::PageUp, shift, Command::PageUp),
        term(KeyCode::PageDown, shift, Command::PageDown),
        term(KeyCode::Up, ctrl | shift, Command::ScrollUp),
        term(KeyCode::Down, ctrl | shift, Command::ScrollDown),
        term(KeyCode::Home, ctrl, Command::ScrollTop),
        term(KeyCode::End, ctrl, Command::ScrollBottom),
        if mac {
            term(KeyCode::Char('c'), KeyModifiers::SUPER, Command::Copy)
        } else {
            term(KeyCode::Char('c'), ctrl | shift, Command::Copy)
        },
        // Jed's terminal find: Cmd+F on macOS, Ctrl+Shift+F elsewhere (Ctrl+F is the shell's).
        term(
            KeyCode::Char('f'),
            if mac { KeyModifiers::SUPER } else { ctrl | shift },
            Command::Search(Sc::Open),
        ),
        (KeyCode::Char('d'), ctrl | shift, Command::Detach, None),
        (KeyCode::Char('i'), ctrl | shift, Command::InstallShellIntegration, None),
        (KeyCode::Char('r'), ctrl | shift, Command::ReloadSettings, None),
    ];
    let side = if mac { KeyModifiers::SUPER | alt } else { alt | shift };
    for (code, s) in [
        (KeyCode::Left, Side::Left),
        (KeyCode::Right, Side::Right),
        (KeyCode::Up, Side::Up),
        (KeyCode::Down, Side::Down),
    ] {
        list.push(term(code, side, Command::FocusSide(s)));
    }
    for n in 1..=8u8 {
        list.push(term(KeyCode::Char(char::from(b'0' + n)), cmd, Command::FocusGroup(n)));
    }
    let go = if mac { ctrl } else { alt };
    for n in 1..=9u8 {
        list.push(term(KeyCode::Char(char::from(b'0' + n)), go, Command::GoToTab(n)));
    }
    // The search bar: Enter / F3 step (Shift back), Alt+C (Cmd+Alt+C) toggles case, Esc closes, and
    // the query field's editing keys (the editor's, on one line).
    let find = |code, mods, command| (code, mods, command, Some(Context::Search));
    let (word, line) = if mac { (alt, KeyModifiers::SUPER) } else { (ctrl, ctrl) };
    let field = |motion, select| Command::Field(Fc::Move { motion, select });
    list.extend([
        find(KeyCode::Enter, none, Command::Search(Sc::Next)),
        find(KeyCode::Enter, shift, Command::Search(Sc::Previous)),
        find(KeyCode::F(3), none, Command::Search(Sc::Next)),
        find(KeyCode::F(3), shift, Command::Search(Sc::Previous)),
        find(
            KeyCode::Char('c'),
            if mac { KeyModifiers::SUPER | alt } else { alt },
            Command::Search(Sc::ToggleCase),
        ),
        find(KeyCode::Esc, none, Command::Search(Sc::Close)),
        find(KeyCode::Left, none, field(Fm::Left, false)),
        find(KeyCode::Left, shift, field(Fm::Left, true)),
        find(KeyCode::Right, none, field(Fm::Right, false)),
        find(KeyCode::Right, shift, field(Fm::Right, true)),
        find(KeyCode::Left, word, field(Fm::WordLeft, false)),
        find(KeyCode::Left, word | shift, field(Fm::WordLeft, true)),
        find(KeyCode::Right, word, field(Fm::WordRight, false)),
        find(KeyCode::Right, word | shift, field(Fm::WordRight, true)),
        find(KeyCode::Home, none, field(Fm::Start, false)),
        find(KeyCode::Home, shift, field(Fm::Start, true)),
        find(KeyCode::End, none, field(Fm::End, false)),
        find(KeyCode::End, shift, field(Fm::End, true)),
        find(KeyCode::Backspace, none, Command::Field(Fc::DeleteLeft)),
        find(KeyCode::Backspace, shift, Command::Field(Fc::DeleteLeft)),
        find(KeyCode::Delete, none, Command::Field(Fc::DeleteRight)),
        find(KeyCode::Backspace, word, Command::Field(Fc::DeleteWordLeft)),
        find(KeyCode::Delete, word, Command::Field(Fc::DeleteWordRight)),
        find(
            KeyCode::Char('a'),
            if mac { KeyModifiers::SUPER } else { ctrl },
            Command::Field(Fc::SelectAll),
        ),
    ]);
    if mac {
        list.push(find(KeyCode::Backspace, line, Command::Field(Fc::DeleteToStart)));
        list.push(find(KeyCode::Left, line, field(Fm::Start, false)));
        list.push(find(KeyCode::Right, line, field(Fm::End, false)));
    } else {
        list.push(find(KeyCode::Char('u'), ctrl, Command::Field(Fc::DeleteToStart)));
    }
    // The commands side bar (VS Code's side bar toggle, Cmd+B; Ctrl+B is the shell's): Ctrl+Shift+B
    // shows / hides it, Ctrl+Shift+E (the explorer's key in VS Code) gives it the keys. Both work
    // while it has them. Its list keys are plain, and the filter box takes the field keys above
    // (Home / End move the list instead).
    let toggle = if mac {
        (KeyCode::Char('b'), KeyModifiers::SUPER)
    } else {
        (KeyCode::Char('b'), ctrl | shift)
    };
    let focus = if mac {
        (KeyCode::Char('e'), KeyModifiers::SUPER | shift)
    } else {
        (KeyCode::Char('e'), ctrl | shift)
    };
    list.push(term(toggle.0, toggle.1, Command::Side(Sd::Toggle)));
    list.push(term(focus.0, focus.1, Command::Side(Sd::Focus)));
    let side = |code, mods, command| (code, mods, command, Some(Context::Side));
    list.extend([
        side(KeyCode::Up, none, Command::Side(Sd::Up)),
        side(KeyCode::Down, none, Command::Side(Sd::Down)),
        side(KeyCode::PageUp, none, Command::Side(Sd::PageUp)),
        side(KeyCode::PageDown, none, Command::Side(Sd::PageDown)),
        side(KeyCode::Home, none, Command::Side(Sd::First)),
        side(KeyCode::End, none, Command::Side(Sd::Last)),
        side(KeyCode::Enter, none, Command::Side(Sd::Open)),
        side(KeyCode::Esc, none, Command::Side(Sd::Back)),
        side(KeyCode::Left, none, field(Fm::Left, false)),
        side(KeyCode::Left, shift, field(Fm::Left, true)),
        side(KeyCode::Right, none, field(Fm::Right, false)),
        side(KeyCode::Right, shift, field(Fm::Right, true)),
        side(KeyCode::Left, word, field(Fm::WordLeft, false)),
        side(KeyCode::Right, word, field(Fm::WordRight, false)),
        side(KeyCode::Backspace, none, Command::Field(Fc::DeleteLeft)),
        side(KeyCode::Backspace, shift, Command::Field(Fc::DeleteLeft)),
        side(KeyCode::Delete, none, Command::Field(Fc::DeleteRight)),
        side(KeyCode::Backspace, word, Command::Field(Fc::DeleteWordLeft)),
        side(KeyCode::Delete, word, Command::Field(Fc::DeleteWordRight)),
        side(
            KeyCode::Char('a'),
            if mac { KeyModifiers::SUPER } else { ctrl },
            Command::Field(Fc::SelectAll),
        ),
    ]);
    if mac {
        list.push(side(KeyCode::Backspace, line, Command::Field(Fc::DeleteToStart)));
    } else {
        list.push(side(KeyCode::Char('u'), ctrl, Command::Field(Fc::DeleteToStart)));
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

/// The user's file name, in the config directory.
pub const FILE_NAME: &str = "keybindings.toml";

/// Blocking: the defaults with the file at `path` over them, and one message per problem (bad
/// entries are skipped, the rest applies).
pub fn load(path: &std::path::Path) -> (JmuxKeymap, Vec<String>) {
    Keymap::load(path, FILE_NAME, defaults(Platform::current()))
}

/// Blocking: like `load`, but a file that can't be read or parsed at all is an `Err` (the caller
/// keeps its keymap).
pub fn reload(path: &std::path::Path) -> Result<(JmuxKeymap, Vec<String>), String> {
    Keymap::reload(path, FILE_NAME, defaults(Platform::current()))
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
    fn split_and_group_keys_resolve_per_platform() {
        let ev = |code, mods| KeyEvent::new(code, mods);
        let (ctrl, shift, alt) = (KeyModifiers::CONTROL, KeyModifiers::SHIFT, KeyModifiers::ALT);
        let lin = Keymap::from_list(defaults(Platform::Linux));
        let look = |m: &JmuxKeymap, code, mods| m.lookup(&ev(code, mods), Context::Terminal);
        assert_eq!(look(&lin, KeyCode::Char('\\'), ctrl), Some(Command::SplitRight));
        assert_eq!(look(&lin, KeyCode::Char('\\'), ctrl | alt), Some(Command::SplitDown));
        assert_eq!(
            look(&lin, KeyCode::Char('.'), ctrl | shift),
            Some(Command::FocusNextGroup)
        );
        assert_eq!(
            look(&lin, KeyCode::Char('>'), ctrl | shift),
            Some(Command::FocusNextGroup)
        );
        assert_eq!(
            look(&lin, KeyCode::Char('<'), ctrl | shift),
            Some(Command::FocusPreviousGroup)
        );
        assert_eq!(look(&lin, KeyCode::Char('2'), ctrl), Some(Command::FocusGroup(2)));
        assert_eq!(look(&lin, KeyCode::Char('2'), alt), Some(Command::GoToTab(2)));
        assert_eq!(
            look(&lin, KeyCode::Left, alt | shift),
            Some(Command::FocusSide(Side::Left))
        );
        let mac = Keymap::from_list(defaults(Platform::MacOs));
        let (sup, ctl) = (KeyModifiers::SUPER, KeyModifiers::CONTROL);
        assert_eq!(look(&mac, KeyCode::Char('\\'), sup), Some(Command::SplitRight));
        assert_eq!(look(&mac, KeyCode::Char('2'), sup), Some(Command::FocusGroup(2)));
        assert_eq!(look(&mac, KeyCode::Char('2'), ctl), Some(Command::GoToTab(2)));
        assert_eq!(
            look(&mac, KeyCode::Down, sup | alt),
            Some(Command::FocusSide(Side::Down))
        );
    }

    #[test]
    fn scrollback_and_copy_keys_follow_jeds_terminal_keys() {
        let ev = |code, mods| KeyEvent::new(code, mods);
        let (ctrl, shift) = (KeyModifiers::CONTROL, KeyModifiers::SHIFT);
        let lin = Keymap::from_list(defaults(Platform::Linux));
        let look = |m: &JmuxKeymap, code, mods| m.lookup(&ev(code, mods), Context::Terminal);
        assert_eq!(look(&lin, KeyCode::PageUp, shift), Some(Command::PageUp));
        assert_eq!(look(&lin, KeyCode::PageDown, shift), Some(Command::PageDown));
        assert_eq!(look(&lin, KeyCode::Up, ctrl | shift), Some(Command::ScrollUp));
        assert_eq!(look(&lin, KeyCode::Down, ctrl | shift), Some(Command::ScrollDown));
        assert_eq!(look(&lin, KeyCode::Home, ctrl), Some(Command::ScrollTop));
        assert_eq!(look(&lin, KeyCode::End, ctrl), Some(Command::ScrollBottom));
        assert_eq!(look(&lin, KeyCode::Char('C'), ctrl | shift), Some(Command::Copy));
        // Plain Ctrl+C stays the program's (SIGINT), selection or not.
        assert_eq!(look(&lin, KeyCode::Char('c'), ctrl), None);
        let mac = Keymap::from_list(defaults(Platform::MacOs));
        assert_eq!(look(&mac, KeyCode::Char('c'), KeyModifiers::SUPER), Some(Command::Copy));
        assert_eq!(look(&mac, KeyCode::Char('c'), ctrl | shift), None);
    }

    #[test]
    fn find_keys_open_the_bar_and_edit_its_query() {
        let ev = |code, mods| KeyEvent::new(code, mods);
        let (ctrl, shift) = (KeyModifiers::CONTROL, KeyModifiers::SHIFT);
        let lin = Keymap::from_list(defaults(Platform::Linux));
        let look = |m: &JmuxKeymap, c, code, mods| m.lookup(&ev(code, mods), c);
        let open = Some(Command::Search(Sc::Open));
        assert_eq!(look(&lin, Context::Terminal, KeyCode::Char('F'), ctrl | shift), open);
        assert_eq!(look(&lin, Context::Search, KeyCode::Char('F'), ctrl | shift), open);
        // Plain Ctrl+F stays the shell's.
        assert_eq!(look(&lin, Context::Terminal, KeyCode::Char('f'), ctrl), None);
        assert_eq!(
            look(&lin, Context::Search, KeyCode::Enter, shift),
            Some(Command::Search(Sc::Previous))
        );
        assert_eq!(
            look(&lin, Context::Search, KeyCode::Esc, KeyModifiers::NONE),
            Some(Command::Search(Sc::Close))
        );
        assert_eq!(
            look(&lin, Context::Search, KeyCode::Backspace, ctrl),
            Some(Command::Field(Fc::DeleteWordLeft))
        );
        // The terminal's bindings are not the bar's: Esc, Enter and the scrollback keys differ.
        assert_eq!(look(&lin, Context::Terminal, KeyCode::Enter, KeyModifiers::NONE), None);
        let mac = Keymap::from_list(defaults(Platform::MacOs));
        assert_eq!(
            look(&mac, Context::Terminal, KeyCode::Char('f'), KeyModifiers::SUPER),
            open
        );
    }

    #[test]
    fn side_bar_keys_resolve_in_their_contexts() {
        use crate::command::SideCommand as Sd;
        let ev = |code, mods| KeyEvent::new(code, mods);
        let (ctrl, shift, none) = (KeyModifiers::CONTROL, KeyModifiers::SHIFT, KeyModifiers::NONE);
        let lin = Keymap::from_list(defaults(Platform::Linux));
        let look = |m: &JmuxKeymap, c, code, mods| m.lookup(&ev(code, mods), c);
        assert_eq!(
            look(&lin, Context::Terminal, KeyCode::Char('B'), ctrl | shift),
            Some(Command::Side(Sd::Toggle))
        );
        assert_eq!(
            look(&lin, Context::Side, KeyCode::Char('E'), ctrl | shift),
            Some(Command::Side(Sd::Focus))
        );
        assert_eq!(look(&lin, Context::Terminal, KeyCode::Down, none), None);
        assert_eq!(
            look(&lin, Context::Side, KeyCode::Down, none),
            Some(Command::Side(Sd::Down))
        );
        assert_eq!(
            look(&lin, Context::Side, KeyCode::Enter, none),
            Some(Command::Side(Sd::Open))
        );
        assert_eq!(
            look(&lin, Context::Side, KeyCode::Esc, none),
            Some(Command::Side(Sd::Back))
        );
        assert_eq!(
            look(&lin, Context::Side, KeyCode::Home, none),
            Some(Command::Side(Sd::First))
        );
        let mac = Keymap::from_list(defaults(Platform::MacOs));
        assert_eq!(
            look(&mac, Context::Terminal, KeyCode::Char('b'), KeyModifiers::SUPER),
            Some(Command::Side(Sd::Toggle))
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

    fn user(text: &str) -> Result<(JmuxKeymap, Vec<String>), String> {
        Keymap::from_text(FILE_NAME, defaults(Platform::Linux), text)
    }

    #[test]
    fn a_user_binding_replaces_the_commands_defaults_and_bad_ones_are_reported() {
        let text = "[[binding]]\nkey = \"ctrl+alt+k\"\ncommand = \"terminal.splitRight\"\n\n\
                    [[binding]]\nkey = \"ctrl+alt+j\"\ncommand = \"noSuchCommand\"\n";
        let (map, errors) = user(text).unwrap();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("keybindings.toml"), "{errors:?}");
        let ev = |code, mods| KeyEvent::new(code, mods);
        let k = ev(KeyCode::Char('k'), KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert_eq!(map.lookup(&k, Context::Terminal), Some(Command::SplitRight));
        let old = ev(KeyCode::Char('\\'), KeyModifiers::CONTROL);
        assert_eq!(map.lookup(&old, Context::Terminal), None);
    }

    #[test]
    fn a_missing_file_gives_the_defaults_and_a_broken_one_is_an_error_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (map, errors) = load(&path);
        assert!(errors.is_empty() && map.binding_for(Command::Detach).is_some());
        assert!(reload(&path).unwrap().1.is_empty());
        std::fs::write(&path, "[[binding\n").unwrap();
        assert!(reload(&path).is_err());
        assert_eq!(load(&path).1.len(), 1);
    }

    /// jmux adds no Cmd key of its own: every one is a row of jed's table in docs/terminals, whose
    /// snippets release it.
    #[test]
    fn jmux_every_cmd_binding_is_in_the_terminal_docs() {
        let readme = include_str!("../../docs/terminals/README.md");
        let documented: HashSet<String> = readme
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .filter(|k| k.starts_with("cmd+"))
            .map(str::to_string)
            .collect();
        let missing: Vec<String> = defaults(Platform::MacOs)
            .iter()
            .filter(|b| b.combo.mods.contains(KeyModifiers::SUPER))
            .map(|b| b.combo.spec())
            .filter(|k| !documented.contains(k))
            .collect();
        assert!(
            missing.is_empty(),
            "docs/terminals/README.md lacks jmux's Cmd keys: {missing:?}"
        );
    }
}
