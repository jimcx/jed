//! jmux's commands and key contexts. Commands never look at keys: `keymap` binds keys to them,
//! `App::execute` runs them. `Command` stays `Copy + Hash` (a keymap key).

use crossterm::event::{KeyCode, KeyModifiers};
use jtui::keymap::{KeyCombo, KeyCommand, KeyContext};

use crate::group::Side;

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
    /// Splits the active group side by side (a new terminal in the new group, which takes focus).
    SplitRight,
    /// Splits the active group one above the other.
    SplitDown,
    /// The group nearest the active one on that side.
    FocusSide(Side),
    FocusNextGroup,
    FocusPreviousGroup,
    /// The Nth group in tree order, 1 to 8.
    FocusGroup(u8),
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
            Command::SplitRight,
            Command::SplitDown,
            Command::FocusNextGroup,
            Command::FocusPreviousGroup,
            Command::FocusSide(Side::Left),
            Command::FocusSide(Side::Right),
            Command::FocusSide(Side::Up),
            Command::FocusSide(Side::Down),
            Command::Detach,
            Command::ConfirmYes,
            Command::ConfirmNo,
        ];
        all.extend((1..=9).map(Command::GoToTab));
        all.extend((1..=8).map(Command::FocusGroup));
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
            Command::SplitRight => "terminal.splitRight".into(),
            Command::SplitDown => "terminal.splitDown".into(),
            Command::FocusSide(Side::Left) => "terminal.focusLeftGroup".into(),
            Command::FocusSide(Side::Right) => "terminal.focusRightGroup".into(),
            Command::FocusSide(Side::Up) => "terminal.focusAboveGroup".into(),
            Command::FocusSide(Side::Down) => "terminal.focusBelowGroup".into(),
            Command::FocusNextGroup => "terminal.focusNextGroup".into(),
            Command::FocusPreviousGroup => "terminal.focusPreviousGroup".into(),
            Command::FocusGroup(n) => format!("terminal.focusGroup{n}"),
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
            Command::SplitRight => "Split Terminal Right".into(),
            Command::SplitDown => "Split Terminal Down".into(),
            Command::FocusSide(Side::Left) => "Focus Group Left".into(),
            Command::FocusSide(Side::Right) => "Focus Group Right".into(),
            Command::FocusSide(Side::Up) => "Focus Group Above".into(),
            Command::FocusSide(Side::Down) => "Focus Group Below".into(),
            Command::FocusNextGroup => "Focus Next Group".into(),
            Command::FocusPreviousGroup => "Focus Previous Group".into(),
            Command::FocusGroup(n) => format!("Focus Group {n}"),
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
