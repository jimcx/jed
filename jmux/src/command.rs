//! jmux's commands and key contexts. Commands never look at keys: `keymap` binds keys to them,
//! `App::execute` runs them. `Command` stays `Copy + Hash` (a keymap key).

use crossterm::event::{KeyCode, KeyModifiers};
use jtui::field::{FieldCommand, FieldMotion};
use jtui::keymap::{KeyCombo, KeyCommand, KeyContext};

use crate::group::Side;

/// Which part has the key focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// A terminal: unbound keys go to its shell.
    Terminal,
    /// The "close a running terminal?" question: unbound keys are swallowed.
    Confirm,
    /// The search bar over a terminal (its parent is `Terminal`): typing edits the query, no key reaches the shell.
    Search,
    /// The commands side bar: the list's keys, typing edits the filter, no key reaches a shell.
    Side,
}

impl KeyContext for Context {
    /// The search bar and the side bar fall back to the terminal's keys (new tab, split,
    /// scrollback, ...): only the keys no binding takes are theirs, so none reaches the shell.
    fn parent(self) -> Option<Self> {
        matches!(self, Context::Search | Context::Side).then_some(Context::Terminal)
    }
    fn parse(name: &str) -> Option<Self> {
        match name {
            "terminal" => Some(Context::Terminal),
            "confirm" => Some(Context::Confirm),
            "search" => Some(Context::Search),
            "side" => Some(Context::Side),
            _ => None,
        }
    }
    fn names() -> Vec<&'static str> {
        vec!["terminal", "confirm", "search", "side"]
    }
    /// Plain Ctrl+letter always reaches the shell (history search, delete word, ...).
    fn passes_through(self, combo: &KeyCombo) -> bool {
        self == Context::Terminal
            && matches!(combo.code, KeyCode::Char(c) if c.is_ascii_lowercase())
            && combo.mods == KeyModifiers::CONTROL
    }
}

/// What the search bar does (`Open` is bound in the terminal context, the rest while it is open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchCommand {
    Open,
    Next,
    Previous,
    ToggleCase,
    Close,
}

impl SearchCommand {
    pub const ALL: [SearchCommand; 5] = [
        SearchCommand::Open,
        SearchCommand::Next,
        SearchCommand::Previous,
        SearchCommand::ToggleCase,
        SearchCommand::Close,
    ];
}

/// What the commands side bar does (`Toggle` and `Focus` are bound in the terminal context, the rest
/// while the bar has the keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SideCommand {
    /// Shows or hides the bar.
    Toggle,
    /// Shows the bar and gives it the keys.
    Focus,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Jumps to the selected command's terminal and output.
    Open,
    /// Gives the keys back to the terminal.
    Back,
}

impl SideCommand {
    pub const ALL: [SideCommand; 10] = [
        SideCommand::Toggle,
        SideCommand::Focus,
        SideCommand::Up,
        SideCommand::Down,
        SideCommand::PageUp,
        SideCommand::PageDown,
        SideCommand::First,
        SideCommand::Last,
        SideCommand::Open,
        SideCommand::Back,
    ];

    fn name(self) -> &'static str {
        match self {
            SideCommand::Toggle => "sideBar.toggle",
            SideCommand::Focus => "sideBar.focus",
            SideCommand::Up => "sideBar.up",
            SideCommand::Down => "sideBar.down",
            SideCommand::PageUp => "sideBar.pageUp",
            SideCommand::PageDown => "sideBar.pageDown",
            SideCommand::First => "sideBar.first",
            SideCommand::Last => "sideBar.last",
            SideCommand::Open => "sideBar.open",
            SideCommand::Back => "sideBar.back",
        }
    }

    fn title(self) -> &'static str {
        match self {
            SideCommand::Toggle => "Toggle Commands Side Bar",
            SideCommand::Focus => "Focus Commands Side Bar",
            SideCommand::Up => "Commands: Previous",
            SideCommand::Down => "Commands: Next",
            SideCommand::PageUp => "Commands: Page Up",
            SideCommand::PageDown => "Commands: Page Down",
            SideCommand::First => "Commands: First",
            SideCommand::Last => "Commands: Last",
            SideCommand::Open => "Commands: Go to Command",
            SideCommand::Back => "Commands: Back to Terminal",
        }
    }
}

/// Every edit of the query field the keymap can bind (the search bar's own editing keys).
fn field_commands() -> Vec<FieldCommand> {
    let mut all = Vec::new();
    for motion in [
        FieldMotion::Left,
        FieldMotion::Right,
        FieldMotion::WordLeft,
        FieldMotion::WordRight,
        FieldMotion::Start,
        FieldMotion::End,
    ] {
        all.push(FieldCommand::Move { motion, select: false });
        all.push(FieldCommand::Move { motion, select: true });
    }
    all.extend([
        FieldCommand::DeleteLeft,
        FieldCommand::DeleteRight,
        FieldCommand::DeleteWordLeft,
        FieldCommand::DeleteWordRight,
        FieldCommand::DeleteToStart,
        FieldCommand::SelectAll,
    ]);
    all
}

