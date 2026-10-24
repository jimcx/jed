//! The keyboard shortcuts view (`app.keys`, model in `crate::keys`): opening and closing it, and
//! while it's open every key and mouse event (only a prompt comes first).

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};

use super::App;
use crate::keys::KeysView;
use crate::terminal::Pointer;

impl App {
    /// "Preferences: Keyboard Shortcuts": opens the view, or closes it.
    pub(super) fn toggle_keyboard_shortcuts(&mut self) {
        if self.keys.take().is_none() {
            self.keys = Some(KeysView);
        }
    }

    /// A key while it's open: Esc closes it, anything else is swallowed.
    pub(super) fn keyboard_shortcuts_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            self.keys = None;
        }
    }

    /// A mouse event while it's open: a press closes it (nothing is drawn yet, so every press is
    /// outside it); other events do nothing.
    pub(super) fn keyboard_shortcuts_mouse(&mut self, m: MouseEvent) {
        self.pointer = Pointer::Default;
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.keys = None;
        }
    }
}
