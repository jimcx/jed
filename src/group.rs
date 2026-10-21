//! Editor groups and their tabs, laid out by a split tree.

use std::collections::HashMap;

use crate::buffer::BufferId;
use crate::diff_view::DiffTab;
use crate::hex::HexTab;
use crate::layout::{GroupId, SplitDir, SplitTree};
use crate::view::View;

/// A text editor tab: a buffer and this tab's view of it.
pub struct TextTab {
    pub buffer: BufferId,
    pub view: View,
}

/// What a tab shows: a buffer in a text editor (a log's or a commit's too, read only), a diff
/// view (two texts side by side, read only), or a file's bytes in a hex view (read only).
/// Commands that only make sense in a text editor find no `active_tab` on a diff or hex tab. The
/// diff and hex tabs are boxed; text tabs, most of them, stay inline.
#[allow(clippy::large_enum_variant)]
pub enum Tab {
    Text(TextTab),
    Diff(Box<DiffTab>),
    Hex(Box<HexTab>),
}

impl Tab {
    pub fn text(&self) -> Option<&TextTab> {
        match self {
            Tab::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextTab> {
        match self {
            Tab::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn diff(&self) -> Option<&DiffTab> {
        match self {
            Tab::Diff(d) => Some(d),
            _ => None,
        }
    }

    pub fn diff_mut(&mut self) -> Option<&mut DiffTab> {
        match self {
            Tab::Diff(d) => Some(d),
            _ => None,
        }
    }

    pub fn hex(&self) -> Option<&HexTab> {
        match self {
            Tab::Hex(h) => Some(h),
            _ => None,
        }
    }

    pub fn hex_mut(&mut self) -> Option<&mut HexTab> {
        match self {
            Tab::Hex(h) => Some(h),
            _ => None,
        }
    }

    /// The buffer a text tab shows (`None` for a diff or hex tab).
    pub fn buffer(&self) -> Option<BufferId> {
        self.text().map(|t| t.buffer)
    }

    /// Shows the same thing as `other`: the same buffer, a diff of the same file and sides, or
    /// the same file's bytes. A group never has two such tabs (opening one shows the other).
    pub fn same_as(&self, other: &Tab) -> bool {
        match (self, other) {
            (Tab::Text(a), Tab::Text(b)) => a.buffer == b.buffer,
            (Tab::Diff(a), Tab::Diff(b)) => a.path == b.path && a.pair == b.pair,
            (Tab::Hex(a), Tab::Hex(b)) => a.path == b.path,
            _ => false,
        }
    }
}

#[derive(Default)]
pub struct Group {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// First tab shown in the tab bar.
    pub scroll: usize,
    /// The active tab index and tab bar width drawing last scrolled into view: the bar reveals
    /// the active tab only when one of them changes, so the wheel and the bar's arrows can
    /// scroll it away from the active tab.
    pub revealed: Option<(usize, u16)>,
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

    /// The active tab, when it's a hex view.
    pub fn active_hex(&self) -> Option<&HexTab> {
        self.tabs.get(self.active).and_then(Tab::hex)
    }

    pub fn active_hex_mut(&mut self) -> Option<&mut HexTab> {
        self.tabs.get_mut(self.active).and_then(Tab::hex_mut)
    }

    /// The hex tab of `path`.
    pub fn find_hex(&self, path: &std::path::Path) -> Option<usize> {
        self.tabs.iter().position(|t| t.hex().is_some_and(|h| h.path == path))
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

    /// Takes in a tab moved from another group and activates it: it replaces a tab showing the
    /// same thing (`Tab::same_as`; VS Code shows the one already there, with the moved view),
    /// else it opens to the right of the active one.
    pub fn take(&mut self, tab: Tab) -> usize {
        match self.tabs.iter().position(|t| t.same_as(&tab)) {
            Some(i) => {
                self.tabs[i] = tab;
                self.active = i;
                i
            }
            None => self.open_tab(tab),
        }
    }

    /// Swaps the active tab with its neighbour `delta` (±1) places along; it stays active.
    /// Nothing at either end (no wrap, as VS Code). False when nothing moved.
    pub fn move_active(&mut self, delta: isize) -> bool {
        let Some(to) = self.active.checked_add_signed(delta).filter(|&to| to < self.tabs.len()) else {
            return false;
        };
        if self.active >= self.tabs.len() {
            return false;
        }
        self.tabs.swap(self.active, to);
        self.active = to;
        true
    }

    pub fn cycle(&mut self, delta: isize) {
        if !self.tabs.is_empty() {
            let n = self.tabs.len() as isize;
            self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// Moves the tab bar by `delta` tabs (its arrows and the wheel). Drawing clamps it to the
    /// furthest the tabs fit.
    pub fn scroll_tabs(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta);
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

    /// Every hex tab, in any group.
    pub fn hex_tabs(&self) -> impl Iterator<Item = &HexTab> {
        self.map.values().flat_map(|g| g.tabs.iter().filter_map(Tab::hex))
    }

    /// Every hex tab, in any group.
    pub fn hex_tabs_mut(&mut self) -> impl Iterator<Item = &mut HexTab> {
        self.map
            .values_mut()
            .flat_map(|g| g.tabs.iter_mut().filter_map(Tab::hex_mut))
    }

    /// Every diff tab, in any group.
    pub fn diff_tabs_mut(&mut self) -> impl Iterator<Item = (GroupId, &mut DiffTab)> {
        self.map
            .iter_mut()
            .flat_map(|(id, g)| g.tabs.iter_mut().filter_map(move |t| Some((*id, t.diff_mut()?))))
    }

    /// Moves the active group's active tab (its view as it is) into the next group in reading
    /// order (`forward`) or the previous one, which becomes active; a group left without tabs
    /// closes. With no next group it goes into a new group split to the right (VS Code), unless
    /// it's its group's only tab: that would end as it began, so nothing happens. With no
    /// previous group nothing happens. False when nothing moved.
    pub fn move_active_tab(&mut self, forward: bool) -> bool {
        let from = self.active;
        let source = self.active_group();
        if source.active >= source.tabs.len() {
            return false;
        }
        let only = source.tabs.len() == 1;
        let ids = self.ids();
        let Some(here) = ids.iter().position(|&g| g == from) else {
            return false;
        };
        let next = if forward {
            ids.get(here + 1).copied()
        } else {
            here.checked_sub(1).map(|i| ids[i])
        };
        let to = match next {
            Some(to) => to,
            None if forward && !only => self.split_empty(SplitDir::Right),
            None => return false,
        };
        let source = self.map.get_mut(&from).expect("source group exists");
        let index = source.active;
        let tab = source.close(index);
        let empty = source.tabs.is_empty();
        self.map.get_mut(&to).expect("target group exists").take(tab);
        self.active = to;
        if empty {
            self.remove(from);
        }
        true
    }

    /// Splits the active group. The new group shows the active tab's buffer (same cursor and
    /// scroll), or a copy of its diff view (`diff_id`: the copy's id) or hex view (the bytes
    /// shared), and becomes active.
    pub fn split(&mut self, dir: SplitDir, diff_id: u64) -> GroupId {
        let tab = match self.active_group().tabs.get(self.active_group().active) {
            Some(Tab::Text(tab)) => Some(Tab::Text(TextTab {
                buffer: tab.buffer,
                view: tab.view.clone(),
            })),
            Some(Tab::Diff(d)) => Some(Tab::Diff(Box::new(d.copy(diff_id)))),
            Some(Tab::Hex(h)) => Some(Tab::Hex(h.clone())),
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

    fn buffers(g: &Group) -> Vec<BufferId> {
        g.tabs.iter().filter_map(Tab::buffer).collect()
    }

    #[test]
    fn move_active_swaps_with_a_neighbour_without_wrapping() {
        let mut g = group(3);
        g.active = 0;
        assert!(!g.move_active(-1), "no wrap at the start");
        assert!(g.move_active(1));
        assert_eq!((buffers(&g), g.active), (vec![1, 0, 2], 1));
        assert!(g.move_active(1));
        assert!(!g.move_active(1), "no wrap at the end");
        assert_eq!((buffers(&g), g.active), (vec![1, 2, 0], 2));
        assert!(!Group::default().move_active(1));
    }

    #[test]
    fn take_opens_right_of_active_or_replaces_the_same_buffer() {
        let mut g = group(3);
        g.active = 0;
        assert_eq!(
            g.take(Tab::Text(TextTab {
                buffer: 7,
                view: View::new()
            })),
            1
        );
        assert_eq!(buffers(&g), [0, 7, 1, 2]);
        let mut view = View::new();
        view.left_col = 4;
        assert_eq!(g.take(Tab::Text(TextTab { buffer: 2, view })), 3);
        assert_eq!((buffers(&g), g.active), (vec![0, 7, 1, 2], 3));
        assert_eq!(g.active_tab().unwrap().view.left_col, 4, "the moved view");
    }

    #[test]
    fn moving_a_tab_between_groups() {
        let mut gs = Groups::default();
        assert!(!gs.move_active_tab(true), "no tab");
        gs.active_group_mut().open(1, View::new());
        assert!(!gs.move_active_tab(true), "the only tab of the only group: nothing");
        assert!(!gs.move_active_tab(false));
        gs.active_group_mut().open(2, View::new());
        let first = gs.active;

        assert!(gs.move_active_tab(true), "no next group: splits right");
        let second = gs.active;
        assert_ne!(second, first);
        assert_eq!(gs.ids(), [first, second]);
        assert_eq!(buffers(gs.get(first).unwrap()), [1]);
        assert_eq!(buffers(gs.active_group()), [2]);
        assert!(
            !gs.move_active_tab(true),
            "the new group's only tab, nothing to the right"
        );

        assert!(gs.move_active_tab(false), "back to the first group");
        assert_eq!((gs.active, gs.len()), (first, 1), "the empty group closed");
        assert_eq!((buffers(gs.active_group()), gs.active_group().active), (vec![1, 2], 1));
        assert!(!gs.move_active_tab(false), "no previous group");

        // Into a group already showing the buffer: that tab is replaced, not duplicated.
        let third = gs.split(SplitDir::Right, 0);
        assert_eq!(buffers(gs.active_group()), [2]);
        gs.active = first;
        assert!(gs.move_active_tab(true));
        assert_eq!(gs.active, third);
        assert_eq!(buffers(gs.active_group()), [2]);
        assert_eq!(buffers(gs.get(first).unwrap()), [1]);
    }
}
