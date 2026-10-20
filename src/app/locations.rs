//! Go to definition (F12, Ctrl / Cmd+Click), go to references (Shift+F12) and the Locations
//! list they fill (`app.locations`, a `locations::Locations`; drawn by `ui::panel` in the bottom
//! panel's "LOCATIONS" tab): definition's several places, references always.

use std::path::PathBuf;

use crossterm::event::MouseEvent;
use lsp_types::{OneOf, ServerCapabilities};
use ropey::Rope;
use serde_json::{Value, json};

use super::files::Goto;
use super::lsp::{Pending, Request};
use super::{App, AppEvent, Focus, Level};
use crate::buffer::BufferId;
use crate::buffer::pos::{self, ByteIdx};
use crate::command::LocationsCommand as L;
use crate::layout::{GroupId, PanelTab};
use crate::locations::{self, Location, Locations};
use crate::problems::{Place, Target};

fn supports_definition(caps: &ServerCapabilities) -> bool {
    caps.definition_provider
        .as_ref()
        .is_some_and(|d| !matches!(d, OneOf::Left(false)))
}

fn supports_references(caps: &ServerCapabilities) -> bool {
    caps.references_provider
        .as_ref()
        .is_some_and(|d| !matches!(d, OneOf::Left(false)))
}

/// The status bar's text when there are no references (VS Code's).
pub(super) fn no_references(word: &str) -> String {
    if word.is_empty() {
        "No references found".into()
    } else {
        format!("No references found for '{word}'")
    }
}

/// The status bar's text when there's no definition (VS Code's).
pub(super) fn no_definition(word: &str) -> String {
    if word.is_empty() {
        "No definition found".into()
    } else {
        format!("No definition found for '{word}'")
    }
}

impl App {
    /// The active group, buffer and primary cursor of the active tab, and the word there (empty
    /// when none); seals the buffer's undo step (a request isn't an edit).
    pub(super) fn cursor_word(&mut self) -> Option<(GroupId, BufferId, ByteIdx, String)> {
        let group = self.groups.active;
        let tab = self.group().active_tab()?;
        let (buffer, at) = (tab.buffer, tab.view.selections.primary().head);
        let word = self
            .buffers
            .get(buffer)
            .and_then(|b| pos::word_at(&b.rope, at).map(|(s, e)| b.rope.slice(s.0..e.0).to_string()))
            .unwrap_or_default();
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        Some((group, buffer, at, word))
    }

    /// F12: asks the server for the definition of what's at the active view's cursor.
    pub(super) fn go_to_definition(&mut self) {
        let Some((group, buffer, at, word)) = self.cursor_word() else {
            return;
        };
        let request = Request::Definition { word: word.clone() };
        if !self.send_request(
            group,
            buffer,
            at,
            "textDocument/definition",
            supports_definition,
            request,
        ) {
            self.notify(Level::Info, no_definition(&word));
        }
    }

    /// A definition answer (current): none → a message; one → opened there (a cursor at its
    /// start, centred); several → the Locations list, focused on its first place.
    pub(super) fn definition_answer(&mut self, p: &Pending, word: &str, value: &Value) {
        let mut places = locations::parse(value);
        match places.len() {
            0 => self.notify(Level::Info, no_definition(word)),
            1 => {
                let l = places.pop().expect("one");
                let place = Place::Lsp {
                    start: l.start,
                    end: l.start,
                    encoding: p.encoding,
                };
                self.open_file(l.path, true, Some(Goto::Problem { place, select: false }));
                // Set by a background answer, so no key records it: Cmd+U goes back from there.
                // (A file loading now starts its own history.)
                let g = self.groups.active_group_mut();
                if let Some(tab) = g.active_tab_mut() {
                    let view = &mut tab.view;
                    let (top, left) = (view.top_line(), view.left_col);
                    view.cursor_history.record(&view.selections, top, left);
                }
            }
            _ => {
                let title = if word.is_empty() {
                    "Definitions".to_string()
                } else {
                    format!("Definitions of '{word}'")
                };
                self.show_locations(title, places, p.encoding);
            }
        }
    }

    /// Shift+F12: asks the server for the references to what's at the active view's cursor
    /// (the declaration included, as VS Code).
    pub(super) fn go_to_references(&mut self) {
        let Some((group, buffer, at, word)) = self.cursor_word() else {
            return;
        };
        let request = Request::References { word: word.clone() };
        let context = json!({ "context": { "includeDeclaration": true } });
        if !self.send_request_with(
            group,
            buffer,
            at,
            "textDocument/references",
            supports_references,
            request,
            context,
        ) {
            self.notify(Level::Info, no_references(&word));
        }
    }

