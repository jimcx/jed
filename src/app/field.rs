//! Keys in a text field (the find widget, the Search view's inputs, the palette, prompts): the
//! editor's own bindings, run on the field's one line.

use crossterm::event::KeyEvent;

use super::{App, Focus};
use crate::clipboard::Copied;
use crate::command::Command;
use crate::config::keymap::Context;
use crate::field::{FieldCommand, Outcome, TextField};

/// Whether `cmd` means something in a text field: a `FieldCommand`, or copy / cut / paste.
pub(super) fn has_field_meaning(cmd: Command) -> bool {
    matches!(cmd, Command::Copy | Command::Cut | Command::Paste) || FieldCommand::from_command(cmd).is_some()
}

impl App {
    /// Whether a text field has the keys (a prompt aside, which takes them all): the palette's,
    /// the find widget's or the Search view's focused input (not its results list).
    pub(super) fn field_focused(&self) -> bool {
        self.palette.is_some()
            || self.ui.focus == Focus::Find
            || (self.search_focused() && !self.search_view.list_focused)
    }

    /// A key for a text field: a plain char is typed; else the command the field's own context
    /// bound to it when that has a field meaning (`handle_event` leaves those to the field, so
    /// a global `keybindings.toml` binding of an editor command applies here too), else the
    /// key's editor-context command. A command with a field meaning runs on `field` (moves,
    /// deletes, select all — `FieldCommand::from_command` — and copy / cut / paste through
    /// `app.clipboard`); others do nothing. So Home / End, Shift+arrows, word moves and rebinds
    /// work as in the editor, with no field keys of their own. `own` is the field's context
    /// (`None` for a prompt's field).
    pub(super) fn field_key(&mut self, field: &mut TextField, own: Option<Context>, key: KeyEvent) -> Outcome {
        let typed = field.key(key);
        if typed != Outcome::Ignored {
            return typed;
        }
        let cmd = own
            .and_then(|c| self.keymap.lookup(&key, c))
            .filter(|&c| has_field_meaning(c))
            .or_else(|| self.keymap.lookup(&key, Context::Editor));
        let Some(cmd) = cmd else {
            return Outcome::Ignored;
        };
        match cmd {
            Command::Copy | Command::Cut => {
                let text = if cmd == Command::Cut { field.cut() } else { field.copy() };
                let Some(text) = text else { return Outcome::Ignored };
                self.clipboard.write(Copied {
                    text,
                    ..Copied::default()
                });
                if cmd == Command::Cut {
                    Outcome::Edited
                } else {
                    Outcome::Ignored
                }
            }
            Command::Paste => match self.read_clipboard() {
                Some(text) => {
                    field.insert(&text.replace(['\r', '\n'], ""));
                    Outcome::Edited
                }
                None => Outcome::Ignored,
            },
            _ => FieldCommand::from_command(cmd).map_or(Outcome::Ignored, |c| field.run(c)),
        }
    }
}
