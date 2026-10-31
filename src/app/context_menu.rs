//! The context menu (`app.context_menu`, a `context_menu::ContextMenu`; drawn by
//! `ui::context_menu`). `open_context_menu(anchor, items)` is the one way in: whatever builds the
//! item list (the next subtasks add the tab and editor menus) calls it; this module knows nothing
//! about what the items are for.
//!
//! - While open it takes every key as `Context::ContextMenu`: Up / Down / Enter / Esc are keymap
//!   data (`Command::ContextMenu`), anything else is swallowed (it neither closes the menu nor
//!   reaches the editor); a paste is swallowed too.
//! - Enter or a left click on an enabled item closes the menu, then runs the item's command
//!   through `execute`. A click on a disabled item or a separator does nothing (the menu stays).
//! - Pointer: moving over an enabled item selects it. A press outside closes the menu and is
//!   consumed (it does not reach what is under it); so is a wheel scroll anywhere. A focus loss
//!   (`Event::FocusLost`) and any command run by other means (`execute`) close it too. No hover
//!   popup or tab tooltip starts while it is open.

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use super::App;
use crate::command::{Command, ContextMenuCommand};
use crate::context_menu::{ContextMenu, MenuItem};
use crate::layout::Hit;

impl App {
    #[cfg_attr(not(test), allow(dead_code))] // until the tab / editor menus open one
    /// Opens a menu of `items` at the screen cell `anchor` (the pointer), replacing any open one.
    pub fn open_context_menu(&mut self, anchor: (u16, u16), items: Vec<MenuItem>) {
        self.close_hover();
        self.close_peek();
        self.close_code_actions();
        self.clear_tab_tip();
        self.context_menu = Some(ContextMenu::new(anchor, items));
        self.needs_redraw = true;
    }

    pub(super) fn close_context_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.needs_redraw = true;
        }
    }

    /// A key while the menu is open: its own bindings only.
    pub(super) fn context_menu_key(&mut self, key: KeyEvent) {
        if let Some(cmd @ Command::ContextMenu(_)) = self.keymap.lookup(&key, super::Context::ContextMenu) {
            self.execute(cmd);
        }
    }

    pub(super) fn context_menu_command(&mut self, c: ContextMenuCommand) {
        let Some(menu) = &mut self.context_menu else { return };
        match c {
            ContextMenuCommand::Up => menu.move_by(-1),
            ContextMenuCommand::Down => menu.move_by(1),
            ContextMenuCommand::Accept => {
                let cmd = menu.selected_command();
                self.run_menu_command(cmd);
            }
            ContextMenuCommand::Close => self.close_context_menu(),
        }
        self.needs_redraw = true;
    }

    /// Closes the menu, then runs `cmd` (when there is one).
    fn run_menu_command(&mut self, cmd: Option<Command>) {
        if let Some(cmd) = cmd {
            self.close_context_menu();
            self.execute(cmd);
        }
    }

    /// Every mouse event while the menu is open; true when consumed (all of them are).
    pub(super) fn context_menu_mouse(&mut self, m: &MouseEvent, hit: Hit) -> bool {
        let Some(menu) = &mut self.context_menu else {
            return false;
        };
        let item = (hit == Hit::ContextMenu)
            .then(|| self.hits.context_menu.and_then(|h| h.item_at(m.row)))
            .flatten();
        match m.kind {
            MouseEventKind::Moved => {
                if let Some(i) = item
                    && menu.select(i)
                {
                    self.needs_redraw = true;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if item.is_some() => {
                let cmd = item.and_then(|i| menu.command_at(i));
                self.run_menu_command(cmd);
            }
            MouseEventKind::Down(_) if hit != Hit::ContextMenu => self.close_context_menu(),
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => self.close_context_menu(),
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_commands_are_listed_with_the_palette_commands() {
        assert!(Command::all().contains(&Command::ContextMenu(ContextMenuCommand::Accept)));
    }
}
