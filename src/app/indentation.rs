//! The active buffer's indentation: the status bar item's picker, "Indent Using Spaces / Tabs",
//! "Change Tab Display Size" (their sizes picker) and "Detect Indentation from Content". What
//! they set lives on the `Buffer` (`indentation`, `tab_display`, the guess), so every view of
//! it follows; `edit::Settings::for_buffer` resolves it. A diff or hex tab has no buffer here:
//! the commands do nothing there.

use super::App;
use crate::buffer::BufferId;
use crate::command::Command;
use crate::indentation::{self, Indentation};
use crate::palette::{self, Mode, Picker, Row, SizeKind};

impl App {
    /// A click on the status bar's `Spaces: N` / `Tab Size: N`: the indentation commands.
    pub(super) fn indentation_clicked(&mut self) {
        if self.active_buffer().is_some() {
            self.open_palette(Picker::Indentation, "");
        }
    }

    /// "Indent Using Spaces / Tabs", "Change Tab Display Size" (a sizes picker for the active
    /// buffer), "Detect Indentation from Content".
    pub(super) fn indentation_command(&mut self, cmd: Command) {
        let Some(buffer) = self.group().active_tab().map(|t| t.buffer) else {
            return;
        };
        let kind = match cmd {
            Command::IndentUsingSpaces => SizeKind::Spaces,
            Command::IndentUsingTabs => SizeKind::Tabs,
            Command::ChangeTabDisplaySize => SizeKind::TabDisplay,
            Command::DetectIndentation => return self.detect_indentation(buffer),
            _ => return,
        };
        self.open_palette(Picker::IndentSize { buffer, kind }, "");
    }

    /// The sizes picker's rows and hint for `query`.
    pub(super) fn indent_size_rows(&self, query: &str) -> (Vec<Row>, String) {
        let Some(Picker::IndentSize { buffer, kind }) = self.palette.as_ref().map(|p| p.picker) else {
            return (Vec::new(), String::new());
        };
        debug_assert_eq!(self.palette.as_ref().map(|p| p.mode()), Some(Mode::IndentSize));
        let Some(buf) = self.buffers.get(buffer) else {
            return (Vec::new(), String::new());
        };
        let s = self.settings.for_buffer(buf);
        let (current, configured) = match kind {
            SizeKind::Spaces => (s.insert_spaces.then(|| s.indent_size()), self.settings.indent_size()),
            SizeKind::Tabs => ((!s.insert_spaces).then_some(s.tab_width), self.settings.tab_width),
            SizeKind::TabDisplay => (Some(s.tab_width), self.settings.tab_width),
        };
        // With the other kind in use, its size is still the one to start on (VS Code).
        let current = current.unwrap_or(if s.insert_spaces { s.indent_size() } else { s.tab_width });
        let rows = palette::size_rows(query, current, configured);
        let hint = if rows.is_empty() {
            "Type a size from 1 to 8".into()
        } else if kind == SizeKind::TabDisplay {
            "Select Tab Display Size for the current file".into()
        } else {
            "Select Tab Size for the current file".into()
        };
        (rows, hint)
    }

    /// A size picked for `buffer`: spaces or tabs of that size, or that drawn tab width.
    pub(super) fn indent_size_picked(&mut self, buffer: BufferId, kind: SizeKind, size: usize) {
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        match kind {
            SizeKind::Spaces => buf.choose_indentation(Indentation { spaces: true, size }),
            SizeKind::Tabs => buf.choose_indentation(Indentation { spaces: false, size }),
            SizeKind::TabDisplay => buf.tab_display = Some(size),
        }
        tracing::debug!(buffer, ?kind, size, "indentation chosen");
        self.reveal_after_indentation(buffer);
    }

    /// "Detect Indentation from Content": the buffer's text as it is now guessed again (the
    /// first `indentation::MAX_LINES` lines: well under a millisecond, fine here), what was
    /// chosen for it dropped.
    fn detect_indentation(&mut self, buffer: BufferId) {
        let defaults = self.settings.indentation();
        let Some(buf) = self.buffers.get_mut(buffer) else {
            return;
        };
        let guess = indentation::guess(&buf.rope, defaults);
        buf.redetect(guess);
        // Asked for, the guess applies even with `detect_indentation = false` (VS Code), where
        // `for_buffer` would ignore it: kept as the chosen indentation instead.
        if !self.settings.detect_indentation {
            let settings = self.settings.for_buffer(buf).indentation();
            buf.indentation = Some(guess.apply(settings));
        }
        tracing::debug!(buffer, ?guess, "indentation detected");
        self.reveal_after_indentation(buffer);
    }

    /// Tabs may now be drawn wider or narrower: the active view keeps its cursor in sight.
    fn reveal_after_indentation(&mut self, buffer: BufferId) {
        if let Some(tab) = self.group_mut().active_tab_mut()
            && tab.buffer == buffer
        {
            tab.view.reveal_cursor = true;
        }
    }
}
