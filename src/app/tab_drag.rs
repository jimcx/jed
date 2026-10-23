//! Dragging a tab by the mouse. A press on a tab starts a drag (the tab activates, so a drag
//! that never leaves it is just a click); every `Drag` event records where the pointer would
//! drop it, and the release applies the drop. A tab drops back into its own bar at a gap, onto
//! another group's bar at the gap there, or onto another group's editor (into that group, right
//! of its active tab). `drag_tabs-c` adds drops on an editor's edges that split it.

use ratatui::layout::Position;

use super::mouse::Drag;
use super::{App, Focus};
use crate::layout::{GroupId, TabHit};

/// An in-flight tab drag: the tab's group and index where it started, and where it would land
/// (`None` while the pointer is not over a target).
#[derive(Debug, Clone, PartialEq)]
pub struct TabDrag {
    pub from: GroupId,
    pub index: usize,
    pub target: Option<TabDrop>,
}

/// Where a dragged tab would land. `index` is a gap among the target group's tabs: the tab at
/// that index would come after the dropped one (`tabs.len()` is the bar's end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDrop {
    TabBar {
        group: GroupId,
        index: usize,
    },
    /// The centre of a group's editor: the tab moves into that group, right of its active tab
    /// (`drag_tabs-c` adds the edges that split it).
    Editor {
        group: GroupId,
    },
}

/// The gap in `tabs` at `x`: the tabs whose midpoint is left of the pointer. The index is in
/// the group's own terms, so a scrolled bar's first drawn tab (`tabs[0].index`) is counted from.
pub(super) fn gap_at(tabs: &[TabHit], x: u16) -> usize {
    let first = tabs.first().map_or(0, |t| t.index);
    first + tabs.iter().filter(|t| t.x0 + (t.x1 - t.x0) / 2 < x).count()
}

impl App {
    /// Where a tab dropped at (`x`, `y`) would land: a gap in the tab bar under the pointer, or
    /// the centre of a group's editor. Anywhere else (a divider, the side panel, the status bar)
    /// there is no target.
    pub(super) fn tab_drop_at(&self, x: u16, y: u16) -> Option<TabDrop> {
        if let Some(group) = self.hits.groups.iter().find(|g| g.tab_bar.contains(Position { x, y })) {
            return Some(TabDrop::TabBar {
                group: group.id,
                index: gap_at(&group.tabs, x),
            });
        }
        let group = self.hits.groups.iter().find(|g| g.editor.contains(Position { x, y }))?;
        Some(TabDrop::Editor { group: group.id })
    }

    /// The gap a drop in `group` would land in, while a tab is being dragged: the drop marker's
    /// cell for that group (`ui::tabs::render`). Any group's bar can take the tab; an editor
    /// target shows no marker (`drag_tabs-c` shades the zone there instead).
    pub(crate) fn tab_drop_in(&self, group: GroupId) -> Option<usize> {
        match &self.drag {
            Some(Drag::Tab(TabDrag {
                target: Some(TabDrop::TabBar { group: g, index }),
                ..
            })) if *g == group => Some(*index),
            _ => None,
        }
    }

    /// Follows the pointer during a tab drag: records the target for the drop marker.
    pub(super) fn tab_drag_to(&mut self, x: u16, y: u16) {
        let target = self.tab_drop_at(x, y);
        if let Some(Drag::Tab(drag)) = &mut self.drag {
            drag.target = target;
        }
    }

    /// Ends a tab drag at (`x`, `y`): reorders the tab within its own bar, moves it to another
    /// group's bar at the gap there, or moves it into the group whose editor it is over. A drop
    /// on its own group's editor, with no target, or after Esc moves nothing. The drag is
    /// cleared either way.
    pub(super) fn finish_tab_drag(&mut self, x: u16, y: u16) {
        let Some(Drag::Tab(drag)) = self.drag.take() else {
            return;
        };
        let Some(target) = self.tab_drop_at(x, y) else {
            return;
        };
        let (group, at) = match target {
            TabDrop::TabBar { group, index } if group == drag.from => {
                let Some(g) = self.groups.get_mut(group) else {
                    return;
                };
                // `index` counts the dragged tab too, while it is still where it started: a gap
                // after it is one place further once it is taken out.
                let to = if index > drag.index { index - 1 } else { index };
                g.move_tab(drag.index, to);
                return;
            }
            TabDrop::TabBar { group, index } => (group, Some(index)),
            TabDrop::Editor { group } if group == drag.from => return,
            TabDrop::Editor { group } => (group, None),
        };
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        if self.groups.move_tab_between(drag.from, drag.index, group, at) {
            self.ui.focus = Focus::Editor;
            self.reveal_active();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::gap_at;
    use crate::layout::TabHit;

    fn tab(index: usize, x0: u16, x1: u16) -> TabHit {
        TabHit {
            index,
            x0,
            x1,
            close_x: x0,
        }
    }

    #[test]
    fn the_gap_is_the_tabs_whose_midpoint_is_left_of_the_pointer() {
        let tabs = [tab(0, 0, 10), tab(1, 10, 20), tab(2, 20, 30)];
        assert_eq!(gap_at(&tabs, 0), 0);
        assert_eq!(gap_at(&tabs, 4), 0, "left of the first midpoint");
        assert_eq!(gap_at(&tabs, 5), 0, "on the midpoint is not past it");
        assert_eq!(gap_at(&tabs, 6), 1);
        assert_eq!(gap_at(&tabs, 15), 1);
        assert_eq!(gap_at(&tabs, 16), 2);
        assert_eq!(gap_at(&tabs, 25), 2);
        assert_eq!(gap_at(&tabs, 26), 3, "past the last midpoint: the end");
        assert_eq!(gap_at(&tabs, 60), 3, "past the bar is still the end");
    }

    #[test]
    fn a_scrolled_bar_counts_from_its_first_drawn_tab() {
        let tabs = [tab(4, 0, 10), tab(5, 10, 20)];
        assert_eq!(gap_at(&tabs, 0), 4);
        assert_eq!(gap_at(&tabs, 6), 5);
        assert_eq!(gap_at(&tabs, 26), 6);
        assert_eq!(gap_at(&[], 8), 0, "an empty bar: the only gap");
    }
}
