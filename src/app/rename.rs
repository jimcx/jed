//! Rename symbol (F2 in the editor, `editor.rename`): `textDocument/prepareRename` when the
//! server supports it (its range / placeholder prefills the prompt, `null` or an error refuses),
//! else the word at the cursor; a prompt with the name selected; then `textDocument/rename` and
//! its `WorkspaceEdit` applied — open buffers at once, files not open loaded off the UI thread
//! into tabs beside the active one and left unsaved (VS Code leaves them dirty too, unless its
//! `files.refactoring.autoSave` saves them); one undo step per buffer.

use lsp_types::{OneOf, ServerCapabilities};
use serde_json::{Value, json};

use super::files::OpenRequest;
use super::lsp::{Pending, Request};
use super::prompt::{Input, InputKind};
use super::{App, AppEvent, Level, Prompt};
use crate::buffer::change::ChangeSet;
use crate::buffer::pos::{self, ByteIdx, PosEncoding};
use crate::buffer::undo::EditKind;
use crate::buffer::{Buffer, BufferId};
use crate::group::Tab;
use crate::layout::GroupId;
use crate::rename::{self, FileEdit, Prepared, TextEdit};
use crate::selection::{Selection, Selections};

/// VS Code's message when there's nothing to rename at the cursor.
pub(super) const CANT_RENAME: &str = "The element can't be renamed.";

/// A `WorkspaceEdit`'s edits waiting for their file to load (`OpenRequest::edits`).
#[derive(Debug)]
pub(super) struct PendingEdit {
    pub edits: Vec<TextEdit>,
    pub encoding: PosEncoding,
    /// What made them, for a failure's message ("Rename", "Code action", …).
    pub what: &'static str,
}

fn supports_rename(caps: &ServerCapabilities) -> bool {
    caps.rename_provider
        .as_ref()
        .is_some_and(|r| !matches!(r, OneOf::Left(false)))
}

fn supports_prepare(caps: &ServerCapabilities) -> bool {
    matches!(&caps.rename_provider, Some(OneOf::Right(o)) if o.prepare_provider == Some(true))
}

impl App {
    /// F2: asks the server whether the symbol at the cursor can be renamed (prepareRename), or
    /// opens the prompt with the word there when it can't say.
    pub(super) fn rename_symbol(&mut self) {
        if !self.active_writable() {
            return;
        }
        let Some((group, buffer, at, word)) = self.cursor_word() else {
            return;
        };
        // Opens the document first if this turn added it.
        self.lsp.sync_documents(&mut self.buffers);
        let prepare = match self.lsp.server_of(buffer) {
            Some((_, _, caps)) if supports_rename(caps) => supports_prepare(caps),
            _ => {
                self.notify(Level::Info, "No language server can rename here".into());
                return;
            }
        };
        if prepare {
            let request = Request::PrepareRename { word };
            if !self.send_request(
                group,
                buffer,
                at,
                "textDocument/prepareRename",
                supports_prepare,
                request,
            ) {
                self.notify(Level::Info, "No language server can rename here".into());
            }
        } else if word.is_empty() {
            self.notify(Level::Info, CANT_RENAME.into());
        } else {
            self.open_rename_prompt(group, buffer, at, word);
        }
    }

    /// A prepareRename answer (current): a range (its text or the placeholder offered), the
    /// default (the word), or a refusal.
    pub(super) fn prepare_rename_answer(&mut self, p: &Pending, word: &str, value: &Value) {
        let name = match rename::parse_prepare(value) {
            Prepared::Refused => None,
            Prepared::Default => Some(word.to_string()).filter(|w| !w.is_empty()),
            Prepared::Range {
                placeholder: Some(p), ..
            } => Some(p),
            Prepared::Range { start, end, .. } => self.buffers.get(p.buffer).map(|b| {
                let (s, e) = (
                    pos::from_lsp_pos(&b.rope, start, p.encoding),
                    pos::from_lsp_pos(&b.rope, end, p.encoding),
                );
                b.rope.slice(s.0..e.0.max(s.0)).to_string()
            }),
        };
        match name {
            Some(name) => self.open_rename_prompt(p.group, p.buffer, p.at, name),
            None => self.notify(Level::Info, CANT_RENAME.into()),
        }
    }

