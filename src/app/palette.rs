//! The palette: opening it, its keys and mouse, filling its rows, and what accepting a row does.
//! Also the commands only the palette reaches: trust, log file, log level.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::mouse::{Drag, FieldTarget, field_at};
use super::{App, AppEvent, Focus, Level};
use crate::buffer::pos;
use crate::command::{Command, PaletteCommand as P};
use crate::explorer;
use crate::field::Outcome;
use crate::hex;
use crate::logging;
use crate::mru;
use crate::palette::{self, Action, FileMatcher, Mode, Palette, Picker};
use crate::paths;

/// Mouse wheel steps through this many rows.
const WHEEL_ROWS: isize = 3;
/// How soon quick open ticks its matcher again while matching runs (nucleo suggests ~10 ms).
const FILES_POLL: Duration = Duration::from_millis(10);

impl App {
    /// Opens (or reopens) the palette with `text` in its input.
    pub(super) fn open_palette(&mut self, picker: Picker, text: &str) {
        self.palette = Some(Palette::new(picker, text));
        self.refresh_palette();
    }

    pub(super) fn close_palette(&mut self) {
        self.palette = None;
        self.file_history.picker = None;
        self.recent.picker = None;
        if let Some(files) = &mut self.files {
            files.stop();
        }
    }

    pub(super) fn palette_command(&mut self, cmd: P) {
        let Some(p) = &mut self.palette else { return };
        let page = p.height.max(1) as isize;
        match cmd {
            P::Next => p.move_selection(1),
            P::Previous => p.move_selection(-1),
            P::PageDown => p.move_selection(page),
            P::PageUp => p.move_selection(-page),
            P::Accept => self.accept_palette(),
            P::Close => self.close_palette(),
        }
    }

    /// Keys the keymap doesn't bind in the palette edit its input.
    pub(super) fn palette_key(&mut self, key: KeyEvent) {
        let Some(p) = &mut self.palette else { return };
        let mut field = std::mem::take(&mut p.field);
        let outcome = self.field_key(&mut field, Some(crate::config::keymap::Context::Palette), key);
        if let Some(p) = &mut self.palette {
            p.field = field;
        }
        if outcome == Outcome::Edited {
            self.refresh_palette();
        }
    }

    pub(super) fn palette_paste(&mut self, text: &str) {
        let Some(p) = &mut self.palette else { return };
        p.field.insert(&text.replace(['\r', '\n'], ""));
        self.refresh_palette();
    }

