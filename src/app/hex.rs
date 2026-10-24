//! Hex tabs in the app (`hex::HexTab`, drawn by `ui::hex`): a binary file opened from anywhere
//! arrives as `Loaded::Binary` (the bytes the load read: no second read) and shows in a hex tab,
//! reused per path as a text tab is; "View: Reopen as Hex / Text" swap a tab in place; the
//! movement / selection / copy / select-all commands and Tab (`hex_command`; every other editor
//! command does nothing there), the mouse (`hex_mouse`), Go to Offset (Ctrl+G's input).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::files::{Loaded, OpenRequest};
use super::mouse::Drag;
use super::{App, AppEvent, Focus, Level};
use crate::buffer::file::{self, LoadError};
use crate::buffer::{Buffer, BufferId};
use crate::clipboard::Copied;
use crate::command::Command;
use crate::group::{Tab, TextTab};
use crate::hex::{self, HexDoc, HexTab};
use crate::indentation::Indentation;
use crate::layout::Hit;
use crate::view::View;

/// Rows the wheel scrolls.
const WHEEL_ROWS: isize = 3;

/// Reads `path` for a tab: a text file as a buffer, a binary one (or any file, `raw`: Reopen as
/// Hex) as its bytes for a hex tab; one over `HEX_MAX_BYTES` is an error.
/// Blocking: call from spawn_blocking.
pub(super) fn load(path: &Path, raw: bool, indentation: Indentation) -> Result<Loaded, LoadError> {
    let too_big = |e: String| LoadError::Io(std::io::Error::other(e));
    if !raw {
        // A binary file too big for a hex tab is refused before it's read whole.
        if let Ok(meta) = std::fs::metadata(path)
            && let Err(e) = hex::check_size(meta.len())
            && sniff_binary(path).map_err(LoadError::Io)?
        {
            return Err(too_big(e));
        }
        return match Buffer::open_with(path, indentation) {
            Ok(buffer) => Ok(Loaded::Text(Box::new(buffer))),
            Err(LoadError::Binary(bytes)) => binary(bytes),
            Err(e) => Err(e),
        };
    }
    match hex::read(path).map_err(LoadError::Io)? {
        Some(doc) => Ok(Loaded::Binary(doc)),
        None => Err(LoadError::Io(std::io::ErrorKind::NotFound.into())),
    }
}

/// Whether the file's start looks binary (`file::looks_binary`), reading only that much.
fn sniff_binary(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut start = Vec::new();
    std::fs::File::open(path)?
        .take(file::BINARY_SNIFF_LEN as u64)
        .read_to_end(&mut start)?;
    Ok(file::looks_binary(&start))
}

/// A binary file's bytes as a hex tab's doc (the stamp hashed here, off the UI thread).
fn binary(bytes: Vec<u8>) -> Result<Loaded, LoadError> {
    hex::check_size(bytes.len() as u64).map_err(|e| LoadError::Io(std::io::Error::other(e)))?;
    Ok(Loaded::Binary(HexDoc::new(bytes)))
}

impl App {
    /// The active tab's hex view, if it is one.
    pub fn active_hex(&self) -> Option<&HexTab> {
        self.group().active_hex()
    }

    /// The command line's file is binary (`main`): its hex tab, focused. Starts nothing (it runs
    /// before `start`, which reveals it).
    pub fn open_initial_hex(&mut self, path: PathBuf, doc: HexDoc) {
        self.group_mut().open_tab(Tab::Hex(Box::new(HexTab::new(path, doc))));
        self.ui.focus = Focus::Editor;
    }

    /// Reads `path` off the UI thread for `request` (`raw`: its bytes for a hex tab, whatever
    /// they are) → `AppEvent::Opened` → `opened`.
    pub(super) fn start_open(&mut self, path: PathBuf, request: OpenRequest) {
        let raw = request.raw;
        self.opening.insert(path.clone(), request);
        let tx = self.tx.clone();
        let indentation = self.settings.indentation();
        tokio::task::spawn_blocking(move || {
            let result = crate::perf::time("file.open", || load(&path, raw, indentation));
            let _ = tx.send(AppEvent::Opened { path, result });
        });
    }

    /// Shows `path`'s hex tab when a group has one: the active group's, else a new tab in it
    /// sharing another group's bytes (no read). False when no group has one.
    pub(super) fn show_hex(&mut self, path: &Path, focus: bool) -> bool {
        if let Some(i) = self.group().find_hex(path) {
            self.activate_tab(i);
        } else {
            let other = self.groups.ids().into_iter().find_map(|id| {
                let g = self.groups.get(id)?;
                g.tabs[g.find_hex(path)?].hex().cloned()
            });
            let Some(other) = other else { return false };
            if let Some((buf, _)) = self.active_mut() {
                buf.seal_undo();
            }
            let mut tab = HexTab::new(other.path, other.doc);
            tab.deleted = other.deleted;
            self.group_mut().open_tab(Tab::Hex(Box::new(tab)));
            self.reveal_active();
        }
        if focus {
            self.ui.focus = Focus::Editor;
        }
        true
    }