    /// The prompt for the new name, prefilled with `old` and all selected (typing replaces it).
    fn open_rename_prompt(&mut self, group: GroupId, buffer: BufferId, at: ByteIdx, old: String) {
        if self.prompt.is_some() {
            return;
        }
        let revision = self.buffers.get(buffer).map_or(0, Buffer::revision);
        let mut input = Input::new(
            InputKind::RenameSymbol {
                group,
                buffer,
                at,
                revision,
                old: old.clone(),
            },
            old,
        );
        input.field.select_all();
        self.prompt = Some(Prompt::Input(input));
    }

    /// The prompt's Enter: asks the server for the edits renaming `old` to `new`.
    pub(super) fn request_rename(
        &mut self,
        group: GroupId,
        buffer: BufferId,
        at: ByteIdx,
        revision: u64,
        old: String,
        new: String,
    ) {
        if self.buffers.get(buffer).is_none_or(|b| b.revision() != revision) {
            self.notify(Level::Info, "The file changed: rename cancelled".into());
            return;
        }
        let extra = json!({ "newName": new });
        let request = Request::Rename { old, new };
        if !self.send_request_with(
            group,
            buffer,
            at,
            "textDocument/rename",
            supports_rename,
            request,
            extra,
        ) {
            self.notify(Level::Info, "No language server can rename here".into());
        }
    }

    /// A rename answer (current): its `WorkspaceEdit` checked whole first (file operations,
    /// read-only or changed open buffers, overlapping edits refuse it all), then applied: open
    /// buffers now, files not open once loaded.
    pub(super) fn rename_answer(&mut self, p: &Pending, old: &str, new: &str, value: &Value) {
        let files = match rename::parse_workspace_edit(value) {
            Ok(files) => files,
            Err(e) => {
                self.notify(Level::Error, format!("Rename failed: {e}"));
                return;
            }
        };
        if files.is_empty() {
            self.notify(Level::Info, "No result.".into());
            return;
        }
        let edits: usize = files.iter().map(|f| f.edits.len()).sum();
        let count = files.len();
        if let Err(why) = self.apply_workspace_edit(files, p.encoding, "Rename") {
            self.notify(Level::Error, format!("Rename failed: {why}"));
            return;
        }
        let (edits, files) = (plural(edits, "edit"), plural(count, "file"));
        self.notify(Level::Info, format!("Renamed '{old}' to '{new}': {edits} in {files}"));
    }

    /// Applies a `WorkspaceEdit`'s `files` (positions in `encoding`), checked whole first: a
    /// read-only or changed open buffer, or overlapping edits, refuse it all (`Err`: why). Open
    /// buffers are edited now (one undo step each), files not open once loaded into tabs beside
    /// the active one (`what` names the edit if one of them fails then); other views follow.
    pub(super) fn apply_workspace_edit(
        &mut self,
        files: Vec<FileEdit>,
        encoding: PosEncoding,
        what: &'static str,
    ) -> Result<(), String> {
        let mut ready: Vec<(BufferId, ChangeSet)> = Vec::new();
        let mut unopened: Vec<FileEdit> = Vec::new();
        for f in files {
            let Some(id) = self.buffers.find_path(&f.path) else {
                unopened.push(f);
                continue;
            };
            let buf = self.buffers.get(id).expect("found");
            if let Some(reason) = &buf.read_only {
                return Err(format!("{} is read-only ({reason})", buf.name()));
            }
            // An older version is fine while its text is the buffer's (an edit undone since).
            if let Some(v) = f.version
                && Some(v) != self.lsp.version(id)
                && self.lsp.text_at(id, v).is_none_or(|text| *text != buf.rope)
            {
                return Err(format!("{} changed since it was asked", buf.name()));
            }
            match rename::changes(&buf.rope, &f.edits, encoding) {
                Ok(set) => ready.push((id, set)),
                Err(e) => return Err(format!("{}: {e}", buf.name())),
            }
        }
        for (id, set) in ready {
            self.apply_changes(id, set);
        }
        for f in unopened {
            self.open_for_edit(
                f.path,
                PendingEdit {
                    edits: f.edits,
                    encoding,
                    what,
                },
            );
        }
        self.follow_edits();
        Ok(())
    }

