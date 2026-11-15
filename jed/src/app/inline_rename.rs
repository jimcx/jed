//! Renaming an Explorer entry in place (F2, `explorer.rename`): the row's name becomes a text
//! field with the cursor at its end (as the prompt it replaces). Enter or a click elsewhere
//! commits through `FileOp::Rename` (so open tabs, file history and listings follow); Esc, or an
//! unchanged name, cancels; an invalid name keeps the field and says why.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, FileOp, Level};
use crate::explorer;
use crate::field::{Outcome, TextField};

/// The field over a row's name.
#[derive(Debug, Clone)]
pub struct InlineRename {
    pub from: PathBuf,
    pub field: TextField,
}

impl App {
    /// Starts renaming the selected row (the root can't be renamed).
    pub(super) fn start_inline_rename(&mut self) {
        let Some(from) = self.explorer.selected.clone() else {
            return;
        };
        if from == self.workspace.root {
            return;
        }
        let name = from.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let field = TextField::new(name);
        self.inline_rename = Some(InlineRename { from, field });
    }

    pub(super) fn cancel_inline_rename(&mut self) {
        self.inline_rename = None;
    }

    /// Commits the typed name. A bad name keeps the field open; the same name just closes it.
    pub(super) fn commit_inline_rename(&mut self) {
        let Some(r) = self.inline_rename.take() else { return };
        let name = r.field.text.trim().to_string();
        if let Err(e) = explorer::validate_name(&name, false) {
            self.notify(Level::Error, e.into());
            self.inline_rename = Some(r);
            return;
        }
        let to = r.from.with_file_name(&name);
        if to != r.from {
            self.run_file_op(FileOp::Rename { from: r.from, to });
        }
    }

    pub(super) fn inline_rename_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.commit_inline_rename(),
            KeyCode::Esc => self.cancel_inline_rename(),
            _ => {
                let Some(mut r) = self.inline_rename.take() else { return };
                let _: Outcome = self.field_key(&mut r.field, None, key);
                self.inline_rename = Some(r);
            }
        }
    }

    pub(super) fn inline_rename_paste(&mut self, text: &str) {
        if let Some(r) = &mut self.inline_rename {
            r.field.insert(&text.replace(['\r', '\n'], ""));
        }
    }
}
