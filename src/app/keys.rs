//! The keyboard shortcuts view (`app.keys`, model in `crate::keys`): opening and closing it, its
//! search and selection while it's open, record-keys mode, Enter's `keybindings.toml` stub, and
//! its mouse events.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::mouse::{Drag, WHEEL_LINES};
use super::{App, AppEvent};
use crate::config::keymap::KeyCombo;
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

    /// A key while it's open: Esc closes it (or cancels recording), Alt+K records the next combo,
    /// a typed character edits the search, Backspace deletes, Enter writes the selected row's
    /// `keybindings.toml` stub, the arrows / pages / Home / End move the selection, anything else
    /// is swallowed.
    pub(super) fn keyboard_shortcuts_key(&mut self, key: KeyEvent) {
        let Some(view) = &mut self.keys else { return };
        // Esc cancels recording first; only otherwise does it close the view.
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            if view.record {
                view.record = false;
            } else {
                self.keys = None;
            }
            return;
        }
        // Alt+K toggles recording (VS Code's record-keys chord).
        if key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Char('k') {
            view.record = !view.record;
            return;
        }
        if view.record {
            // The navigation keys still move the selection; anything else is the combo recorded.
            match key.code {
                KeyCode::Up => view.move_selection(-1),
                KeyCode::Down => view.move_selection(1),
                KeyCode::PageUp => view.page(-1),
                KeyCode::PageDown => view.page(1),
                KeyCode::Home => view.select(0),
                KeyCode::End => view.select(usize::MAX),
                _ => {
                    let combo = KeyCombo::from_event(&key);
                    view.query = combo.label();
                    view.refilter();
                    view.record = false;
                }
            }
            return;
        }
        if key.code == KeyCode::Enter && key.modifiers.is_empty() {
            self.open_keybinding_stub();
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

    /// Enter on a row: appends a `[[binding]]` stub for its command to `keybindings.toml`
    /// (created from the template if it isn't there) and opens it, closing the view (VS Code
    /// switches to the file).
    pub(super) fn open_keybinding_stub(&mut self) {
        let Some((command, spec)) = self
            .keys
            .as_ref()
            .and_then(|v| v.selected_row())
            .map(|r| (r.command.name(), r.spec.clone()))
        else {
            return;
        };
        let path = self.config_dir.join(crate::config::keymap::FILE_NAME);
        let tx = self.tx.clone();
        self.keys = None;
        tokio::task::spawn_blocking(move || {
            let result = crate::config::keymap::create_if_missing(&path).and_then(|path| {
                crate::config::keymap::append_stub(&path, &command, &spec)?;
                Ok(path)
            });
            let _ = tx.send(AppEvent::KeybindingsReady(result));
        });
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
