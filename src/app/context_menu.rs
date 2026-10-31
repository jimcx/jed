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

/// What a menu is opened on. `menu_for` is the only place that lists items: a new target adds a
/// variant and an arm, a new item one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTarget {
    /// A tab; `has_file` is false for an untitled buffer, a shell, a log or a commit tab.
    Tab { has_file: bool },
}

/// The items of the menu for `target`. They are the active file's commands, so whoever opens the
/// menu makes the target's tab the active one first.
pub fn menu_for(target: MenuTarget) -> Vec<MenuItem> {
    let MenuTarget::Tab { has_file } = target;
    let reveal = if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else {
        "Open Containing Folder"
    };
    let file_item = |label: &str, command| {
        let item = MenuItem::new(label, command);
        if has_file { item } else { item.disabled() }
    };
    vec![
        file_item("Copy Relative Path", Command::CopyRelativePath),
        file_item("Copy Path", Command::CopyPath),
        file_item(reveal, Command::RevealInFinder),
        MenuItem::Separator,
        file_item("File History: Show", Command::ShowFileHistory),
    ]
}

impl App {
    /// A right-click at `at` on tab `index` of group `g`: focus and activate it, open its menu
    /// under the pointer (on the row below, as VS Code does, so the tab bar stays visible and
    /// another tab's right-click lands on that tab).
    pub(super) fn open_tab_menu(&mut self, g: crate::layout::GroupId, index: usize, at: (u16, u16)) {
        self.focus_group(g);
        self.activate_tab(index);
        let has_file = self.tab_path_text(g, index).is_some();
        self.open_context_menu((at.0, at.1.saturating_add(1)), menu_for(MenuTarget::Tab { has_file }));
    }

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
            // A right press elsewhere closes the menu and acts as usual (a tab's right-click
            // opens its own menu in its place).
            MouseEventKind::Down(MouseButton::Right) if hit != Hit::ContextMenu => {
                self.close_context_menu();
                return false;
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

    #[test]
    fn tab_menu_items_follow_has_file() {
        let enabled = |has_file| -> Vec<(String, bool)> {
            menu_for(MenuTarget::Tab { has_file })
                .iter()
                .filter_map(|i| match i {
                    MenuItem::Item { label, enabled, .. } => Some((label.clone(), *enabled)),
                    MenuItem::Separator => None,
                })
                .collect()
        };
        let with = enabled(true);
        assert_eq!(with.len(), 4);
        assert!(with.iter().all(|(_, e)| *e));
        assert_eq!(with[0].0, "Copy Relative Path");
        assert_eq!(with[3].0, "File History: Show");
        assert!(enabled(false).iter().all(|(_, e)| !*e));
    }
}