fn field_name(c: FieldCommand) -> String {
    let word = |m| match m {
        FieldMotion::Left => "left",
        FieldMotion::Right => "right",
        FieldMotion::WordLeft => "wordLeft",
        FieldMotion::WordRight => "wordRight",
        FieldMotion::Start => "start",
        FieldMotion::End => "end",
    };
    match c {
        FieldCommand::Move { motion, select: false } => format!("search.field.{}", word(motion)),
        FieldCommand::Move { motion, select: true } => format!("search.field.select.{}", word(motion)),
        FieldCommand::DeleteLeft => "search.field.deleteLeft".into(),
        FieldCommand::DeleteRight => "search.field.deleteRight".into(),
        FieldCommand::DeleteWordLeft => "search.field.deleteWordLeft".into(),
        FieldCommand::DeleteWordRight => "search.field.deleteWordRight".into(),
        FieldCommand::DeleteToStart => "search.field.deleteToStart".into(),
        FieldCommand::SelectAll => "search.field.selectAll".into(),
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
    /// Copies the mouse selection to the client's clipboard (OSC 52); nothing selected, nothing happens.
    Copy,
    /// Scrolls the active terminal's scrollback by a line, a page, or to its oldest / newest output.
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    ScrollTop,
    ScrollBottom,
    /// The search bar over the active terminal (`Open` shows it, the rest act while it is open).
    Search(SearchCommand),
    /// The commands side bar.
    Side(SideCommand),
    /// An edit of the search bar's query (or of the side bar's filter while it has the keys).
    Field(FieldCommand),
    Detach,
    ConfirmYes,
    ConfirmNo,
    /// Writes the shell integration's snippets and adds the line sourcing one to the shell's init file.
    InstallShellIntegration,
    /// Re-reads `settings.toml` (off-thread); it applies to the next terminal.
    ReloadSettings,
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
            Command::Copy,
            Command::ScrollUp,
            Command::ScrollDown,
            Command::PageUp,
            Command::PageDown,
            Command::ScrollTop,
            Command::ScrollBottom,
            Command::Detach,
            Command::ConfirmYes,
            Command::ConfirmNo,
            Command::InstallShellIntegration,
            Command::ReloadSettings,
        ];
        all.extend(SearchCommand::ALL.map(Command::Search));
        all.extend(SideCommand::ALL.map(Command::Side));
        all.extend(field_commands().into_iter().map(Command::Field));
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
            Command::Copy => "terminal.copySelection".into(),
            Command::ScrollUp => "terminal.scrollUp".into(),
            Command::ScrollDown => "terminal.scrollDown".into(),
            Command::PageUp => "terminal.scrollPageUp".into(),
            Command::PageDown => "terminal.scrollPageDown".into(),
            Command::ScrollTop => "terminal.scrollToTop".into(),
            Command::ScrollBottom => "terminal.scrollToBottom".into(),
            Command::Search(c) => match c {
                SearchCommand::Open => "terminal.find".into(),
                SearchCommand::Next => "search.next".into(),
                SearchCommand::Previous => "search.previous".into(),
                SearchCommand::ToggleCase => "search.toggleCase".into(),
                SearchCommand::Close => "search.close".into(),
            },
            Command::Side(c) => c.name().into(),
            Command::Field(c) => field_name(*c),
            Command::Detach => "session.detach".into(),
            Command::ConfirmYes => "confirm.yes".into(),
            Command::ConfirmNo => "confirm.no".into(),
            Command::InstallShellIntegration => "session.installShellIntegration".into(),
            Command::ReloadSettings => "session.reloadSettings".into(),
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
            Command::Copy => "Copy Selection".into(),
            Command::ScrollUp => "Scroll Up".into(),
            Command::ScrollDown => "Scroll Down".into(),
            Command::PageUp => "Scroll Page Up".into(),
            Command::PageDown => "Scroll Page Down".into(),
            Command::ScrollTop => "Scroll to Top".into(),
            Command::ScrollBottom => "Scroll to Bottom".into(),
            Command::Search(c) => match c {
                SearchCommand::Open => "Find in Terminal",
                SearchCommand::Next => "Find Next",
                SearchCommand::Previous => "Find Previous",
                SearchCommand::ToggleCase => "Toggle Find Case Sensitivity",
                SearchCommand::Close => "Close Find",
            }
            .into(),
            Command::Side(c) => c.title().into(),
            Command::Field(c) => format!("Find Field: {}", field_name(*c).trim_start_matches("search.field.")),
            Command::Detach => "Detach".into(),
            Command::ConfirmYes => "Confirm".into(),
            Command::ConfirmNo => "Cancel".into(),
            Command::InstallShellIntegration => "Install Shell Integration".into(),
            Command::ReloadSettings => "Reload Settings".into(),
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
        assert_eq!(Context::parse("search"), Some(Context::Search));
        assert_eq!(Context::parse("side"), Some(Context::Side));
        assert_eq!(Context::names().len(), 4);
    }
}
