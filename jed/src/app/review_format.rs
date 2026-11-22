//! The comment box's formatting buttons as one command: `review_format` ("Code Review: Bold", …,
//! `Command::ReviewFormat`). `comment_format` has the Markdown per forge and the edit; this
//! applies it to the box's buffer as one `EditKind::Other` step.

use super::{App, Level};
use crate::buffer::undo::EditKind;
use crate::comment_format::{self, Markup, Style};
use crate::forge::Kind;

impl App {
    /// Formats the open comment box's selections with `style`, in the form the merge request's
    /// forge renders. A style the forge doesn't render says so and changes nothing.
    pub(super) fn review_format(&mut self, style: Style) {
        if self.composer.is_none() {
            return self.notify(Level::Info, "Open a comment box first".into());
        }
        let Some(kind) = self.review.review().map(|r| r.kind) else {
            return self.notify(Level::Info, "No merge request".into());
        };
        self.format_composer(kind, style, comment_format::markup(kind, style));
    }

    /// `markup` (None: `kind` has no `style`) applied to the box.
    pub(super) fn format_composer(&mut self, kind: Kind, style: Style, markup: Option<Markup>) {
        let Some(markup) = markup else {
            return self.notify(
                Level::Info,
                format!("{} has no {}", kind.display_name(), style.title().to_lowercase()),
            );
        };
        let Some(c) = &mut self.composer else { return };
        let before = c.view.selections.clone();
        let (changes, after) = comment_format::apply(&c.buf.rope, &before, markup);
        if changes.is_empty() {
            return;
        }
        c.buf.edit(changes, before, after.clone(), EditKind::Other);
        c.view.selections = after;
        c.view.reveal_cursor = true;
        self.composer_changed();
    }

    /// For tests: selects `from..to` (bytes) of the open comment box's text.
    #[cfg(test)]
    pub fn test_composer_select(&mut self, from: usize, to: usize) {
        use crate::buffer::pos::ByteIdx;
        use crate::selection::{Selection, Selections};
        let c = self.composer.as_mut().expect("a comment box is open");
        c.view.selections = Selections::single(Selection::new(ByteIdx(from), ByteIdx(to)));
    }

    /// For tests: the open comment box's selected text.
    #[cfg(test)]
    pub fn test_composer_selected(&self) -> String {
        let c = self.composer.as_ref().expect("a comment box is open");
        let s = c.view.selections.primary();
        c.buf.rope.slice(s.start().0..s.end().0).to_string()
    }

    /// For tests: the unsupported path no forge takes today.
    #[cfg(test)]
    pub fn test_format_unsupported(&mut self, kind: Kind, style: Style) {
        self.format_composer(kind, style, None);
    }
}
