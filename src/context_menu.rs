//! The context menu's model (no app state beyond the menu itself, no drawing; `app/context_menu.rs`
//! drives it, `ui/context_menu.rs` draws it). A menu is data: where it was opened and a list of
//! items, each a label and the `Command` it runs (or a separator). It knows nothing about what it
//! is a menu for; whoever opens it builds the list. A keybinding hint is not stored: the drawing
//! looks it up from the keymap. No submenus, icons, checkmarks or per-item callbacks.
//!
//! - Up / Down (`move_by`) go over the enabled items only, wrapping round; a disabled item or a
//!   separator is never selected by them or by the pointer (`select`), and runs nothing.
//! - `place` puts the box at the anchor, flipped to the other side of it (left, up) when it does
//!   not fit that way, and kept inside the screen.

// Nothing opens a menu yet outside tests (the tab and editor menus come with the next subtasks).
#![cfg_attr(not(test), allow(dead_code))]

use ratatui::layout::Rect;

use crate::command::Command;

/// One row of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    Item {
        label: String,
        command: Command,
        enabled: bool,
    },
    Separator,
}

impl MenuItem {
    /// An enabled item running `command`.
    pub fn new(label: impl Into<String>, command: Command) -> Self {
        MenuItem::Item {
            label: label.into(),
            command,
            enabled: true,
        }
    }

    /// This item dimmed and skipped (a separator is unchanged).
    pub fn disabled(mut self) -> Self {
        if let MenuItem::Item { enabled, .. } = &mut self {
            *enabled = false;
        }
        self
    }

    /// The command of an enabled item.
    pub fn runnable(&self) -> Option<Command> {
        match self {
            MenuItem::Item {
                command, enabled: true, ..
            } => Some(*command),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    /// The screen cell it was opened at (the pointer).
    pub anchor: (u16, u16),
    pub items: Vec<MenuItem>,
    /// The highlighted item: always an enabled one, unless there is none (then 0, running nothing).
    pub selected: usize,
}

impl ContextMenu {
    /// A menu with the first enabled item selected.
    pub fn new(anchor: (u16, u16), items: Vec<MenuItem>) -> Self {
        let selected = items.iter().position(|i| i.runnable().is_some()).unwrap_or(0);
        ContextMenu {
            anchor,
            items,
            selected,
        }
    }

    /// Moves the selection `delta` enabled items (wrapping round).
    pub fn move_by(&mut self, delta: isize) {
        let enabled: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.items[i].runnable().is_some())
            .collect();
        if enabled.is_empty() {
            return;
        }
        let at = enabled.iter().position(|&i| i == self.selected).unwrap_or(0) as isize;
        self.selected = enabled[(at + delta).rem_euclid(enabled.len() as isize) as usize];
    }

    /// Selects `index` if it is an enabled item (the pointer's hover); true if it is.
    pub fn select(&mut self, index: usize) -> bool {
        let ok = self.items.get(index).and_then(MenuItem::runnable).is_some();
        if ok {
            self.selected = index;
        }
        ok
    }

    /// The command Enter runs.
    pub fn selected_command(&self) -> Option<Command> {
        self.items.get(self.selected).and_then(MenuItem::runnable)
    }

    /// The command of the enabled item `index`.
    pub fn command_at(&self, index: usize) -> Option<Command> {
        self.items.get(index).and_then(MenuItem::runnable)
    }
}

/// The box of `width` x `height` for a menu opened at `anchor` on `screen`: below and right of the
/// anchor when that fits, else flipped above / left of it, else shifted to the screen's edge. Cut to
/// the screen's size when larger.
pub fn place(anchor: (u16, u16), width: u16, height: u16, screen: Rect) -> Rect {
    let (width, height) = (width.min(screen.width), height.min(screen.height));
    let fit = |at: u16, size: u16, start: u16, end: u16| {
        if at + size <= end {
            at
        } else if at >= start + size {
            at - size
        } else {
            end - size
        }
    };
    Rect {
        x: fit(anchor.0.max(screen.x), width, screen.x, screen.right()),
        y: fit(anchor.1.max(screen.y), height, screen.y, screen.bottom()),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> ContextMenu {
        ContextMenu::new(
            (3, 4),
            vec![
                MenuItem::new("Off", Command::Undo).disabled(),
                MenuItem::new("A", Command::Copy),
                MenuItem::Separator,
                MenuItem::new("B", Command::Cut).disabled(),
                MenuItem::new("C", Command::Paste),
            ],
        )
    }

    #[test]
    fn starts_on_the_first_enabled_item() {
        assert_eq!(menu().selected, 1);
        assert_eq!(menu().selected_command(), Some(Command::Copy));
    }

    #[test]
    fn up_and_down_skip_disabled_items_and_separators_and_wrap() {
        let mut m = menu();
        m.move_by(1);
        assert_eq!(m.selected, 4);
        m.move_by(1);
        assert_eq!(m.selected, 1);
        m.move_by(-1);
        assert_eq!(m.selected, 4);
    }

    #[test]
    fn select_takes_only_enabled_items() {
        let mut m = menu();
        assert!(!m.select(0) && !m.select(2) && !m.select(3) && !m.select(9));
        assert_eq!(m.selected, 1);
        assert!(m.select(4));
        assert_eq!(m.command_at(4), Some(Command::Paste));
        assert_eq!(m.command_at(3), None);
    }

    #[test]
    fn a_menu_with_nothing_enabled_runs_nothing() {
        let mut m = ContextMenu::new(
            (0, 0),
            vec![MenuItem::new("x", Command::Copy).disabled(), MenuItem::Separator],
        );
        m.move_by(1);
        assert_eq!(m.selected_command(), None);
    }

    #[test]
    fn place_flips_and_shifts_to_stay_on_screen() {
        let screen = Rect::new(0, 0, 40, 20);
        assert_eq!(place((5, 5), 10, 4, screen), Rect::new(5, 5, 10, 4));
        // Right edge: flips to the left of the anchor.
        assert_eq!(place((35, 5), 10, 4, screen), Rect::new(25, 5, 10, 4));
        // Bottom edge: flips above.
        assert_eq!(place((5, 18), 10, 4, screen), Rect::new(5, 14, 10, 4));
        // Neither side fits: against the edge.
        assert_eq!(place((5, 2), 10, 19, screen), Rect::new(5, 1, 10, 19));
        assert_eq!(place((3, 5), 38, 4, screen), Rect::new(2, 5, 38, 4));
        // Larger than the screen: cut to it.
        assert_eq!(place((5, 5), 50, 30, screen), Rect::new(0, 0, 40, 20));
    }
}
