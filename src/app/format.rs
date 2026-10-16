//! Format document (Shift+Alt+F, Linux Ctrl+Shift+I, `editor.formatDocument`):
//! `textDocument/formatting` with the buffer language's tab size and spaces, its `TextEdit[]`
//! applied as one undo step (`apply_changes`, shared with rename), each edit first shrunk to
//! what it changes (`format::minimize`) so cursors outside the changes stay put. An answer is
//! dropped when the text or the cursor changed meanwhile (`request_current`).

use lsp_types::{OneOf, ServerCapabilities};
use serde_json::{Value, json};

use super::lsp::{Pending, Request};
use super::{App, Level};
use crate::format;
use crate::rename;

fn supports_formatting(caps: &ServerCapabilities) -> bool {
    caps.document_formatting_provider
        .as_ref()
        .is_some_and(|f| !matches!(f, OneOf::Left(false)))
}

impl App {
    pub(super) fn format_document(&mut self) {
        if !self.active_writable() {
            return;
        }
        let group = self.groups.active;
        let Some(tab) = self.group().active_tab() else { return };
        let (buffer, at) = (tab.buffer, tab.view.selections.primary().head);
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        buf.seal_undo();
        let language = buf.syntax.language();
        let options = format::options(&self.settings.for_language(language));
        let extra = json!({ "options": options });
        if !self.send_doc_request(
            (group, buffer, at),
            "textDocument/formatting",
            supports_formatting,
            Request::Format,
            extra,
        ) {
            // VS Code's message.
            let text = format!("There is no formatter for '{}' files installed.", language.id());
            self.notify(Level::Info, text);
        }
    }

    /// A formatting answer (current: the text and the cursor unchanged since it was asked).
    pub(super) fn format_answer(&mut self, p: &Pending, value: &Value) {
        let edits = match format::parse(value) {
            Ok(edits) => edits,
            Err(e) => {
                self.notify(Level::Error, format!("Format Document failed: {e}"));
                return;
            }
        };
        let Some(buf) = self.buffers.get(p.buffer) else { return };
        if let Some(reason) = &buf.read_only {
            self.notify(Level::Error, format!("Read-only: {reason}"));
            return;
        }
        let set = match rename::changes(&buf.rope, &edits, p.encoding) {
            Ok(set) => format::minimize(&buf.rope, &set),
            Err(e) => {
                self.notify(Level::Error, format!("Format Document failed: {e}"));
                return;
            }
        };
        if set.is_empty() {
            tracing::debug!("format document: nothing to change");
            return;
        }
        self.apply_changes(p.buffer, set);
        self.follow_edits();
    }
}
