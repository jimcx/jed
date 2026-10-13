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
        }
    }
}
