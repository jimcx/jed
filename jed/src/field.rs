//! A one-line text field (`jtui::field`) with the editor's commands mapped onto it.

use crate::command::{Command, Motion};

pub use jtui::field::*;

/// The field meaning of an editor command: moves along the line (line / document start and
/// end are the field's ends; up / down / page moves have none), deletes and select all.
/// Copy / cut / paste need the clipboard: `App::field_key` handles them.
pub fn from_command(cmd: Command) -> Option<FieldCommand> {
    Some(match cmd {
        Command::Move { motion, select } => {
            let motion = match motion {
                Motion::Left => FieldMotion::Left,
                Motion::Right => FieldMotion::Right,
                Motion::WordLeft => FieldMotion::WordLeft,
                Motion::WordRight => FieldMotion::WordRight,
                Motion::LineStart | Motion::DocStart => FieldMotion::Start,
                Motion::LineEnd | Motion::DocEnd => FieldMotion::End,
                Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => return None,
            };
            FieldCommand::Move { motion, select }
        }
        Command::DeleteLeft => FieldCommand::DeleteLeft,
        Command::DeleteRight => FieldCommand::DeleteRight,
        Command::DeleteWordLeft => FieldCommand::DeleteWordLeft,
        Command::DeleteWordRight => FieldCommand::DeleteWordRight,
        Command::DeleteToLineStart => FieldCommand::DeleteToStart,
        Command::SelectAll => FieldCommand::SelectAll,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_commands_with_a_field_meaning() {
        let m = |motion, select| Command::Move { motion, select };
        let f = |motion, select| Some(FieldCommand::Move { motion, select });
        assert_eq!(from_command(m(Motion::LineStart, true)), f(FieldMotion::Start, true));
        assert_eq!(from_command(m(Motion::DocEnd, false)), f(FieldMotion::End, false));
        assert_eq!(
            from_command(m(Motion::WordLeft, false)),
            f(FieldMotion::WordLeft, false)
        );
        assert_eq!(from_command(m(Motion::Up, false)), None);
        assert_eq!(from_command(m(Motion::PageDown, true)), None);
        assert_eq!(
            from_command(Command::DeleteToLineStart),
            Some(FieldCommand::DeleteToStart)
        );
        assert_eq!(from_command(m(Motion::Left, false)), f(FieldMotion::Left, false));
        assert_eq!(from_command(m(Motion::Right, true)), f(FieldMotion::Right, true));
        assert_eq!(
            from_command(m(Motion::WordRight, false)),
            f(FieldMotion::WordRight, false)
        );
        assert_eq!(from_command(m(Motion::LineEnd, false)), f(FieldMotion::End, false));
        assert_eq!(from_command(m(Motion::DocStart, true)), f(FieldMotion::Start, true));
        assert_eq!(from_command(m(Motion::Down, false)), None);
        assert_eq!(from_command(m(Motion::PageUp, false)), None);
        assert_eq!(from_command(Command::DeleteLeft), Some(FieldCommand::DeleteLeft));
        assert_eq!(from_command(Command::DeleteRight), Some(FieldCommand::DeleteRight));
        assert_eq!(
            from_command(Command::DeleteWordLeft),
            Some(FieldCommand::DeleteWordLeft)
        );
        assert_eq!(
            from_command(Command::DeleteWordRight),
            Some(FieldCommand::DeleteWordRight)
        );
        assert_eq!(from_command(Command::SelectAll), Some(FieldCommand::SelectAll));
        assert_eq!(from_command(Command::Undo), None);
        assert_eq!(from_command(Command::InsertNewline), None);
    }
}