    /// A binary file's bytes arrived (`opened`): its hex tab in the active group — in place of
    /// the group's text tab of the file for Reopen as Hex (`reopen`; not if it has edits now),
    /// else beside the active tab (or the group's hex tab of it shown).
    pub(super) fn add_hex_tab(&mut self, path: PathBuf, doc: HexDoc, focus: bool, reopen: bool) {
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        let text = reopen.then(|| self.clean_text_tab(&path)).flatten();
        let tab = Tab::Hex(Box::new(HexTab::new(path.clone(), doc)));
        match text {
            Some((i, buffer)) => self.replace_tab(i, tab, Some(buffer)),
            None => match self.group().find_hex(&path) {
                Some(i) => self.group_mut().active = i,
                None => {
                    self.group_mut().open_tab(tab);
                }
            },
        }
        if focus {
            self.ui.focus = Focus::Editor;
        }
        self.reveal_active();
    }

    /// The active group's text tab of `path`, when its buffer has no unsaved edits.
    fn clean_text_tab(&self, path: &Path) -> Option<(usize, BufferId)> {
        let id = self.buffers.find_path(path)?;
        let i = self.group().find(id)?;
        (!self.buffers.get(id)?.is_dirty()).then_some((i, id))
    }

    /// Puts `tab` at the active group's tab `i` and activates it; `old`: the buffer the replaced
    /// tab showed, dropped when no tab shows it any more.
    fn replace_tab(&mut self, i: usize, tab: Tab, old: Option<BufferId>) {
        let group = self.group_mut();
        group.tabs[i] = tab;
        group.active = i;
        if let Some(id) = old
            && self.groups.tabs_showing(id) == 0
        {
            self.buffers.remove(id);
        }
    }

    /// A text buffer arrived for Reopen as Text (`opened`): in place of the active group's hex
    /// tab of the file. False when the group has none (the caller opens it as usual).
    pub(super) fn replace_hex_with_text(&mut self, path: &Path, id: BufferId) -> bool {
        let Some(i) = self.group().find_hex(path) else {
            return false;
        };
        self.replace_tab(
            i,
            Tab::Text(TextTab {
                buffer: id,
                view: View::new(),
            }),
            None,
        );
        true
    }

    /// "View: Reopen as Hex": the active text tab's file, its bytes on disk read again, in a hex
    /// tab in its place. Refused while the buffer has unsaved edits (the bytes shown are the
    /// file's, not the buffer's).
    pub(super) fn reopen_as_hex(&mut self) {
        let Some(id) = self.group().active_tab().map(|t| t.buffer) else {
            return;
        };
        let Some(buf) = self.buffers.get(id).filter(|b| b.has_file()) else {
            return;
        };
        let path = buf.path.clone();
        if buf.is_dirty() {
            let text = format!(
                "{} has unsaved changes: save them before reopening it as hex",
                buf.name()
            );
            return self.notify(Level::Info, text);
        }
        if self.show_hex_in_group(&path) || self.opening.contains_key(&path) {
            return;
        }
        self.start_open(
            path,
            OpenRequest {
                focus: true,
                raw: true,
                reopen: true,
                ..Default::default()
            },
        );
    }

    /// The active group's hex tab of `path` shown, if it has one.
    fn show_hex_in_group(&mut self, path: &Path) -> bool {
        let Some(i) = self.group().find_hex(path) else {
            return false;
        };
        self.activate_tab(i);
        true
    }

    /// "View: Reopen as Text": the active hex tab's file in a text tab in its place (its open
    /// buffer, else loaded off the UI thread); a binary file stays (it can't be text).
    pub(super) fn reopen_as_text(&mut self) {
        let Some(h) = self.active_hex() else { return };
        let path = h.path.clone();
        if file::looks_binary(h.doc.bytes()) {
            let text = format!("{} is a binary file: it opens as hex only", h.name());
            return self.notify(Level::Info, text);
        }
        if let Some(id) = self.buffers.find_path(&path) {
            let i = self.group().active;
            match self.group().find(id) {
                // The group shows the buffer already: that tab, the hex one closed.
                Some(j) => {
                    let group = self.group_mut();
                    group.close(i);
                    group.active = if j > i { j - 1 } else { j };
                }
                None => self.replace_tab(
                    i,
                    Tab::Text(TextTab {
                        buffer: id,
                        view: View::new(),
                    }),
                    None,
                ),
            }
            self.ui.focus = Focus::Editor;
            return;
        }
        if self.opening.contains_key(&path) {
            return;
        }
        self.start_open(
            path,
            OpenRequest {
                focus: true,
                reopen: true,
                ..Default::default()
            },
        );
    }

