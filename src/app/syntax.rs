//! Language detection for open buffers and their background parses.

use crate::buffer::{Buffer, BufferId};
use crate::syntax::{self, Associations, Language, Parsed};

use super::{App, AppEvent, Level};

impl App {
    /// The language of `buf`: `.workspace` associations, then its name, then its shebang.
    pub(super) fn detect_language(&self, buf: &Buffer) -> Language {
        let rope = &buf.rope;
        let head = rope.slice(..rope.floor_char_boundary(rope.len().min(256))).to_string();
        let first_line = head.lines().next().unwrap_or("");
        syntax::detect(&buf.path, &self.workspace.root, first_line, &self.associations)
    }

    /// Re-detects every open buffer's language (associations changed, files renamed).
    pub(super) fn redetect_languages(&mut self) {
        let langs: Vec<(BufferId, Language)> = self
            .buffers
            .iter()
            .map(|(id, b)| (id, self.detect_language(b)))
            .collect();
        for (id, lang) in langs {
            if let Some(buf) = self.buffers.get_mut(id) {
                buf.syntax.set_language(lang);
            }
        }
        self.servers_dirty = true;
    }

    /// Compiles `.workspace`'s `[files] associations`, reporting entries it can't use.
    pub(super) fn load_associations(&mut self) {
        let (associations, errors) = Associations::new(&self.workspace.config.files.associations);
        self.associations = associations;
        if let Some(first) = errors.first() {
            self.notify(
                Level::Error,
                format!("{}: associations: {first}", crate::workspace::FILE_NAME),
            );
        }
    }

    /// Brings every buffer's parse tree up to date: small edits are re-parsed here, first parses
    /// and big edits are started off-thread (→ `AppEvent::Parsed`). The event loop calls it once
    /// per turn, before drawing. Without a runtime (`App::new`, plain unit tests) it waits.
    pub(super) fn sync_syntax(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let max = self.settings.syntax_max_bytes;
        for (id, buf) in self.buffers.iter_mut() {
            if let Some(job) = buf.syntax.update(&buf.rope, max) {
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = tx.send(AppEvent::Parsed {
                        buffer: id,
                        parsed: job.run(),
                    });
                });
            }
        }
    }

    pub(super) fn parsed(&mut self, buffer: BufferId, parsed: Parsed) {
        if let Some(buf) = self.buffers.get_mut(buffer) {
            buf.syntax.parsed(parsed);
        }
    }
}
