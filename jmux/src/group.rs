//! Groups of shell tabs in a split tree. A `Group` is a tab bar's worth of shells; `Groups` lays
//! them out with `jtui::split::SplitTree` (one group until splits arrive) and knows which one has
//! the focus. Closing a tab drops its `ShellTab`, which hangs the shell up; a group left without
//! tabs leaves the tree, and the last tab closing leaves the session without any.

use jterm::run::RunId;
use jterm::shell::ShellTab;
use jtui::split::{Divider, GroupId, SplitTree};
use ratatui::layout::Rect;

/// Tabs side by side in one bar, one of them active.
#[derive(Default)]
pub struct Group {
    pub tabs: Vec<ShellTab>,
    pub active: usize,
    /// The first tab the bar shows when the tabs overflow it.
    pub scroll: usize,
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
    pub fn scroll_tabs(&mut self, delta: isize) {
        let last = self.tabs.len().saturating_sub(1);
        self.scroll = self.scroll.saturating_add_signed(delta).min(last);
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

    /// The next free group id (for splits, `jmux_layout-b`).
    #[allow(dead_code)]
    pub fn fresh_id(&mut self) -> GroupId {
        self.next += 1;
        self.next - 1
    }
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
        let second = gs.fresh_id();
        assert!(gs.tree.split(1, jtui::split::SplitDir::Right, second));
        gs.groups.push((second, Group::default()));
        gs.get_mut(1).unwrap().open(tab(1));
        gs.get_mut(second).unwrap().open(tab(2));
        gs.active = second;
        gs.close_tab(second, 0);
        assert_eq!((gs.active, gs.layout(Rect::new(0, 0, 40, 10)).0.len()), (1, 1));
        assert!(gs.get(second).is_none());
    }
}
