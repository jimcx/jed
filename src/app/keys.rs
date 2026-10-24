//! The keyboard shortcuts view (`app.keys`, model in `crate::keys`): opening and closing it, its
//! search and selection while it's open, and its mouse events.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::App;
use super::mouse::{Drag, WHEEL_LINES};
use crate::keys::KeysView;
use crate::layout::{Hit, ScrollTarget};
use crate::terminal::Pointer;

impl App {
    /// "Preferences: Keyboard Shortcuts": opens the view, or closes it.
    pub(super) fn toggle_keyboard_shortcuts(&mut self) {
        if self.keys.take().is_none() {
            self.keys = Some(KeysView::new(&self.keymap));
        }
    }

    /// A key while it's open: Esc closes it, a typed character edits the search, Backspace
    /// deletes, the arrows / pages / Home / End move the selection, anything else is swallowed.
    pub(super) fn keyboard_shortcuts_key(&mut self, key: KeyEvent) {
        let Some(view) = &mut self.keys else { return };
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            self.keys = None;
            return;
        }
        // Ctrl / Alt / Cmd chords are swallowed: they are bindings, not search text.
        if !(key.modifiers - KeyModifiers::SHIFT).is_empty() {
            return;
        }
        match key.code {
            KeyCode::Char(c) => {
                view.query.push(c);
                view.refilter();
            }
            KeyCode::Backspace => {
                if view.query.pop().is_some() {
                    view.refilter();
                }
            }
            KeyCode::Up => view.move_selection(-1),
            KeyCode::Down => view.move_selection(1),
            KeyCode::PageUp => view.page(-1),
            KeyCode::PageDown => view.page(1),
            KeyCode::Home => view.select(0),
            KeyCode::End => view.select(usize::MAX),
            _ => {}
        }
    }

    /// A mouse event while it's open: the wheel scrolls it, its bar drags, a press outside it
    /// closes it (and does nothing else).
    pub(super) fn keyboard_shortcuts_mouse(&mut self, m: MouseEvent) {
        self.pointer = Pointer::Default;
        let target = ScrollTarget::Keys;
        let Some(hit) = self.hits.keys else {
            // Nothing drawn yet: every press is outside it.
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.keys = None;
            }
            return;
        };
        let p = ratatui::layout::Position { x: m.column, y: m.row };
        let on_bar = hit.scrollbar.is_some_and(|s| s.track.contains(p));
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if m.kind == MouseEventKind::ScrollUp {
                    -WHEEL_LINES
                } else {
                    WHEEL_LINES
                };
                if let Some(view) = &mut self.keys {
                    view.scroll_by(delta);
                }
            }
            MouseEventKind::Down(_) if !hit.rect.contains(p) => self.keys = None,
            MouseEventKind::Down(MouseButton::Left) if on_bar => self.scrollbar_click(target, m.column, m.row),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(Drag::ScrollBar { target, grab }) = self.drag {
                    self.scrollbar_drag(target, m.column, m.row, grab);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
        let over = if on_bar { Hit::ScrollBar(target) } else { Hit::Nothing };
        self.scrollbar_hover(&m, over);
    }
}