    /// Rebuilds the rows for the current input.
    pub(super) fn refresh_palette(&mut self) {
        let Some(p) = &self.palette else { return };
        let (mode, query) = (p.mode(), p.query().to_string());
        let mut hint = None;
        let rows = match mode {
            Mode::Commands => {
                let commands: Vec<Command> = Command::all()
                    .into_iter()
                    .filter(|c| c.in_palette() && self.mode.allows(*c))
                    .collect();
                let recent: &[Command] = if query.is_empty() { &self.mru.commands } else { &[] };
                let rows = palette::command_rows_with_recent(&query, &commands, recent, |c| {
                    self.keymap.binding_for(c).map(|k| k.label())
                });
                if rows.is_empty() {
                    hint = Some("No matching commands".into());
                }
                rows
            }
            Mode::LogLevel => {
                let current = self.log.as_ref().and_then(logging::current_filter);
                hint = Some(format!(
                    "Current filter: {}. Pick a level, or type a filter like jed::app=debug,info",
                    current.as_deref().unwrap_or("logging is off")
                ));
                if query.contains('=') {
                    Vec::new()
                } else {
                    palette::log_level_rows(&query, current.as_deref())
                }
            }
            Mode::GoToLine => {
                hint = Some(self.line_hint(&query));
                Vec::new()
            }
            Mode::History => match &self.file_history.picker {
                Some(h) => {
                    let rows = palette::history_rows(&query, &h.entries, crate::history::now_ms());
                    let rel = h.rel.display();
                    hint = Some(if rows.is_empty() {
                        "No matching entries".into()
                    } else if h.restore {
                        format!("{rel}: Enter restores the entry into the buffer (not saved)")
                    } else {
                        format!("{rel}: Enter compares the entry with yours")
                    });
                    rows
                }
                None => Vec::new(),
            },
            Mode::Tasks => {
                let rows = palette::task_rows(&query, &self.task_templates());
                hint = if rows.is_empty() {
                    Some("No matching tasks".into())
                } else {
                    self.restricted_tasks_note()
                };
                rows
            }
            Mode::Recent => match &self.recent.picker {
                Some(entries) => {
                    let rows = palette::recent_rows(&query, entries, &paths::home());
                    if rows.is_empty() {
                        hint = Some("No matching workspaces".into());
                    }
                    rows
                }
                None => Vec::new(),
            },
            Mode::Logs => {
                let rows = palette::log_rows(&query, &self.log_tools());
                if rows.is_empty() {
                    hint = Some("No matching logs".into());
                }
                rows
            }
            Mode::Indentation => {
                let mut rows = palette::command_rows(&query, &palette::INDENTATION_ACTIONS, |c| {
                    self.keymap.binding_for(c).map(|k| k.label())
                });
                if query.trim().is_empty() {
                    // Their own order, not alphabetical.
                    rows.sort_by_key(|r| {
                        palette::INDENTATION_ACTIONS
                            .iter()
                            .position(|c| r.action == Action::Run(*c))
                    });
                }
                hint = Some(if rows.is_empty() {
                    "No matching commands".into()
                } else {
                    "Select Action".into()
                });
                rows
            }
            Mode::IndentSize => {
                let (rows, text) = self.indent_size_rows(&query);
                hint = Some(text);
                rows
            }
            Mode::Language => {
                let (rows, text) = self.language_rows(&query);
                hint = Some(text);
                rows
            }
            Mode::Files if !self.mode.allows(Command::QuickOpen) => {
                hint = Some(super::mode::UNAVAILABLE.into());
                Vec::new()
            }
            Mode::Files => {
                let files = self.file_matcher();
                files.set_query(palette::file_query(&query));
                let running = files.tick();
                let rows = files.rows();
                self.files_poll = running.then(|| Instant::now() + FILES_POLL);
                self.start_file_walk();
                let recent: &[PathBuf] = if query.is_empty() { &self.mru.files } else { &[] };
                palette::file_rows_with_recent(rows, recent)
            }
        };
        if let Some(p) = &mut self.palette {
            p.set_rows(rows);
            p.hint = hint;
            // A size picker starts on the current size (VS Code).
            if matches!(mode, Mode::IndentSize | Mode::Language)
                && let Some(i) = p
                    .rows
                    .iter()
                    .position(|r| r.detail.ends_with("current") || r.detail.starts_with("current"))
            {
                p.select(i);
            }
        }
        self.update_files_hint();
    }

    fn line_hint(&self, query: &str) -> String {
        // A hex tab: Go to Offset.
        if let Some(h) = self.active_hex() {
            return self.offset_hint(h, query);
        }
        let Some((buf, tab)) = self.active_buffer().zip(self.group().active_tab()) else {
            return "Open a file first to go to a line".into();
        };
        let lines = pos::line_count(&buf.rope);
        match palette::parse_line(query) {
            Some((line, Some(col))) => format!("Go to line {line}, column {col}"),
            Some((line, None)) => format!("Go to line {line}"),
            None => {
                let head = tab.view.selections.primary().head;
                format!(
                    "Current line: {}, column: {}. Type a line number between 1 and {lines} (line:column for a column)",
                    pos::line_of(&buf.rope, head) + 1,
                    pos::display_col(&buf.rope, head, self.settings.for_buffer(buf).tab_width).0 + 1,
                )
            }
        }
    }

    fn file_matcher(&mut self) -> &mut FileMatcher {
        let tx = self.tx.clone();
        let pending = self.files_pending.clone();
        self.files.get_or_insert_with(|| {
            // Called from nucleo's threads and the walk, once per file: send one wake-up at a time.
            FileMatcher::new(Arc::new(move || {
                if !pending.swap(true, Ordering::AcqRel) {
                    let _ = tx.send(AppEvent::FilesMatched);
                }
            }))
        })
    }

    /// Walks the workspace into quick open's list, once per opening of the palette.
    fn start_file_walk(&mut self) {
        let root = self.workspace.root.clone();
        let excludes = self.excludes.clone();
        let tx = self.tx.clone();
        let files = self.file_matcher();
        if !files.cancel.load(Ordering::Relaxed) {
            return; // this opening's walk is running or done
        }
        let (injector, cancel) = files.restart();
        let generation = files.generation;
        tokio::task::spawn_blocking(move || {
            let t = std::time::Instant::now();
            let mut count = 0usize;
            explorer::walk_files(&root, &excludes, &cancel, |rel| {
                count += 1;
                injector.push(rel);
            });
            tracing::debug!(count, elapsed_ms = t.elapsed().as_millis() as u64, "quick open walk");
            let _ = tx.send(AppEvent::FilesWalked { generation });
        });
    }