    /// Edits `buffer` by `changes` as one undo step: the active editor's view (if it shows the
    /// buffer) gets its selections moved with the text and is what undo restores; other views
    /// follow in `follow_edits`.
    pub(super) fn apply_changes(&mut self, buffer: BufferId, changes: ChangeSet) {
        // Undo restores these: a view of the buffer, the active one first.
        let shown = std::iter::once(self.groups.active)
            .chain(self.groups.ids())
            .filter_map(|g| self.groups.get(g))
            .find_map(|g| {
                let active = g.active_tab().filter(|t| t.buffer == buffer);
                active.or_else(|| g.tabs.iter().filter_map(Tab::text).find(|t| t.buffer == buffer))
            })
            .map(|t| t.view.selections.clone());
        let before = shown.unwrap_or_else(|| Selections::single(Selection::cursor(ByteIdx(0))));
        let g = self.groups.active_group_mut();
        let active = g.active_tab_mut().filter(|t| t.buffer == buffer);
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        let after = rename::follow(&before, &changes);
        buf.seal_undo();
        buf.edit(changes, before, after.clone(), EditKind::Other);
        buf.seal_undo();
        if let Some(tab) = active {
            tab.view.cursor_history.reset(&after);
            tab.view.selections = after;
        }
        self.needs_redraw = true;
    }

    /// Loads `path` off the UI thread into a tab beside the active one, then makes `edit` there
    /// (`apply_pending_edits`). A file already being opened gets the edit when it arrives.
    fn open_for_edit(&mut self, path: std::path::PathBuf, edit: PendingEdit) {
        if let Some(pending) = self.opening.get_mut(&path) {
            pending.edits.push(edit);
            return;
        }
        self.opening.insert(
            path.clone(),
            OpenRequest {
                background: true,
                edits: vec![edit],
                ..Default::default()
            },
        );
        let tx = self.tx.clone();
        let indentation = self.settings.indentation();
        tokio::task::spawn_blocking(move || {
            // A missing file would load empty: the edits were made for text that isn't there.
            let result = if path.exists() {
                Buffer::open_with(&path, indentation).map(|b| super::files::Loaded::Text(Box::new(b)))
            } else {
                Err(crate::buffer::file::LoadError::Io(std::io::ErrorKind::NotFound.into()))
            };
            let _ = tx.send(AppEvent::Opened { path, result });
        });
    }

    /// A file loaded for a `WorkspaceEdit` (`opened`): its edits, each one undo step.
    pub(super) fn apply_pending_edits(&mut self, path: &std::path::Path, edits: Vec<PendingEdit>) {
        let Some(id) = self.buffers.find_path(path) else { return };
        for edit in edits {
            let buf = self.buffers.get(id).expect("found");
            let set = match &buf.read_only {
                Some(reason) => Err(format!("read-only ({reason})")),
                None => rename::changes(&buf.rope, &edit.edits, edit.encoding),
            };
            match set {
                Ok(set) => self.apply_changes(id, set),
                Err(e) => {
                    let text = format!("{} not applied to {}: {e}", edit.what, path.display());
                    self.notify(Level::Error, text);
                }
            }
        }
        self.follow_edits();
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}
