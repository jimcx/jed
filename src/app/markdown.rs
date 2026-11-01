//! Markdown preview tabs in the app (`markdown_tab::MarkdownTab`, drawn by `ui::markdown`):
//! "Markdown: Open Preview to the Side" (`open_markdown_preview`), the movement / selection /
//! select-all / copy commands (`markdown_command`; every other editor command does nothing in a
//! preview), Cmd+C (`markdown_copy`) and the mouse (`markdown_mouse`: wheel, click, Shift+click,
//! drag).

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::mouse::Drag;
use super::{App, Level};
use crate::clipboard::Copied;
use crate::command::Command;
use crate::group::Tab;
use crate::layout::{Hit, SplitDir};
use crate::markdown_tab::MarkdownTab;
use crate::syntax::Language;

/// Rows the wheel scrolls; columns for a sideways wheel.
const WHEEL_ROWS: isize = 3;
const WHEEL_COLS: isize = 4;

impl App {
    /// The active tab's Markdown preview, if it is one.
    pub fn active_markdown(&self) -> Option<&MarkdownTab> {
        self.group().active_markdown()
    }

    /// "Markdown: Open Preview to the Side": a preview of the active Markdown text tab in the
    /// editor group after the active one (a new split to the right when there is none), active
    /// there; the source's group keeps the focus. A preview of that source in any group is
    /// focused instead. Another kind of tab (or language): a notice and nothing else.
    pub(super) fn open_markdown_preview(&mut self) {
        let Some(buf) = self.active_buffer() else {
            self.notify(Level::Info, "Markdown preview needs a Markdown file".to_string());
            return;
        };
        if buf.syntax.language() != Language::Markdown {
            self.notify(Level::Info, "Markdown preview needs a Markdown file".to_string());
            return;
        }
        let source = self.group().active_tab().map(|t| t.buffer).expect("a text tab");
        if let Some((g, i)) = self.groups.find_markdown(source) {
            if let Some(group) = self.groups.get_mut(g) {
                group.active = i;
            }
            self.focus_group(g);
            return;
        }
        let tab = MarkdownTab::new(
            source,
            buf.has_file().then(|| buf.path.clone()),
            buf.name(),
            buf.rope.to_string().into(),
        );
        let from = self.groups.active;
        let ids = self.groups.ids();
        let next = ids
            .iter()
            .position(|&g| g == from)
            .and_then(|i| ids.get(i + 1).copied());
        let to = match next {
            Some(g) => g,
            None => {
                let g = self.groups.split_empty(SplitDir::Right);
                self.groups.active = from;
                g
            }
        };
        if let Some(g) = self.groups.get_mut(to) {
            g.open_tab(Tab::Markdown(Box::new(tab)));
        }
        self.ui.focus = super::Focus::Editor;
        self.needs_redraw = true;
    }

    /// A command while the active tab is a preview: movement and selection (Shift), select all,
    /// Esc (drops the selection). Every other editor command does nothing there (Cmd+C is
    /// `copy`'s). False when the active tab isn't one.
    pub(super) fn markdown_command(&mut self, cmd: Command) -> bool {
        let Some(m) = self.group_mut().active_markdown_mut() else {
            return false;
        };
        match cmd {
            Command::Move { motion, select } => m.motion(motion, select),
            Command::SelectAll => m.select_all(),
            Command::CancelSelection => m.anchor = None,
            _ => tracing::debug!(command = %cmd.name(), "not in a Markdown preview"),
        }
        self.needs_redraw = true;
        true
    }

    /// Cmd+C on a preview: the selected rendered text (nothing without a selection). False when
    /// the active tab isn't a preview.
    pub(super) fn markdown_copy(&mut self) -> bool {
        let Some(m) = self.active_markdown() else { return false };
        if let Some(text) = m.copy_text() {
            self.clipboard.write(Copied {
                text,
                whole_line: false,
                pieces: Vec::new(),
            });
        }
        true
    }

    /// The mouse over a group's preview: the wheel scrolls it (without focusing; sideways with
    /// Shift or a sideways wheel), a click puts the caret under the pointer (Shift: extends the
    /// selection) and starts a drag selecting from there. False for other events and groups not
    /// showing a preview.
    pub(super) fn markdown_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        if let (MouseEventKind::Drag(MouseButton::Left), Some(Drag::Markdown(g))) = (m.kind, &self.drag) {
            let g = *g;
            let Some(rows) = self.hits.group(g).map(|h| h.text) else {
                return true;
            };
            let row = m.row.saturating_sub(rows.y) as usize;
            let col = m.column.saturating_sub(rows.x) as usize;
            if let Some(t) = self.groups.get_mut(g).and_then(|g| g.active_markdown_mut()) {
                t.click(row, col, true);
            }
            return true;
        }
        let Hit::Editor(g) = hit else { return false };
        let rows = self.hits.group(g).map(|h| h.text).unwrap_or_default();
        let Some(t) = self.groups.get_mut(g).and_then(|g| g.active_markdown_mut()) else {
            return false;
        };
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
        match m.kind {
            MouseEventKind::ScrollUp if shift => t.scroll_cols(-WHEEL_COLS),
            MouseEventKind::ScrollDown if shift => t.scroll_cols(WHEEL_COLS),
            MouseEventKind::ScrollUp => t.scroll_by(-WHEEL_ROWS),
            MouseEventKind::ScrollDown => t.scroll_by(WHEEL_ROWS),
            MouseEventKind::ScrollLeft => t.scroll_cols(-WHEEL_COLS),
            MouseEventKind::ScrollRight => t.scroll_cols(WHEEL_COLS),
            MouseEventKind::Down(MouseButton::Left) => {
                let p = ratatui::layout::Position { x: m.column, y: m.row };
                if rows.contains(p) {
                    t.click((m.row - rows.y) as usize, (m.column - rows.x) as usize, shift);
                    self.drag = Some(Drag::Markdown(g));
                }
                self.focus_group(g);
            }
            _ => return false,
        }
        true
    }
}