    /// Matching made progress: take in the new results.
    pub(super) fn files_matched(&mut self) {
        self.files_pending.store(false, Ordering::Release);
        self.files_poll = None;
        if self.palette.as_ref().is_none_or(|p| p.mode() != Mode::Files) {
            return;
        }
        let query_empty = self.palette.as_ref().is_some_and(|p| p.query().is_empty());
        let Some(files) = &mut self.files else { return };
        let running = files.tick();
        let rows = files.rows();
        self.files_poll = running.then(|| Instant::now() + FILES_POLL);
        let recent: &[PathBuf] = if query_empty { &self.mru.files } else { &[] };
        let rows = palette::file_rows_with_recent(rows, recent);
        if let Some(p) = &mut self.palette {
            // Keep the selection on the same file where possible.
            let selected = p.selected_row().map(|r| r.action.clone());
            p.rows = rows;
            let index = selected.and_then(|a| p.rows.iter().position(|r| r.action == a));
            p.select(index.unwrap_or(0));
        }
        self.update_files_hint();
    }

    pub(super) fn files_walked(&mut self, generation: u64) {
        if let Some(files) = &mut self.files
            && files.generation == generation
        {
            files.walked = true;
        }
        self.files_matched();
    }

    fn update_files_hint(&mut self) {
        let (Some(p), Some(files)) = (&mut self.palette, &self.files) else {
            return;
        };
        if p.mode() != Mode::Files {
            return;
        }
        let (matched, total) = files.counts();
        let recent = if p.query().is_empty() { self.mru.files.len() } else { 0 };
        p.hint = if !files.walked {
            Some(format!("Looking for files… {total} so far"))
        } else if matched == 0 && recent == 0 {
            Some("No matching files".into())
        } else if matched > palette::MAX_FILE_ROWS {
            Some(format!("Best {} of {matched} matches", palette::MAX_FILE_ROWS))
        } else {
            None
        };
    }

    fn accept_palette(&mut self) {
        let Some(p) = &self.palette else { return };
        let (mode, query) = (p.mode(), p.query().to_string());
        let action = p.selected_row().map(|r| r.action.clone());
        match (mode, action) {
            (Mode::GoToLine, _) if self.active_hex().is_some() => {
                if hex::parse_offset(&query).is_some() {
                    self.close_palette();
                    self.go_to_offset(&query);
                }
            }
            (Mode::GoToLine, _) => {
                let Some((line, col)) = palette::parse_line(&query) else {
                    return;
                };
                self.close_palette();
                self.go_to_line(line, col.unwrap_or(1));
            }
            (Mode::LogLevel, _) if query.contains('=') => {
                self.close_palette();
                self.set_log_filter(query.trim());
            }
            (_, Some(Action::SetLogLevel(level))) => {
                self.close_palette();
                self.set_log_filter(level);
            }
            (_, Some(Action::Run(cmd))) => {
                self.close_palette();
                // Only a command picked here counts as recently used, not one from a key.
                mru::record(&mut self.mru.commands, cmd);
                self.execute(cmd);
            }
            (_, Some(Action::History(snapshot))) => {
                let picker = self.file_history.picker.take();
                self.close_palette();
                if let Some(picker) = picker {
                    self.history_picked(picker, snapshot);
                }
            }
            (_, Some(Action::OpenWorkspace(path))) => {
                let entry = self
                    .recent
                    .picker
                    .take()
                    .and_then(|e| e.into_iter().find(|e| e.path == path));
                self.close_palette();
                self.open_recent(entry.unwrap_or_else(|| crate::recent::Entry::new(path)));
            }
            (_, Some(Action::RunTask(name))) => {
                self.close_palette();
                self.run_task(&name);
            }
            (_, Some(Action::OpenLog(tool))) => {
                self.close_palette();
                self.open_tool_log(tool);
            }
            (_, Some(Action::IndentSize(size))) => {
                let picker = p.picker;
                self.close_palette();
                if let Picker::IndentSize { buffer, kind } = picker {
                    self.indent_size_picked(buffer, kind, size);
                }
            }
            (_, Some(Action::Language(lang))) => {
                let picker = p.picker;
                self.close_palette();
                if let Picker::Language { buffer } = picker {
                    self.language_picked(buffer, lang);
                }
            }
            (_, Some(Action::Open(rel))) => {
                let goto = paths::strip_position(&query).map(|(_, (l, c))| super::files::Goto::Cursor(l, c));
                self.close_palette();
                self.open_file(self.workspace.root.join(rel), true, goto);
            }
            (_, None) => {}
        }
    }

