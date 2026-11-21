//! Groups of shell tabs in a split tree. A `Group` is a tab bar's worth of shells; `Groups` lays
//! them out with `jtui::split::SplitTree` and knows which one has
//! the focus. Closing a tab drops its `ShellTab`, which hangs the shell up; a group left without
//! tabs leaves the tree, and the last tab closing leaves the session without any.

use jterm::run::RunId;
use jterm::shell::ShellTab;
use jtui::split::{Divider, DropSide, GroupId, SplitDir, SplitTree};
use ratatui::layout::Rect;

/// Tabs side by side in one bar, one of them active.
#[derive(Default)]
pub struct Group {
    pub tabs: Vec<ShellTab>,
    pub active: usize,
    /// The first tab the bar shows when the tabs overflow it.
    pub scroll: usize,
    /// Columns of tab `scroll` hidden past the bar's left edge: a touch swipe moves the bar by
    /// columns, so it can stop part way through a tab. The wheel and the arrows clear it.
    pub shift: u16,
    /// The (active tab, bar width) the bar last scrolled to reveal; the bar keeps its own scroll
    /// (wheel, arrows) until one of them changes.
    pub revealed: Option<(usize, u16)>,
}

impl Group {
    pub fn active_tab(&self) -> Option<&ShellTab> {
        self.tabs.get(self.active)
    }

    /// Adds `tab` after the active one and activates it.
    pub fn open(&mut self, tab: ShellTab) {
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(at, tab);
        self.active = at;
    }

    /// Removes tab `index`; the one after it (else before) becomes active when it was.
    pub fn close(&mut self, index: usize) -> Option<ShellTab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        Some(tab)
    }

    pub fn position(&self, run: RunId) -> Option<usize> {
        self.tabs.iter().position(|t| t.run.id == run)
    }

    /// Moves the tab at `from` to `to`, its index afterwards; it becomes active. False when either
    /// index is out of range or it wouldn't move.
    pub fn move_tab(&mut self, from: usize, to: usize) -> bool {
        let len = self.tabs.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = to;
        true
    }

    /// The next (`delta` 1) or previous tab, wrapping.
    pub fn cycle(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// Tab `n` (1-based); nothing past the end.
    pub fn go_to(&mut self, n: usize) {
        if n >= 1 && n <= self.tabs.len() {
            self.active = n - 1;
        }
    }

    /// Scrolls the tab bar by `delta` tabs (the bar clamps it to what fits).
    /// A tab back from part way through one first shows that tab whole.
    pub fn scroll_tabs(&mut self, mut delta: isize) {
        if delta < 0 && self.shift > 0 {
            delta += 1;
        }
        self.shift = 0;
        let last = self.tabs.len().saturating_sub(1);
        self.scroll = self.scroll.saturating_add_signed(delta).min(last);
    }

    /// The bar's scroll in columns, given its tabs' `widths`: those before `scroll`, plus `shift`.
    pub fn scroll_offset(&self, widths: &[u32]) -> u32 {
        widths.iter().take(self.scroll).sum::<u32>() + u32::from(self.shift)
    }

    /// Scrolls the bar to `offset` columns, given its tabs' `widths` (never past the last tab).
    pub fn set_scroll_offset(&mut self, mut offset: u32, widths: &[u32]) {
        let mut i = 0;
        while i + 1 < widths.len() && offset >= widths[i] {
            offset -= widths[i];
            i += 1;
        }
        let w = widths.get(i).copied().unwrap_or(1);
        self.scroll = i;
        self.shift = offset.min(w.saturating_sub(1)) as u16;
    }
}

pub struct Groups {
    tree: SplitTree,
    groups: Vec<(GroupId, Group)>,
    pub active: GroupId,
    next: GroupId,
}

impl Groups {
    /// One empty group (the first tab is opened by `App::start`).
    pub fn new() -> Groups {
        Groups {
            tree: SplitTree::new(1),
            groups: vec![(1, Group::default())],
            active: 1,
            next: 2,
        }
    }

