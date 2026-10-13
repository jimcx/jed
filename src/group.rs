//! Editor groups and their tabs, laid out by a split tree.

use std::collections::HashMap;

use crate::buffer::BufferId;
use crate::layout::{GroupId, SplitDir, SplitTree};
use crate::view::View;

pub struct Tab {
    pub buffer: BufferId,
    pub view: View,
}

#[derive(Default)]
pub struct Group {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// First tab shown in the tab bar.
    pub scroll: usize,
}

impl Group {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn find(&self, buffer: BufferId) -> Option<usize> {
        self.tabs.iter().position(|t| t.buffer == buffer)
    }

    /// Opens a tab to the right of the active one and activates it.
    pub fn open(&mut self, buffer: BufferId, view: View) -> usize {
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(at, Tab { buffer, view });
        self.active = at;
        at
    }

    /// Removes a tab; the tab to its right (or left, at the end) becomes active.
    pub fn close(&mut self, index: usize) -> Tab {
        let tab = self.tabs.remove(index);
        if index < self.active || (index == self.active && self.active >= self.tabs.len()) {
            self.active = self.active.saturating_sub(1);
        }
        tab
    }

    pub fn cycle(&mut self, delta: isize) {
        if !self.tabs.is_empty() {
            let n = self.tabs.len() as isize;
            self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        }
    }
}

/// All editor groups. There is always at least one, and exactly one is active.
pub struct Groups {
    map: HashMap<GroupId, Group>,
    pub active: GroupId,
    pub tree: SplitTree,
    next: GroupId,
}

impl Default for Groups {
    fn default() -> Self {
        Self {
            map: HashMap::from([(0, Group::default())]),
            active: 0,
            tree: SplitTree::new(0),
            next: 1,
        }
    }
}

impl Groups {
    pub fn active_group(&self) -> &Group {
        &self.map[&self.active]
    }

    pub fn active_group_mut(&mut self) -> &mut Group {
        self.map.get_mut(&self.active).expect("active group exists")
    }

    pub fn get(&self, id: GroupId) -> Option<&Group> {
        self.map.get(&id)
    }

    pub fn get_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        self.map.get_mut(&id)
    }

    /// In reading order.
    pub fn ids(&self) -> Vec<GroupId> {
        self.tree.groups()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (GroupId, &mut Group)> {
        self.map.iter_mut().map(|(id, g)| (*id, g))
    }

    /// Tabs showing `buffer`, across all groups.
    pub fn tabs_showing(&self, buffer: BufferId) -> usize {
        self.map
            .values()
            .flat_map(|g| &g.tabs)
            .filter(|t| t.buffer == buffer)
            .count()
    }

    /// Splits the active group. The new group shows the active tab's buffer (same cursor and
    /// scroll) and becomes active.
    pub fn split(&mut self, dir: SplitDir) -> GroupId {
        let id = self.next;
        self.next += 1;
        let mut group = Group::default();
        if let Some(tab) = self.active_group().active_tab() {
            group.tabs.push(Tab {
                buffer: tab.buffer,
                view: tab.view.clone(),
            });
        }
        self.tree.split(self.active, dir, id);
        self.map.insert(id, group);
        self.active = id;
        id
    }

    /// Removes a group (never the last one). Its neighbour becomes active if it was.
    pub fn remove(&mut self, id: GroupId) -> bool {
        if self.map.len() <= 1 {
            return false;
        }
        let Some(focus) = self.tree.remove(id) else {
            return false;
        };
        self.map.remove(&id);
        if self.active == id {
            self.active = focus;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(n: u64) -> Group {
        let mut g = Group::default();
        for id in 0..n {
            g.tabs.push(Tab {
                buffer: id,
                view: View::new(),
            });
        }
        g
    }

    #[test]
    fn closing_moves_active_sensibly() {
        let mut g = group(4);
        g.active = 1;
        g.close(1);
        assert_eq!(g.tabs[g.active].buffer, 2, "right neighbour");
        g.active = 2;
        g.close(2);
        assert_eq!(g.tabs[g.active].buffer, 2, "left neighbour at the end");
        g.close(0);
        assert_eq!(g.tabs[g.active].buffer, 2, "closing to the left keeps the same tab");
    }

    #[test]
    fn open_inserts_right_of_active_and_cycle_wraps() {
        let mut g = group(3);
        g.active = 0;
        g.open(9, View::new());
        assert_eq!(g.tabs.iter().map(|t| t.buffer).collect::<Vec<_>>(), vec![0, 9, 1, 2]);
        assert_eq!(g.active, 1);
        g.cycle(-2);
        assert_eq!(g.active, 3);
    }

    #[test]
    fn split_copies_active_tab_and_remove_refocuses() {
        let mut gs = Groups::default();
        gs.active_group_mut().open(5, View::new());
        let first = gs.active;
        let second = gs.split(SplitDir::Right);
        assert_eq!(gs.active, second);
        assert_eq!(gs.active_group().tabs[0].buffer, 5);
        assert_eq!(gs.tabs_showing(5), 2);
        assert_eq!(gs.ids(), vec![first, second]);

        assert!(gs.remove(second));
        assert_eq!(gs.active, first);
        assert!(!gs.remove(first), "the last group stays");
        assert_eq!(gs.len(), 1);
    }
}
