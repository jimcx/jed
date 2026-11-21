//! The tab menu's behaviour (`menu` holds its state, `ui_menu` draws it): opening and closing it,
//! its keys, and picking an item with a tap. While it is open every pointer event is its own: a
//! press on an item, on `☰` or outside it is a pending `Drag::Press` (`app/touch.rs`), and its tap
//! acts (`menu_tap`), so a plain client's click picks too.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::{App, WHEEL_LINES};
use crate::command::MenuCommand;
use crate::hit::Hit;
use crate::menu::{self, Item, Menu, Target};

impl App {
    /// The menu's items: the focused group's tabs, then `menu::actions()`.
    pub(crate) fn menu_items(&self) -> Vec<Item> {
        let group = self.groups.active_group();
        let tabs = group.tabs.iter().enumerate().map(|(i, t)| Item {
            label: t.title.clone(),
            target: Target::Tab(i),
            active: i == group.active,
        });
        let actions = self.menu_actions.iter().copied().map(|(label, command)| Item {
            label: label.to_string(),
            target: Target::Action(command),
            active: false,
        });
        tabs.chain(actions).collect()
    }

    pub(super) fn menu_command(&mut self, command: MenuCommand) {
        match command {
            MenuCommand::Toggle => {
                self.menu = if self.menu.is_some() {
                    None
                } else {
                    Some(Menu::default())
                }
            }
            MenuCommand::Close => self.menu = None,
            MenuCommand::Up | MenuCommand::Down => self.menu_move(command == MenuCommand::Down),
            MenuCommand::Pick => {
                if let Some(i) = self.menu.as_ref().and_then(|m| m.highlight) {
                    self.menu_pick(i);
                } else {
                    self.menu = None;
                }
            }
        }
    }

    /// The highlight one item on (a key moves from the active tab at first), scrolled into view.
    fn menu_move(&mut self, forward: bool) {
        let items = self.menu_items();
        let active = items.iter().position(|i| i.active).unwrap_or(0);
        let tabs = items.iter().filter(|i| matches!(i.target, Target::Tab(_))).count();
        let Some(m) = &mut self.menu else {
            return;
        };
        let at = m.highlight.unwrap_or(active);
        let to = if forward {
            (at + 1).min(items.len().saturating_sub(1))
        } else {
            at.saturating_sub(1)
        };
        m.highlight = Some(to);
        m.reveal(&menu::rows(tabs, items.len()), to);
    }

    /// Picks item `i`: closes the menu, then focuses the tab or runs the action's command.
    fn menu_pick(&mut self, i: usize) {
        self.menu = None;
        let Some(item) = self.menu_items().into_iter().nth(i) else {
            return;
        };
        match item.target {
            Target::Tab(index) => {
                let group = self.groups.active;
                if let Some(g) = self.groups.get_mut(group) {
                    g.active = index;
                }
            }
            Target::Action(command) => self.execute(command),
        }
    }

    /// A pointer event while the menu is open: a left press starts a pending press (on `☰`, an
    /// item, its separator, or outside); the wheel over the menu scrolls it; nothing else matters.
    pub(super) fn menu_pointer(&mut self, m: MouseEvent) {
        let on_menu = matches!(self.hit.at(m.column, m.row), Some(Hit::MenuItem(_) | Hit::MenuDead));
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = match self.hit.at(m.column, m.row) {
                    Some(h @ (Hit::Menu | Hit::MenuItem(_) | Hit::MenuDead)) => h,
                    _ => Hit::MenuOutside,
                };
                self.touch_press(hit, m);
            }
            MouseEventKind::Down(_) if !on_menu => self.menu = None,
            MouseEventKind::ScrollUp if on_menu => self.menu_scroll(-WHEEL_LINES),
            MouseEventKind::ScrollDown if on_menu => self.menu_scroll(WHEEL_LINES),
            _ => {}
        }
    }

    fn menu_scroll(&mut self, rows: isize) {
        if let Some(menu) = &mut self.menu {
            menu.scroll_by(rows as i32);
        }
    }

    /// A tap (touch or click) while the menu is open, on `hit`.
    pub(super) fn menu_tap(&mut self, hit: Hit) {
        match hit {
            Hit::Menu => self.menu_command(MenuCommand::Toggle),
            Hit::MenuOutside => self.menu = None,
            Hit::MenuItem(i) => self.menu_pick(i),
            _ => {}
        }
    }
}