    pub fn get(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|(g, _)| *g == id).map(|(_, g)| g)
    }

    pub fn get_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        self.groups.iter_mut().find(|(g, _)| *g == id).map(|(_, g)| g)
    }

    pub fn active_group(&self) -> &Group {
        self.get(self.active).expect("the active group exists")
    }

    pub fn active_group_mut(&mut self) -> &mut Group {
        let id = self.active;
        self.get_mut(id).expect("the active group exists")
    }

    pub fn tabs(&self) -> impl Iterator<Item = &ShellTab> {
        self.groups.iter().flat_map(|(_, g)| g.tabs.iter())
    }

    pub fn tab_count(&self) -> usize {
        self.tabs().count()
    }

    /// Where the tab running `run` is: its group and index.
    pub fn find(&self, run: RunId) -> Option<(GroupId, usize)> {
        self.groups.iter().find_map(|(id, g)| g.position(run).map(|i| (*id, i)))
    }

    /// Each group's area and each divider in `area`.
    pub fn layout(&self, area: Rect) -> (Vec<(GroupId, Rect)>, Vec<Divider>) {
        self.tree.layout(area)
    }

    /// Closes tab `index` of `group` (dropping it hangs its shell up). A group left empty leaves
    /// the tree and focus moves to its neighbour, unless it is the last group, which stays.
    pub fn close_tab(&mut self, group: GroupId, index: usize) -> Option<ShellTab> {
        let tab = self.get_mut(group)?.close(index)?;
        if self.get(group).is_some_and(|g| g.tabs.is_empty())
            && let Some(focus) = self.tree.remove(group)
        {
            self.groups.retain(|(id, _)| *id != group);
            if self.active == group {
                self.active = focus;
            }
        }
        Some(tab)
    }

    /// Splits the active group in `dir`: a new empty group beside it takes the focus (the caller
    /// opens its shell). Returns its id.
    pub fn split(&mut self, dir: SplitDir) -> GroupId {
        let new = self.fresh_id();
        self.tree.split(self.active, dir, new);
        self.groups.push((new, Group::default()));
        self.active = new;
        new
    }

    /// Moves the tab at `index` of group `from` into group `to`, at a gap among its tabs or (`None`)
    /// right of its active tab. The tab (and its running shell) moves as it is, becomes `to`'s
    /// active tab and `to` the active group; a `from` left empty leaves the tree. False and
    /// nothing changes for the same group, an unknown group or a bad `index`.
    pub fn move_between(&mut self, from: GroupId, index: usize, to: GroupId, at: Option<usize>) -> bool {
        if from == to || self.get(to).is_none() || self.get(from).is_none_or(|g| index >= g.tabs.len()) {
            return false;
        }
        let Some(tab) = self.close_tab(from, index) else {
            return false;
        };
        let target = self.get_mut(to).expect("checked above");
        let at = at.map_or(if target.tabs.is_empty() { 0 } else { target.active + 1 }, |a| {
            a.min(target.tabs.len())
        });
        target.tabs.insert(at, tab);
        target.active = at;
        self.active = to;
        true
    }

    /// Splits `target` on `side` into a new empty group and moves the tab at `index` of `from`
    /// into it, which gets the focus. A left or upper side puts the new group first. If nothing
    /// can move the tree is left as it was. Returns the new group's id.
    pub fn move_to_split(&mut self, from: GroupId, index: usize, target: GroupId, side: DropSide) -> Option<GroupId> {
        if self.get(from).is_none_or(|g| index >= g.tabs.len()) {
            return None;
        }
        let new = self.split_empty_of(target, side)?;
        if self.move_between(from, index, new, None) {
            Some(new)
        } else {
            self.tree.remove(new);
            self.groups.retain(|(id, _)| *id != new);
            self.active = target;
            None
        }
    }

    /// Splits `target` into a new empty group on `side`, which becomes active: a left or upper
    /// side puts the new group first in the tree. `None` (nothing changes) if `target` isn't in it.
    pub fn split_empty_of(&mut self, target: GroupId, side: DropSide) -> Option<GroupId> {
        let (dir, new_first) = match side {
            DropSide::Left => (SplitDir::Right, true),
            DropSide::Right => (SplitDir::Right, false),
            DropSide::Up => (SplitDir::Down, true),
            DropSide::Down => (SplitDir::Down, false),
        };
        let new = self.fresh_id();
        if !self.tree.split_with(target, dir, new, new_first) {
            self.next -= 1;
            return None;
        }
        self.groups.push((new, Group::default()));
        self.active = new;
        Some(new)
    }

    fn fresh_id(&mut self) -> GroupId {
        self.next += 1;
        self.next - 1
    }

    /// Moves the divider `divider` (from `layout`) to `ratio`.
    pub fn set_ratio(&mut self, divider: &Divider, ratio: f32) {
        self.tree.set_ratio(divider, ratio);
    }

    /// The groups in tree order.
    pub fn order(&self) -> Vec<GroupId> {
        self.tree.groups()
    }

    /// Focuses the group `step` places on in tree order (wrapping).
    pub fn focus_step(&mut self, step: isize) {
        let order = self.order();
        if let Some(at) = order.iter().position(|g| *g == self.active) {
            let n = order.len() as isize;
            self.active = order[(at as isize + step).rem_euclid(n) as usize];
        }
    }

    /// Focuses the `n`th group (1-based, tree order); none there does nothing.
    pub fn focus_nth(&mut self, n: usize) {
        if let Some(g) = n.checked_sub(1).and_then(|i| self.order().get(i).copied()) {
            self.active = g;
        }
    }

    /// Focuses the group nearest the active one in `side`, given the last layout.
    pub fn focus_side(&mut self, cells: &[(GroupId, Rect)], side: Side) {
        if let Some(g) = neighbour(cells, self.active, side) {
            self.active = g;
        }
    }
}

