//! Every user action is a named command; keybindings (and later the palette) go through these.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// Toggles between first non-blank and line start.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    Move {
        motion: Motion,
        select: bool,
    },
    DeleteLeft,
    DeleteRight,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteToLineStart,
    InsertNewline,
    InsertTab,
    SelectAll,
    /// Esc: drop extra cursors, then clear the selection.
    CancelSelection,
    Undo,
    Redo,
    Save,
    Quit,
    CloseTab,
    NextTab,
    PrevTab,
    ToggleSidebar,
    FocusExplorer,
    /// Focus editor group N (1-based, reading order); one past the last splits right.
    FocusGroup(u8),
    SplitRight,
    SplitDown,
    Explorer(ExplorerCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExplorerCommand {
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Collapse, or go to the parent.
    Left,
    /// Expand, or step into.
    Right,
    /// Open a file and focus it; toggle a folder.
    Open,
    /// Open a file but keep focus in the explorer.
    Preview,
    NewFile,
    NewFolder,
    Rename,
    Delete,
}

impl Command {
    /// Stable name, used in logs, keybinding config and the palette.
    pub fn name(&self) -> String {
        match self {
            Command::Move { motion, select } => {
                let m = match motion {
                    Motion::Left => "left",
                    Motion::Right => "right",
                    Motion::Up => "up",
                    Motion::Down => "down",
                    Motion::WordLeft => "wordLeft",
                    Motion::WordRight => "wordRight",
                    Motion::LineStart => "home",
                    Motion::LineEnd => "end",
                    Motion::DocStart => "top",
                    Motion::DocEnd => "bottom",
                    Motion::PageUp => "pageUp",
                    Motion::PageDown => "pageDown",
                };
                format!("cursor.{m}{}", if *select { ".select" } else { "" })
            }
            Command::DeleteLeft => "edit.deleteLeft".into(),
            Command::DeleteRight => "edit.deleteRight".into(),
            Command::DeleteWordLeft => "edit.deleteWordLeft".into(),
            Command::DeleteWordRight => "edit.deleteWordRight".into(),
            Command::DeleteToLineStart => "edit.deleteToLineStart".into(),
            Command::InsertNewline => "edit.newline".into(),
            Command::InsertTab => "edit.tab".into(),
            Command::SelectAll => "selection.all".into(),
            Command::CancelSelection => "selection.cancel".into(),
            Command::Undo => "edit.undo".into(),
            Command::Redo => "edit.redo".into(),
            Command::Save => "file.save".into(),
            Command::Quit => "app.quit".into(),
            Command::CloseTab => "tab.close".into(),
            Command::NextTab => "tab.next".into(),
            Command::PrevTab => "tab.previous".into(),
            Command::ToggleSidebar => "view.toggleSidebar".into(),
            Command::FocusExplorer => "view.focusExplorer".into(),
            Command::FocusGroup(n) => format!("view.focusGroup{n}"),
            Command::SplitRight => "view.splitRight".into(),
            Command::SplitDown => "view.splitDown".into(),
            Command::Explorer(c) => format!(
                "explorer.{}",
                match c {
                    ExplorerCommand::Up => "up",
                    ExplorerCommand::Down => "down",
                    ExplorerCommand::PageUp => "pageUp",
                    ExplorerCommand::PageDown => "pageDown",
                    ExplorerCommand::First => "first",
                    ExplorerCommand::Last => "last",
                    ExplorerCommand::Left => "collapse",
                    ExplorerCommand::Right => "expand",
                    ExplorerCommand::Open => "open",
                    ExplorerCommand::Preview => "preview",
                    ExplorerCommand::NewFile => "newFile",
                    ExplorerCommand::NewFolder => "newFolder",
                    ExplorerCommand::Rename => "rename",
                    ExplorerCommand::Delete => "delete",
                }
            ),
        }
    }
}
