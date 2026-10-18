//! Editor groups and their tabs, laid out by a split tree.

use std::collections::HashMap;

use crate::buffer::BufferId;
use crate::diff_view::DiffTab;
use crate::layout::{GroupId, SplitDir, SplitTree};
use crate::view::View;

/// A text editor tab: a buffer and this tab's view of it.
pub struct TextTab {
    pub buffer: BufferId,
    pub view: View,
}

/// What a tab shows: a buffer in a text editor (a log's or a commit's too, read only), or a diff
/// view (two texts side by side, read only). Commands that only make sense in a text editor find
/// no `active_tab` on a diff tab. The diff is boxed; text tabs, most of them, stay inline.
#[allow(clippy::large_enum_variant)]
pub enum Tab {
    Text(TextTab),
    Diff(Box<DiffTab>),
}

impl Tab {
    pub fn text(&self) -> Option<&TextTab> {
        match self {
            Tab::Text(t) => Some(t),
            Tab::Diff(_) => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextTab> {
        match self {
            Tab::Text(t) => Some(t),
            Tab::Diff(_) => None,
        }
    }

    pub fn diff(&self) -> Option<&DiffTab> {
        match self {
            Tab::Diff(d) => Some(d),
            Tab::Text(_) => None,
        }
    }

    pub fn diff_mut(&mut self) -> Option<&mut DiffTab> {
        match self {
            Tab::Diff(d) => Some(d),
            Tab::Text(_) => None,
        }
    }

    /// The buffer a text tab shows (`None` for a diff tab).
    pub fn buffer(&self) -> Option<BufferId> {
        self.text().map(|t| t.buffer)
    }
}

#[derive(Default)]
pub struct Group {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// First tab shown in the tab bar.
    pub scroll: usize,
}

impl Group {
    /// The active tab, when it's a text editor.
    pub fn active_tab(&self) -> Option<&TextTab> {
        self.tabs.get(self.active).and_then(Tab::text)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TextTab> {
        self.tabs.get_mut(self.active).and_then(Tab::text_mut)
    }

    /// The active tab, when it's a diff view.
    pub fn active_diff(&self) -> Option<&DiffTab> {
        self.tabs.get(self.active).and_then(Tab::diff)
    }

    pub fn active_diff_mut(&mut self) -> Option<&mut DiffTab> {
        self.tabs.get_mut(self.active).and_then(Tab::diff_mut)
    }

    /// The text tab showing `buffer`.
    pub fn find(&self, buffer: BufferId) -> Option<usize> {
        self.tabs.iter().position(|t| t.buffer() == Some(buffer))
    }

    /// Text tabs, with their indices.
    pub fn text_tabs_mut(&mut self) -> impl Iterator<Item = (usize, &mut TextTab)> {
        self.tabs
            .iter_mut()
            .enumerate()
            .filter_map(|(i, t)| Some((i, t.text_mut()?)))
    }

    /// Opens a text tab to the right of the active one and activates it.
    pub fn open(&mut self, buffer: BufferId, view: View) -> usize {
        self.open_tab(Tab::Text(TextTab { buffer, view }))
    }

    /// Opens any tab to the right of the active one and activates it.
    pub fn open_tab(&mut self, tab: Tab) -> usize {
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(at, tab);
        self.active = at;
        at
    }

    /// Opens a tab to the right of the active one, keeping the active one (the first tab of an
    /// empty group is active). Rename's edits open files this way.
    pub fn open_background(&mut self, buffer: BufferId, view: View) -> usize {
        if self.tabs.is_empty() {
            return self.open(buffer, view);
        }
        let at = self.active + 1;
        self.tabs.insert(at, Tab::Text(TextTab { buffer, view }));
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
            .filter(|t| t.buffer() == Some(buffer))
            .count()
    }

    /// Every diff tab, in any group.
    pub fn diff_tabs_mut(&mut self) -> impl Iterator<Item = (GroupId, &mut DiffTab)> {
        self.map
            .iter_mut()
            .flat_map(|(id, g)| g.tabs.iter_mut().filter_map(move |t| Some((*id, t.diff_mut()?))))
    }

    /// Splits the active group. The new group shows the active tab's buffer (same cursor and
    /// scroll), or a copy of its diff view (`diff_id`: the copy's id), and becomes active.
    pub fn split(&mut self, dir: SplitDir, diff_id: u64) -> GroupId {
        let tab = match self.active_group().tabs.get(self.active_group().active) {
            Some(Tab::Text(tab)) => Some(Tab::Text(TextTab {
                buffer: tab.buffer,
                view: tab.view.clone(),
            })),
            Some(Tab::Diff(d)) => Some(Tab::Diff(Box::new(d.copy(diff_id)))),
            None => None,
        };
        let id = self.split_empty(dir);
        if let Some(tab) = tab {
            self.map.get_mut(&id).expect("just split").tabs.push(tab);
        }
        id
    }

    /// Splits the active group into a new group with no tabs, which becomes active.
    pub fn split_empty(&mut self, dir: SplitDir) -> GroupId {
        let id = self.next;
        self.next += 1;
        self.tree.split(self.active, dir, id);
        self.map.insert(id, Group::default());
        self.active = id;
        id
    }

    /// Groups restored from a session: `tree`'s leaves must be exactly `groups`' ids (each
    /// once), and `active` one of them, else `Err` and nothing changes. Group ids after the
    /// largest are given to new groups.
    pub fn restore(
        &mut self,
        mut tree: SplitTree,
        groups: Vec<(GroupId, Group)>,
        active: GroupId,
    ) -> Result<(), String> {
        let ids: Vec<GroupId> = groups.iter().map(|(id, _)| *id).collect();
        check_restore(&tree, &ids, active)?;
        tree.sanitize();
        self.next = ids.iter().max().map_or(0, |m| m + 1);
        self.map = groups.into_iter().collect();
        self.tree = tree;
        self.active = active;
        Ok(())
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

/// Whether `Groups::restore` would take groups `ids` laid out by `tree` with `active` active:
/// the tree's leaves are exactly `ids`, each once, and `active` is one of them.
pub fn check_restore(tree: &SplitTree, ids: &[GroupId], active: GroupId) -> Result<(), String> {
    let mut leaves = tree.groups();
    let mut ids = ids.to_vec();
    leaves.sort_unstable();
    ids.sort_unstable();
    if leaves.is_empty() || leaves != ids || ids.windows(2).any(|w| w[0] == w[1]) {
        return Err(format!("groups {ids:?} don't match the split tree's {leaves:?}"));
    }
    if !ids.contains(&active) {
        return Err(format!("active group {active} isn't one of {ids:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(n: u64) -> Group {
        let mut g = Group::default();
        for id in 0..n {
            g.tabs.push(Tab::Text(TextTab {
                buffer: id,
                view: View::new(),
            }));
        }
        g
    }

    #[test]
    fn closing_moves_active_sensibly() {
        let mut g = group(4);
        g.active = 1;
        g.close(1);
        assert_eq!(g.tabs[g.active].buffer(), Some(2), "right neighbour");
        g.active = 2;
        g.close(2);
        assert_eq!(g.tabs[g.active].buffer(), Some(2), "left neighbour at the end");
        g.close(0);
        assert_eq!(
            g.tabs[g.active].buffer(),
            Some(2),
            "closing to the left keeps the same tab"
        );
    }

    #[test]
    fn background_tabs_keep_the_active_one() {
        let mut g = Group::default();
        assert_eq!(g.open_background(7, View::new()), 0);
        assert_eq!(g.active, 0, "the first tab of an empty group");
        g.open(8, View::new());
        g.active = 0;
        assert_eq!(g.open_background(9, View::new()), 1);
        assert_eq!(g.active, 0);
        assert_eq!(g.tabs.iter().filter_map(Tab::buffer).collect::<Vec<_>>(), [7, 9, 8]);
    }

    #[test]
    fn open_inserts_right_of_active_and_cycle_wraps() {
        let mut g = group(3);
        g.active = 0;
        g.open(9, View::new());
        assert_eq!(
            g.tabs.iter().filter_map(Tab::buffer).collect::<Vec<_>>(),
            vec![0, 9, 1, 2]
        );
        assert_eq!(g.active, 1);
        g.cycle(-2);
        assert_eq!(g.active, 3);
    }

    #[test]
    fn restore_checks_the_tree_against_the_groups() {
        let mut gs = Groups::default();
        gs.split(SplitDir::Right, 0);
        gs.split(SplitDir::Down, 0);
        let tree = gs.tree.clone();
        let ids = gs.ids();
        let groups = |ids: &[GroupId]| ids.iter().map(|&id| (id, group(1))).collect::<Vec<_>>();

        let mut fresh = Groups::default();
        assert!(
            fresh.restore(tree.clone(), groups(&ids[..2]), ids[0]).is_err(),
            "a leaf without a group"
        );
        assert!(
            fresh
                .restore(tree.clone(), groups(&[ids[0], ids[1], ids[2], 9]), ids[0])
                .is_err()
        );
        assert!(
            fresh
                .restore(tree.clone(), groups(&[ids[0], ids[1], ids[1]]), ids[0])
                .is_err()
        );
        assert!(
            fresh.restore(tree.clone(), groups(&ids), 9).is_err(),
            "active not a group"
        );
        assert_eq!((fresh.len(), fresh.ids()), (1, vec![0]), "unchanged");

        fresh.restore(tree.clone(), groups(&ids), ids[1]).unwrap();
        assert_eq!((fresh.ids(), fresh.active, fresh.len()), (ids.clone(), ids[1], 3));
        assert_eq!(fresh.tree, tree);
        let new = fresh.split(SplitDir::Right, 0);
        assert!(!ids.contains(&new), "new groups get new ids");
    }

    #[test]
    fn split_copies_active_tab_and_remove_refocuses() {
        let mut gs = Groups::default();
        gs.active_group_mut().open(5, View::new());
        let first = gs.active;
        let second = gs.split(SplitDir::Right, 0);
        assert_eq!(gs.active, second);
        assert_eq!(gs.active_group().tabs[0].buffer(), Some(5));
        assert_eq!(gs.tabs_showing(5), 2);
        assert_eq!(gs.ids(), vec![first, second]);

        assert!(gs.remove(second));
        assert_eq!(gs.active, first);
        assert!(!gs.remove(first), "the last group stays");
        assert_eq!(gs.len(), 1);
    }
}