    /// Mouse while the palette is open: click the input (the cursor there, a drag selects; it
    /// takes every mouse event, so unlike the other fields it keeps the drag here), click a row
    /// to accept it, wheel to scroll, click outside to close it.
    pub(super) fn palette_mouse(&mut self, m: MouseEvent) {
        let Some(hit) = self.hits.palette else { return };
        if self.palette.is_none() {
            return;
        }
        let inside = |r: ratatui::layout::Rect| {
            m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height
        };
        if matches!(m.kind, MouseEventKind::Down(_)) {
            // A press while a drag is armed means its release was lost (as `mouse::left_click`).
            self.drag = None;
        }
        match m.kind {
            MouseEventKind::ScrollUp => {
                if let Some(p) = &mut self.palette {
                    p.scroll_by(-WHEEL_ROWS);
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(p) = &mut self.palette {
                    p.scroll_by(WHEEL_ROWS);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(hit.input) => {
                let count = self.clicks.click(Instant::now(), m.column, m.row);
                let shift = m.modifiers.contains(KeyModifiers::SHIFT);
                let at = self
                    .palette
                    .as_ref()
                    .map(|p| field_at(&p.field, hit.input, hit.input_scroll, m.column));
                if let (Some(at), Some(p)) = (at, self.palette.as_mut()) {
                    p.field.click(at, count, shift);
                }
                self.drag = Some(Drag::Field(FieldTarget::Palette));
            }
            MouseEventKind::Down(MouseButton::Left) if inside(hit.rows) => {
                let Some(p) = &mut self.palette else { return };
                let index = p.scroll + (m.row - hit.rows.y) as usize;
                if index < p.rows.len() {
                    p.select(index);
                    self.accept_palette();
                }
            }
            MouseEventKind::Down(_) if !inside(hit.area) => self.close_palette(),
            MouseEventKind::Drag(MouseButton::Left) if matches!(self.drag, Some(Drag::Field(FieldTarget::Palette))) => {
                let at = self
                    .palette
                    .as_ref()
                    .map(|p| field_at(&p.field, hit.input, hit.input_scroll, m.column));
                if let (Some(at), Some(p)) = (at, self.palette.as_mut()) {
                    p.field.drag_to(at);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if matches!(self.drag, Some(Drag::Field(FieldTarget::Palette))) {
                    self.drag = None;
                }
            }
            _ => {}
        }
    }

    /// Puts the cursor on a 1-based line and column of the active view.
    fn go_to_line(&mut self, line: usize, col: usize) {
        self.nav_jump();
        let settings = self.settings.clone();
        if let Some((buf, view)) = self.active_mut() {
            buf.seal_undo();
            let tab_width = settings.for_buffer(buf).tab_width;
            super::files::place_cursor(view, &buf.rope, (line, col), tab_width);
            self.ui.focus = Focus::Editor;
        }
    }

    pub(super) fn manage_trust(&mut self) {
        if self.workspace.config.has_commands() {
            self.ask_trust();
        } else {
            self.notify(
                Level::Info,
                format!("{} names no commands: nothing to trust", crate::workspace::FILE_NAME),
            );
        }
    }

    pub(super) fn open_log_file(&mut self) {
        let dir = self.log_dir.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(AppEvent::LogFileFound(logging::newest_log(&dir)));
        });
    }

    /// Opens the user's `settings.toml` in a tab, first writing the commented template (and the
    /// directory) if it doesn't exist.
    pub(super) fn open_user_settings(&mut self) {
        let path = self.config_dir.join(crate::config::settings::FILE_NAME);
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(AppEvent::UserSettingsReady(crate::config::settings::create_if_missing(
                &path,
            )));
        });
    }

    pub(super) fn log_file_found(&mut self, path: Option<PathBuf>) {
        match path {
            Some(path) => self.open_file(path, true, None),
            None => self.notify(Level::Warning, format!("No log file in {}", self.log_dir.display())),
        }
    }

    fn set_log_filter(&mut self, filter: &str) {
        let Some(handle) = &self.log else {
            self.notify(Level::Error, "Logging is off: see the start-up error".into());
            return;
        };
        match logging::set_filter(handle, filter) {
            Ok(()) => self.notify(Level::Info, format!("Log level: {filter}")),
            Err(e) => self.notify(Level::Error, format!("Bad log filter {filter}: {e}")),
        }
    }
}
