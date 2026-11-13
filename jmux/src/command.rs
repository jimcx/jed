//! jmux's commands and key contexts. Commands never look at keys: `keymap` binds keys to them,
//! `App::execute` runs them. `Command` stays `Copy + Hash` (a keymap key).

use crossterm::event::{KeyCode, KeyModifiers};
use jtui::keymap::{KeyCombo, KeyCommand, KeyContext};

/// Which part has the key focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// A terminal: unbound keys go to its shell.
    Terminal,
    /// The "close a running terminal?" question: unbound keys are swallowed.
    Confirm,
}

impl KeyContext for Context {
    fn parent(self) -> Option<Self> {
        None
    }
    fn parse(name: &str) -> Option<Self> {
        match name {
            "terminal" => Some(Context::Terminal),
            "confirm" => Some(Context::Confirm),
            _ => None,
        }
    }
    fn names() -> Vec<&'static str> {
        vec!["terminal", "confirm"]
    }
    /// Plain Ctrl+letter always reaches the shell (history search, delete word, ...).
    fn passes_through(self, combo: &KeyCombo) -> bool {
        self == Context::Terminal
            && matches!(combo.code, KeyCode::Char(c) if c.is_ascii_lowercase())
            && combo.mods == KeyModifiers::CONTROL
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// A new shell tab after the active one.
    NewTerminal,
    /// Hangs up the active tab's shell (asks first while a command runs).
    CloseTerminal,
    NextTab,
    PrevTab,
    /// The Nth tab of the active group, 1 to 9.
    GoToTab(u8),
    Detach,
    ConfirmYes,
    ConfirmNo,
}

impl Command {
    /// Every command, for the name lookup.
    pub fn all() -> Vec<Command> {
        let mut all = vec![
            Command::NewTerminal,
            Command::CloseTerminal,
            Command::NextTab,
            Command::PrevTab,
            Command::Detach,
            Command::ConfirmYes,
            Command::ConfirmNo,
        ];
        all.extend((1..=9).map(Command::GoToTab));
        all
    }
}

impl KeyCommand for Command {
    fn from_name(name: &str) -> Option<Self> {
        Command::all().into_iter().find(|c| c.name() == name)
    }
    fn name(&self) -> String {
        match self {
            Command::NewTerminal => "terminal.new".into(),
            Command::CloseTerminal => "terminal.close".into(),
            Command::NextTab => "terminal.nextTab".into(),
            Command::PrevTab => "terminal.previousTab".into(),
            Command::GoToTab(n) => format!("terminal.goToTab{n}"),
            Command::Detach => "session.detach".into(),
            Command::ConfirmYes => "confirm.yes".into(),
            Command::ConfirmNo => "confirm.no".into(),
        }
    }
    fn title(&self) -> String {
        match self {
            Command::NewTerminal => "New Terminal".into(),
            Command::CloseTerminal => "Close Terminal".into(),
            Command::NextTab => "Next Tab".into(),
            Command::PrevTab => "Previous Tab".into(),
            Command::GoToTab(n) => format!("Go to Tab {n}"),
            Command::Detach => "Detach".into(),
            Command::ConfirmYes => "Confirm".into(),
            Command::ConfirmNo => "Cancel".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_are_unique() {
        let all = Command::all();
        for c in &all {
            assert_eq!(Command::from_name(&c.name()), Some(*c), "{c:?}");
        }
        let mut names: Vec<String> = all.iter().map(Command::name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len());
        assert_eq!(Command::from_name("nope"), None);
    }

    #[test]
    fn contexts_parse_by_their_names() {
        assert_eq!(Context::parse("terminal"), Some(Context::Terminal));
        assert_eq!(Context::parse("confirm"), Some(Context::Confirm));
        assert_eq!(Context::parse("x"), None);
        assert_eq!(Context::names().len(), 2);
    }
}