/// A direction on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// The group beside `from` on `side`: the nearest one past its edge whose span overlaps `from`'s
/// most.
pub fn neighbour(cells: &[(GroupId, Rect)], from: GroupId, side: Side) -> Option<GroupId> {
    let (_, r) = cells.iter().find(|(g, _)| *g == from)?;
    let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| a1.min(b1).saturating_sub(a0.max(b0));
    let (rr, rb) = (r.x + r.width, r.y + r.height);
    cells
        .iter()
        .filter(|(g, _)| *g != from)
        .filter_map(|(g, c)| {
            let (cr, cb) = (c.x + c.width, c.y + c.height);
            let rows = overlap(r.y, rb, c.y, cb);
            let cols = overlap(r.x, rr, c.x, cr);
            let (gap, span) = match side {
                Side::Left if cr <= r.x => (r.x - cr, rows),
                Side::Right if c.x >= rr => (c.x - rr, rows),
                Side::Up if cb <= r.y => (r.y - cb, cols),
                Side::Down if c.y >= rb => (c.y - rb, cols),
                _ => return None,
            };
            (span > 0).then_some((*g, span, gap))
        })
        .min_by_key(|(_, span, gap)| (*gap, std::cmp::Reverse(*span)))
        .map(|(g, ..)| g)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use jterm::run::{EnvNames, Sink, StartOptions};
    use jterm::shell;

    use super::*;

    /// A tab whose shell nothing here reads (a real `sh`, hung up when the tab drops).
    fn tab(id: u64) -> ShellTab {
        let sink: Sink = Arc::new(|_| {});
        let spec = shell::spec(Path::new("/bin/sh"), &std::env::temp_dir(), None);
        let env = EnvNames {
            ipc: "JMUX",
            terminal: "JMUX_TERMINAL",
        };
        let options = StartOptions { env, command_ring: 5 };
        ShellTab::new(RunId(id), spec, 5, 20, sink, 100, options, 1)
    }

    #[test]
    fn the_scroll_offset_counts_whole_tabs_and_the_shift() {
        let widths = [10, 20, 5];
        let mut g = Group::default();
        g.set_scroll_offset(14, &widths);
        assert_eq!((g.scroll, g.shift, g.scroll_offset(&widths)), (1, 4, 14));
        g.set_scroll_offset(10, &widths);
        assert_eq!((g.scroll, g.shift), (1, 0));
        g.set_scroll_offset(99, &widths);
        assert_eq!((g.scroll, g.shift), (2, 4), "never past the last tab");
    }

    fn ids(g: &Group) -> Vec<u64> {
        g.tabs.iter().map(|t| t.run.id.0).collect()
    }

    #[tokio::test]
    async fn open_goes_after_the_active_tab_and_activates_it() {
        let mut g = Group::default();
        g.open(tab(1));
        g.open(tab(2));
        g.go_to(1);
        g.open(tab(3));
        assert_eq!(ids(&g), [1, 3, 2]);
        assert_eq!(g.active, 1);
    }

    #[tokio::test]
    async fn cycle_wraps_and_go_to_ignores_a_missing_tab() {
        let mut g = Group::default();
        for i in 1..=3 {
            g.open(tab(i));
        }
        assert_eq!(g.active, 2);
        g.cycle(1);
        assert_eq!(g.active, 0);
        g.cycle(-1);
        assert_eq!(g.active, 2);
        g.go_to(2);
        assert_eq!(g.active, 1);
        g.go_to(9);
        g.go_to(0);
        assert_eq!(g.active, 1);
    }

    #[tokio::test]
    async fn closing_keeps_the_active_tab_sensible() {
        let mut g = Group::default();
        for i in 1..=4 {
            g.open(tab(i));
        }
        g.go_to(3);
        g.close(0);
        assert_eq!((ids(&g), g.active), (vec![2, 3, 4], 1));
        g.close(1);
        assert_eq!((ids(&g), g.active), (vec![2, 4], 1));
        g.close(1);
        assert_eq!((ids(&g), g.active), (vec![2], 0));
        assert!(g.close(5).is_none());
    }

    #[tokio::test]
    async fn an_emptied_group_leaves_the_tree_but_the_last_one_stays() {
        let mut gs = Groups::new();
        gs.active_group_mut().open(tab(1));
        gs.active_group_mut().open(tab(2));
        assert_eq!(gs.find(RunId(2)), Some((1, 1)));
        assert!(gs.close_tab(1, 1).is_some());
        assert!(gs.close_tab(1, 0).is_some());
        assert_eq!(gs.tab_count(), 0);
        assert_eq!(gs.layout(Rect::new(0, 0, 40, 10)).0.len(), 1);
        // With a second group, emptying it removes it and moves the focus.
        let mut gs = Groups::new();
        let second = gs.split(SplitDir::Right);
        gs.get_mut(1).unwrap().open(tab(1));
        gs.get_mut(second).unwrap().open(tab(2));
        gs.active = second;
        gs.close_tab(second, 0);
        assert_eq!((gs.active, gs.layout(Rect::new(0, 0, 40, 10)).0.len()), (1, 1));
        assert!(gs.get(second).is_none());
    }

    #[tokio::test]
    async fn split_adds_a_focused_group_beside_the_active_one() {
        let mut gs = Groups::new();
        gs.active_group_mut().open(tab(1));
        let second = gs.split(SplitDir::Right);
        assert_eq!((gs.active, gs.order()), (second, vec![1, second]));
        gs.active_group_mut().open(tab(2));
        let (cells, dividers) = gs.layout(Rect::new(0, 0, 41, 10));
        assert_eq!((cells.len(), dividers.len()), (2, 1));
        assert!(cells[0].1.x < cells[1].1.x);
        // Closing the new group's only tab removes it and focus returns to the first.
        gs.close_tab(second, 0);
        assert_eq!((gs.active, gs.order()), (1, vec![1]));
        assert_eq!(gs.layout(Rect::new(0, 0, 41, 10)).0[0].1.width, 41);
    }

    #[tokio::test]
    async fn move_tab_reorders_activates_and_ignores_bad_or_no_moves() {
        let mut g = Group::default();
        for i in 1..=4 {
            g.open(tab(i));
        }
        assert!(g.move_tab(0, 2));
        assert_eq!((ids(&g), g.active), (vec![2, 3, 1, 4], 2));
        assert!(g.move_tab(3, 0));
        assert_eq!((ids(&g), g.active), (vec![4, 2, 3, 1], 0));
        assert!(!g.move_tab(1, 1), "same place");
        assert!(!g.move_tab(1, 4), "past the end");
        assert!(!g.move_tab(4, 0), "no such tab");
        assert_eq!((ids(&g), g.active), (vec![4, 2, 3, 1], 0));
    }

    #[tokio::test]
    async fn move_between_keeps_the_tab_and_removes_an_emptied_group() {
        let mut gs = Groups::new();
        for i in 1..=2 {
            gs.get_mut(1).unwrap().open(tab(i));
        }
        let b = gs.split(SplitDir::Right);
        for i in 3..=4 {
            gs.get_mut(b).unwrap().open(tab(i));
        }
        assert!(!gs.move_between(1, 0, 1, Some(0)), "same group");
        assert!(!gs.move_between(1, 5, b, None), "no such tab");
        assert!(!gs.move_between(1, 0, 99, None), "no such group");
        assert_eq!(gs.tab_count(), 4);
        // At a gap: the tab is inserted there and active, its group focused.
        gs.active = 1;
        assert!(gs.move_between(1, 0, b, Some(1)));
        assert_eq!(
            (ids(gs.get(1).unwrap()), ids(gs.get(b).unwrap())),
            (vec![2], vec![3, 1, 4])
        );
        assert_eq!((gs.get(b).unwrap().active, gs.active), (1, b));
        // Without a gap: right of the target's active tab.
        gs.get_mut(b).unwrap().active = 0;
        gs.active = 1;
        assert!(gs.move_between(1, 0, b, None));
        assert_eq!(ids(gs.get(b).unwrap()), [3, 2, 1, 4]);
        assert_eq!((gs.get(b).unwrap().active, gs.active), (1, b));
        assert!(gs.get(1).is_none(), "the emptied group left the tree");
        assert_eq!(gs.order(), vec![b]);
        // A gap past the end clamps to the end.
        let c = gs.split(SplitDir::Down);
        gs.get_mut(c).unwrap().open(tab(5));
        assert!(gs.move_between(c, 0, b, Some(99)));
        assert_eq!(ids(gs.get(b).unwrap()), [3, 2, 1, 4, 5]);
    }

    #[tokio::test]
    async fn move_to_split_puts_the_new_group_on_the_side_and_gives_it_the_tab() {
        for (side, new_first) in [
            (DropSide::Left, true),
            (DropSide::Right, false),
            (DropSide::Up, true),
            (DropSide::Down, false),
        ] {
            let mut gs = Groups::new();
            gs.get_mut(1).unwrap().open(tab(1));
            gs.get_mut(1).unwrap().open(tab(2));
            let new = gs.move_to_split(1, 0, 1, side).expect("splits");
            let order = gs.order();
            assert_eq!(order, if new_first { vec![new, 1] } else { vec![1, new] }, "{side:?}");
            assert_eq!((ids(gs.get(new).unwrap()), ids(gs.get(1).unwrap())), (vec![1], vec![2]));
            assert_eq!(gs.active, new);
        }
        // A bad tab or target leaves the tree alone.
        let mut gs = Groups::new();
        gs.get_mut(1).unwrap().open(tab(1));
        assert!(gs.move_to_split(1, 3, 1, DropSide::Right).is_none());
        assert!(gs.move_to_split(1, 0, 99, DropSide::Right).is_none());
        assert_eq!((gs.order(), ids(gs.get(1).unwrap())), (vec![1], vec![1]));
        // A lone tab onto its own edge: the original empties and goes.
        let new = gs.move_to_split(1, 0, 1, DropSide::Right).unwrap();
        assert_eq!((gs.order(), ids(gs.get(new).unwrap())), (vec![new], vec![1]));
    }

    #[tokio::test]
    async fn focus_steps_wrap_and_nth_ignores_a_missing_group() {
        let mut gs = Groups::new();
        let b = gs.split(SplitDir::Right);
        let c = gs.split(SplitDir::Down);
        assert_eq!(gs.order(), vec![1, b, c]);
        gs.focus_step(1);
        assert_eq!(gs.active, 1);
        gs.focus_step(-1);
        assert_eq!(gs.active, c);
        gs.focus_nth(2);
        assert_eq!(gs.active, b);
        gs.focus_nth(9);
        gs.focus_nth(0);
        assert_eq!(gs.active, b);
    }

    #[test]
    fn neighbours_on_a_two_by_two_layout() {
        let r = |x, y| Rect::new(x, y, 10, 5);
        // 1 2
        // 3 4, with a wide group 5 under them.
        let cells = [
            (1, r(0, 0)),
            (2, r(11, 0)),
            (3, r(0, 6)),
            (4, r(11, 6)),
            (5, Rect::new(0, 12, 21, 5)),
        ];
        assert_eq!(neighbour(&cells, 1, Side::Right), Some(2));
        assert_eq!(neighbour(&cells, 1, Side::Down), Some(3));
        assert_eq!(neighbour(&cells, 4, Side::Left), Some(3));
        assert_eq!(neighbour(&cells, 4, Side::Up), Some(2));
        assert_eq!(neighbour(&cells, 3, Side::Down), Some(5));
        assert_eq!(neighbour(&cells, 1, Side::Left), None);
        assert_eq!(neighbour(&cells, 1, Side::Up), None);
        assert_eq!(neighbour(&cells, 5, Side::Up), Some(3));
        assert_eq!(neighbour(&cells, 9, Side::Up), None);
    }
}