    /// A references answer (current): none → a message; else the Locations list, even for one
    /// place (VS Code jumps to a lone reference; with the declaration included that's usually
    /// where the cursor already is, so the list says "1" instead of seeming to do nothing).
    pub(super) fn references_answer(&mut self, p: &Pending, word: &str, value: &Value) {
        let places = locations::parse(value);
        if places.is_empty() {
            self.notify(Level::Info, no_references(word));
            return;
        }
        let title = if word.is_empty() {
            "References".to_string()
        } else {
            format!("References to '{word}'")
        };
        self.show_locations(title, places, p.encoding);
    }

    /// Fills the Locations list and shows it (panel focused, its first place selected); files not
    /// open are read off the UI thread for their previews (`AppEvent::LocationsRead`).
    fn show_locations(&mut self, title: String, places: Vec<Location>, encoding: crate::buffer::pos::PosEncoding) {
        self.locations_generation += 1;
        let generation = self.locations_generation;
        let mut list = Locations::new(title, encoding, places, generation);
        for path in list.unread() {
            if let Some(buf) = self.buffers.find_path(&path).and_then(|id| self.buffers.get(id)) {
                list.read(&path, &buf.rope);
            }
        }
        let unread = list.unread();
        list.list.select(&list.files, 1);
        self.locations = Some(list);
        self.focus_panel(PanelTab::Locations);
        if unread.is_empty() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let files = unread
                .into_iter()
                .filter_map(|path| {
                    let bytes = std::fs::read(&path).ok()?;
                    let text = Rope::from_str(&String::from_utf8_lossy(&bytes));
                    Some((path, text))
                })
                .collect();
            let _ = tx.send(AppEvent::LocationsRead { generation, files });
        });
    }

    /// Files read for the Locations list's previews (dropped if the list was replaced).
    pub(super) fn locations_read(&mut self, generation: u64, files: Vec<(PathBuf, Rope)>) {
        if let Some(list) = self.locations.as_mut().filter(|l| l.generation == generation) {
            for (path, text) in files {
                list.read(&path, &text);
            }
        }
    }

    pub(super) fn locations_command(&mut self, cmd: L) {
        let Some(list) = self.locations.as_mut() else {
            if cmd == L::FocusEditor && !self.group().tabs.is_empty() {
                self.ui.focus = Focus::Editor;
            }
            return;
        };
        let state = &mut list.list;
        match cmd {
            L::Up | L::Down | L::PageUp | L::PageDown => {
                let page = state.height.max(2) as isize - 1;
                let delta = match cmd {
                    L::Up => -1,
                    L::Down => 1,
                    L::PageUp => -page,
                    _ => page,
                };
                state.move_selection(&list.files, delta);
            }
            L::First | L::Last => state.select(&list.files, if cmd == L::First { 0 } else { usize::MAX }),
            L::Open => {
                if let Some(t) = list.target() {
                    self.open_location(t, true);
                }
            }
            L::FocusEditor => {
                if !self.group().tabs.is_empty() {
                    self.ui.focus = Focus::Editor;
                }
            }
        }
    }

    /// Opens a place in the active (last focused) group with a cursor at its start, centred.
    fn open_location(&mut self, t: Target, focus: bool) {
        let place = match t.place {
            Place::Lsp { start, encoding, .. } => Place::Lsp {
                start,
                end: start,
                encoding,
            },
            p => p,
        };
        self.open_file(t.path, focus, Some(Goto::Problem { place, select: false }));
    }

    /// A click in the Locations list, as in the Problems list: the row is selected and the list
    /// gets the keys; a place row opens with the focus staying in the list, a double click (also
    /// on a file row: its first place) opens it and focuses the editor.
    pub(super) fn locations_click(&mut self, m: MouseEvent, count: u8) {
        self.ui.focus = Focus::Panel;
        let Some(hit) = self.hits.panel_list.take() else { return };
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        if hit.list.contains(at)
            && let Some(key) = hit.rows.get((at.y - hit.list.y) as usize)
            && let Some(list) = self.locations.as_mut()
            && list.select_key(key)
            && (key.1 > 0 || count >= 2)
            && let Some(t) = list.target()
        {
            self.open_location(t, count >= 2);
        }
        self.hits.panel_list = Some(hit);
    }

    /// The mouse wheel over the panel scrolls the list it shows.
    pub(super) fn locations_scroll(&mut self, delta: isize) {
        if let Some(list) = self.locations.as_mut() {
            list.list.scroll_by(&list.files, delta);
        }
    }
}