    /// A command while the active tab is a hex view: movement and selection (Shift), select
    /// all, Esc (drops the selection), Tab / Shift+Tab (the other column). Every other editor
    /// command does nothing there (Cmd+C is `copy`'s). False when the active tab isn't one.
    pub(super) fn hex_command(&mut self, cmd: Command) -> bool {
        let Some(h) = self.group_mut().active_hex_mut() else {
            return false;
        };
        match cmd {
            Command::Move { motion, select } => h.motion(motion, select),
            Command::SelectAll => h.select_all(),
            Command::CancelSelection => h.anchor = None,
            Command::InsertTab | Command::OutdentLines => h.toggle_column(),
            _ => tracing::debug!(command = %cmd.name(), "not in a hex view"),
        }
        self.needs_redraw = true;
        true
    }

    /// Cmd+C on a hex tab: the selection (else the cursor's byte) as its column shows it. False
    /// when the active tab isn't a hex view.
    pub(super) fn hex_copy(&mut self) -> bool {
        let Some(h) = self.active_hex() else { return false };
        if let Some(text) = h.copy_text() {
            self.clipboard.write(Copied {
                text,
                whole_line: false,
                pieces: Vec::new(),
            });
        }
        true
    }

    /// Go to Offset (Ctrl+G's input on a hex tab): hex (`0x1f0`, `1f0h`) or decimal, past the
    /// end → the last byte. False when it doesn't parse (the input stays open).
    pub(super) fn go_to_offset(&mut self, query: &str) -> bool {
        let Some(offset) = hex::parse_offset(query) else {
            return false;
        };
        self.nav_jump();
        if let Some(h) = self.group_mut().active_hex_mut() {
            h.move_to(offset, false);
        }
        self.ui.focus = Focus::Editor;
        true
    }

    /// Ctrl+G's hint on a hex tab.
    pub(super) fn offset_hint(&self, h: &HexTab, query: &str) -> String {
        match hex::parse_offset(query) {
            Some(o) => format!("Go to offset 0x{o:x} ({o})"),
            None => format!(
                "Current offset: 0x{c:x} ({c}). Type an offset in hex (0x1f0, 1f0h) or decimal (496), below {}",
                h.len(),
                c = h.cursor,
            ),
        }
    }

    /// The mouse over a group's hex tab: the wheel scrolls it (without focusing), a click puts
    /// the cursor on the byte under it in that column (Shift: extends the selection) and starts
    /// a drag selecting from there, wherever the pointer then goes. False for other events and
    /// groups not showing a hex tab.
    pub(super) fn hex_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        if let (MouseEventKind::Drag(MouseButton::Left), Some(Drag::Hex(g))) = (m.kind, &self.drag) {
            let g = *g;
            let Some(rows) = self.hits.group(g).map(|h| h.text) else {
                return true;
            };
            let row = m.row.saturating_sub(rows.y) as usize;
            let col = m.column.saturating_sub(rows.x) as usize;
            if let Some(h) = self.groups.get_mut(g).and_then(|g| g.active_hex_mut())
                && let Some((offset, _)) = h.offset_at(row, col, true)
            {
                h.move_to(offset, true);
            }
            return true;
        }
        let Hit::Editor(g) = hit else { return false };
        let rows = self.hits.group(g).map(|h| h.text).unwrap_or_default();
        let Some(h) = self.groups.get_mut(g).and_then(|g| g.active_hex_mut()) else {
            return false;
        };
        match m.kind {
            MouseEventKind::ScrollUp => h.scroll_by(-WHEEL_ROWS),
            MouseEventKind::ScrollDown => h.scroll_by(WHEEL_ROWS),
            MouseEventKind::Down(MouseButton::Left) => {
                let p = ratatui::layout::Position { x: m.column, y: m.row };
                if rows.contains(p)
                    && let Some((offset, column)) =
                        h.offset_at((m.row - rows.y) as usize, (m.column - rows.x) as usize, false)
                {
                    h.column = column;
                    h.move_to(offset, m.modifiers.contains(KeyModifiers::SHIFT));
                    self.drag = Some(Drag::Hex(g));
                }
                self.focus_group(g);
            }
            _ => return false,
        }
        true
    }
}
