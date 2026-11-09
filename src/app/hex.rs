//! Hex tabs in the app (`hex::HexTab`, drawn by `ui::hex`): a binary file opened from anywhere
//! arrives as `Loaded::Binary` (the bytes the load read: no second read) and shows in a hex tab,
//! reused per path as a text tab is; "View: Reopen as Hex / Text" swap a tab in place; the
//! movement / selection / copy / select-all commands and Tab (`hex_command`; every other editor
//! command does nothing there), the mouse (`hex_mouse`), Go to Offset (Ctrl+G's input).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::files::{FileEvent, Loaded, OpenRequest};
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
/// Hex) as its bytes for a hex tab (a big one is opened, not read: its pages load as they are
/// drawn).
/// Blocking: call from spawn_blocking.
pub(super) fn load(path: &Path, raw: bool, indentation: Indentation) -> Result<Loaded, LoadError> {
    if !raw {
        // A big binary file is sniffed first, so it is never read whole.
        if hex::is_big_binary(path).map_err(LoadError::Io)? {
            return open_hex(path);
        }
        return match Buffer::open_with(path, indentation) {
            Ok(buffer) => Ok(Loaded::Text(Box::new(buffer))),
            Err(LoadError::Binary(bytes)) => Ok(Loaded::Binary(HexDoc::new(bytes))),
            Err(e) => Err(e),
        };
    }
    open_hex(path)
}

fn open_hex(path: &Path) -> Result<Loaded, LoadError> {
    match hex::read(path).map_err(LoadError::Io)? {
        Some(doc) => Ok(Loaded::Binary(doc)),
        None => Err(LoadError::Io(std::io::ErrorKind::NotFound.into())),
    }
}

/// Results of hex and Markdown tab background work (`AppEvent::View`).
pub(super) enum ViewEvent {
    /// Find's whole-file count on a hex tab (`HexTab::id`) for one query generation
    /// (`find::HexFind`); an older generation's is dropped.
    HexCount {
        tab: u64,
        generation: u64,
        count: hex::Count,
    },
    /// Pages of a paged hex doc read off the UI thread (`App::sync_hex_pages`), for the doc's
    /// `version` (`HexTab::id` finds the doc); ones for another version, or a tab that is gone,
    /// are dropped.
    HexPages {
        id: u64,
        version: hex::Version,
        pages: Vec<(u64, std::io::Result<std::sync::Arc<[u8]>>)>,
    },
    /// Find's next / previous match on a paged hex doc, for one search generation
    /// (`find::HexFind::begin_step`); `None`: no match (or the read failed). An older
    /// generation's is dropped.
    HexFound {
        tab: u64,
        generation: u64,
        range: Option<std::ops::Range<u64>>,
    },
    /// A paged hex selection read for Cmd+C: its text, or why it couldn't be read.
    HexCopied { text: std::io::Result<String> },
    /// A big document's Markdown render (`MarkdownTab::id`, the generation asked for, the width
    /// and source revision it is for); an older generation's is dropped.
    MarkdownRendered {
        tab: u64,
        generation: u64,
        width: usize,
        revision: u64,
        text: std::sync::Arc<str>,
        doc: crate::markdown::Doc,
    },
}

