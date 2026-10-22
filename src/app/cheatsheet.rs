//! The keyboard cheat sheet overlay (`app.cheat_sheet`, model in `crate::cheatsheet`): opening
//! and closing it, and while it's open every key and mouse event (only a prompt comes first).

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use super::App;
use super::mouse::{Drag, WHEEL_LINES};
use crate::cheatsheet::CheatSheet;
use crate::command::Command;
use crate::layout::{Hit, ScrollTarget};
use crate::scrollbar::Scrollable;
use crate::terminal::Pointer;

impl App {
    /// "Help: Keyboard Cheat Sheet": opens it over the editor area with the current context's
    /// keys first, or closes it.
    pub(super) fn toggle_cheat_sheet(&mut self) {
        if self.cheat_sheet.take().is_none() {
            self.cheat_sheet = Some(CheatSheet::new(self.context()));
        }
    }

    /// A key while it's open: Esc or its own key closes it, the arrows / pages / Home / End
    /// scroll it, anything else is swallowed.
    pub(super) fn cheat_sheet_key(&mut self, key: KeyEvent) {
        let context = self.context();
        let Some(sheet) = &mut self.cheat_sheet else { return };
        if (key.code == KeyCode::Esc && key.modifiers.is_empty())
            || self.keymap.lookup(&key, context) == Some(Command::CheatSheet)
        {
            self.cheat_sheet = None;
            return;
        }
        if !key.modifiers.is_empty() {
            return;
        }
        let page = sheet.height.max(1) as isize;
        match key.code {
            KeyCode::Up => sheet.scroll_by(-1),
            KeyCode::Down => sheet.scroll_by(1),
            KeyCode::PageUp => sheet.scroll_by(-page),
            KeyCode::PageDown => sheet.scroll_by(page),
            KeyCode::Home => sheet.set_top(0),
            KeyCode::End => sheet.set_top(usize::MAX),
            _ => {}
        }
    }

    /// A mouse event while it's open: the wheel scrolls it, its bar drags, a press outside
    /// closes it (and does nothing else).
    pub(super) fn cheat_sheet_mouse(&mut self, m: MouseEvent) {
        self.pointer = Pointer::Default;
        let target = ScrollTarget::CheatSheet;
        let Some(hit) = self.hits.cheat_sheet else {
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
                if let Some(sheet) = &mut self.cheat_sheet {
                    sheet.scroll_by(delta);
                }
            }
            MouseEventKind::Down(_) if !hit.rect.contains(p) => self.cheat_sheet = None,
            MouseEventKind::Down(MouseButton::Left) if on_bar => self.scrollbar_click(target, m.row),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(Drag::ScrollBar { target, grab }) = self.drag {
                    self.scrollbar_drag(target, m.row, grab);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
        let over = if on_bar { Hit::ScrollBar(target) } else { Hit::Nothing };
        self.scrollbar_hover(&m, over);
    }
}