impl App {
    pub(super) fn handle_view_event(&mut self, ev: ViewEvent) {
        match ev {
            ViewEvent::HexCount { tab, generation, count } => self.hex_counted(tab, generation, count),
            ViewEvent::HexPages { id, version, pages } => self.hex_pages_read(id, version, pages),
            ViewEvent::HexFound { tab, generation, range } => self.hex_found(tab, generation, range),
            ViewEvent::HexCopied { text } => {
                match text {
                    Ok(text) => self.clipboard.write(Copied {
                        text,
                        whole_line: false,
                        pieces: Vec::new(),
                    }),
                    Err(e) => self.notify(Level::Warning, format!("Can't copy the selection: {e}")),
                }
                self.needs_redraw = true;
            }
            ViewEvent::MarkdownRendered {
                tab,
                generation,
                width,
                revision,
                text,
                doc,
            } => self.markdown_rendered(tab, generation, width, revision, text, doc),
        }
    }
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
    /// they are) → `FileEvent::Opened` → `opened`.
    pub(super) fn start_open(&mut self, path: PathBuf, request: OpenRequest) {
        let raw = request.raw;
        self.opening.insert(path.clone(), request);
        let tx = self.tx.clone();
        let indentation = self.settings.indentation();
        tokio::task::spawn_blocking(move || {
            let result = crate::perf::time("file.open", || load(&path, raw, indentation));
            let _ = tx.send(AppEvent::Files(FileEvent::Opened { path, result }));
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
        // A paged doc is judged by its first page when that is loaded; else the open below sniffs it.
        let start = h.doc.loaded_bytes(0..(h.len().min(file::BINARY_SNIFF_LEN) as u64));
        if start.is_some_and(|b| file::looks_binary(&b)) {
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

    /// Cmd+C on a hex tab: the selection (else the cursor's byte) as its column shows it. A
    /// paged doc's bytes are read off the UI thread (`ViewEvent::HexCopied`), up to
    /// `HEX_COPY_MAX`; a bigger selection is a message. False when the active tab isn't a hex view.
    pub(super) fn hex_copy(&mut self) -> bool {
        let Some(h) = self.active_hex() else { return false };
        if let Some(text) = h.copy_text() {
            self.clipboard.write(Copied {
                text,
                whole_line: false,
                pieces: Vec::new(),
            });
            return true;
        }
        let Some(range) = h.copy_range() else { return true };
        let size = range.len() as u64;
        if size > hex::HEX_COPY_MAX {
            let text = format!(
                "Selection too big to copy ({} MB, at most {} MB)",
                size >> 20,
                hex::HEX_COPY_MAX >> 20
            );
            self.notify(Level::Warning, text);
            return true;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return true;
        };
        let (source, column) = (h.doc.source().clone(), h.column);
        let tx = self.tx.clone();
        rt.spawn_blocking(move || {
            let mut bytes = vec![0u8; size as usize];
            let text = source
                .read_exact_at(range.start as u64, &mut bytes)
                .map(|()| hex::format_bytes(column, &bytes));
            let _ = tx.send(AppEvent::View(ViewEvent::HexCopied { text }));
        });
        true
    }

    /// Starts the reads the last frames asked for (`HexDoc::window`): each paged doc's missing
    /// pages in jobs of a few, the visible ones first (`spawn_blocking`) →
    /// `ViewEvent::HexPages` → `hex_pages_read`. Called after every frame.
    pub(super) fn sync_hex_pages(&mut self) {
        let mut jobs = Vec::new();
        for h in self.groups.hex_tabs() {
            if h.doc.resident_bytes().is_some() {
                continue;
            }
            for pages in h.doc.plan_reads() {
                jobs.push((h.id, h.doc.clone(), pages));
            }
        }
        if jobs.is_empty() {
            return;
        }
        // Outside a runtime (unit tests that don't settle) the pages stay placeholders.
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for (id, doc, pages) in jobs {
            let tx = self.tx.clone();
            rt.spawn_blocking(move || {
                let version = doc.version();
                let pages = hex::read_pages(doc.source(), pages);
                let _ = tx.send(AppEvent::View(ViewEvent::HexPages { id, version, pages }));
            });
        }
    }

    /// Pages arrived: cached when the tab's doc is still the version they were read for, else
    /// dropped.
    fn hex_pages_read(
        &mut self,
        id: u64,
        version: hex::Version,
        pages: Vec<(u64, std::io::Result<std::sync::Arc<[u8]>>)>,
    ) {
        let Some(doc) = self.groups.hex_tabs().find(|h| h.id == id).map(|h| h.doc.clone()) else {
            tracing::debug!(id, "pages for a closed hex tab dropped");
            return;
        };
        if doc.finish_reads(version, pages) {
            self.needs_redraw = true;
        } else {
            tracing::debug!(id, "pages of an older version dropped");
        }
    }

    /// Go to Offset (Ctrl+G's input on a hex tab): hex (`0x1f0`, `1f0h`) or decimal, past the
    /// end → the last byte. False when it doesn't parse (the input stays open).
    pub(super) fn go_to_offset(&mut self, query: &str) -> bool {
        let Some(offset) = hex::parse_offset(query) else {
            return false;
        };
        self.nav_jump();
        if let Some(h) = self.group_mut().active_hex_mut() {
            h.move_to(offset as usize, false);
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
