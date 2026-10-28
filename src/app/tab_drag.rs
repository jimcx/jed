//! Dragging a tab by the mouse. A press on a tab starts a drag (the tab activates, so a drag
//! that never leaves it is just a click); every `Drag` event records where the pointer would
//! drop it, and the release applies the drop. A tab drops back into its own bar at a gap, onto
//! another group's bar at the gap there, into a group whose editor it is over, or on an editor's
//! edge: the nearer quarter of an editor is its band, and a drop there splits the group on that
//! side, the new split taking the tab (`ui` shades the band while dragging).

use ratatui::layout::{Position, Rect};

use super::mouse::Drag;
use super::{App, Focus};
use crate::group::{Tab, TabIdentity, is_terminal_group};
use crate::layout::{DropSide, GroupId, MIN_GROUP_HEIGHT, MIN_GROUP_WIDTH, TabHit};

/// An in-flight tab drag: the tab's group and index where it started, and where it would land
/// (`None` while the pointer is not over a target).
#[derive(Debug, Clone, PartialEq)]
pub struct TabDrag {
    pub from: GroupId,
    pub index: usize,
    /// The dragged tab's identity at the press. A key can close, open or reorder a tab while a
    /// drag is in flight, after which `index` may no longer be the dragged tab (and the group's
    /// count alone can't tell): the release moves nothing unless the tab at `index` is still the
    /// same one.
    pub tab: Option<TabIdentity>,
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
    /// A group's editor: `side` is the edge band the pointer is in, which the group splits on,
    /// or `None` for the centre (the tab moves into the group, right of its active tab).
    Editor {
        group: GroupId,
        side: Option<DropSide>,
    },
    /// The strip the hidden terminal area would take: a shell dropped here shows the area and
    /// lands in it.
    TerminalStrip,
}

/// The gap in `tabs` at `x`: the tabs whose midpoint is left of the pointer. The index is in
/// the group's own terms, so a scrolled bar's first drawn tab (`tabs[0].index`) is counted from.
fn gap_at(tabs: &[TabHit], x: u16) -> usize {
    let first = tabs.first().map_or(0, |t| t.index);
    first + tabs.iter().filter(|t| t.x0 + (t.x1 - t.x0) / 2 < x).count()
}

/// Which edge of `rect` the point (`x`, `y`) is near enough to snap to, or `None` for its
/// centre (or outside it). Each edge's band is a quarter of that dimension, and the nearest
/// edge within its band wins (ties in the order Left, Right, Up, Down). An edge whose dimension
/// can't hold a split of two minimum groups is no edge at all.
fn snap_side(rect: Rect, x: u16, y: u16) -> Option<DropSide> {
    if !rect.contains(Position { x, y }) {
        return None;
    }
    let horizontal = rect.width > 2 * MIN_GROUP_WIDTH;
    let vertical = rect.height > 2 * MIN_GROUP_HEIGHT;
    let (band_w, band_h) = (rect.width / 4, rect.height / 4);
    let (right, bottom) = (rect.x + rect.width - 1, rect.y + rect.height - 1);
    // In tie order; `min` keeps the first of equal distances.
    let candidates = [
        (DropSide::Left, x - rect.x, horizontal, band_w),
        (DropSide::Right, right - x, horizontal, band_w),
        (DropSide::Up, y - rect.y, vertical, band_h),
        (DropSide::Down, bottom - y, vertical, band_h),
    ];
    candidates
        .into_iter()
        .filter(|&(_, distance, fits, band)| fits && distance <= band)
        .min_by_key(|&(_, distance, ..)| distance)
        .map(|(side, ..)| side)
}

/// Whether a tab dragged out of group `from` (`shell`: it is a terminal) may land in group `to`.
/// Only terminals dock in the terminal area, from the area or an editor group; a terminal may
/// drop on any editor group.
fn can_dock(from: GroupId, to: GroupId, shell: bool) -> bool {
    !is_terminal_group(to) || is_terminal_group(from) || shell
}

impl App {
    /// Where a tab dragged out of `from` and dropped at (`x`, `y`) would land: a gap in the tab bar
    /// under the pointer, or a group's editor (on an edge band, or its centre). Anywhere else (a
    /// divider, the side panel, the status bar, a group `can_dock` refuses) there is no target.
    pub(super) fn tab_drop_at(&self, from: GroupId, index: usize, x: u16, y: u16) -> Option<TabDrop> {
        let shell = self
            .any_group(from)
            .is_some_and(|g| matches!(g.tabs.get(index), Some(Tab::Shell(_))));
        if shell && !self.ui.terminal_visible && self.hits.terminal_strip.is_some_and(|r| r.contains(Position { x, y }))
        {
            return Some(TabDrop::TerminalStrip);
        }
        let mut dockable = self.hits.groups.iter().filter(|g| can_dock(from, g.id, shell));
        if let Some(group) = dockable.clone().find(|g| g.tab_bar.contains(Position { x, y })) {
            return Some(TabDrop::TabBar {
                group: group.id,
                index: gap_at(&group.tabs, x),
            });
        }
        let group = dockable.find(|g| g.editor.contains(Position { x, y }))?;
        Some(TabDrop::Editor {
            group: group.id,
            side: snap_side(group.editor, x, y),
        })
    }

    /// The gap a drop in `group` would land in, while a tab is being dragged: the drop marker's
    /// cell for that group (`ui::tabs::render`). Any group's bar can take the tab; an editor
    /// target shows no marker (the shade marks the zone there instead).
    pub(crate) fn tab_drop_in(&self, group: GroupId) -> Option<usize> {
        match &self.drag {
            Some(Drag::Tab(TabDrag {
                target: Some(TabDrop::TabBar { group: g, index }),
                ..
            })) if *g == group => Some(*index),
            _ => None,
        }
    }

    /// The zone to shade while a tab drag would drop in `group`'s editor: all of it for the
    /// centre, or the half the pointer's edge band would split off. `None` for another group,
    /// a bar target, or no drag.
    pub(crate) fn tab_drop_shade(&self, group: GroupId, editor: Rect) -> Option<Rect> {
        let Some(Drag::Tab(TabDrag {
            target: Some(TabDrop::Editor { group: g, side }),
            ..
        })) = &self.drag
        else {
            return None;
        };
        if *g != group {
            return None;
        }
        let (half_w, half_h) = (editor.width / 2, editor.height / 2);
        Some(match side {
            None => editor,
            Some(DropSide::Left) => Rect {
                width: half_w,
                ..editor
            },
            Some(DropSide::Right) => Rect {
                x: editor.x + half_w,
                width: editor.width - half_w,
                ..editor
            },
            Some(DropSide::Up) => Rect {
                height: half_h,
                ..editor
            },
            Some(DropSide::Down) => Rect {
                y: editor.y + half_h,
                height: editor.height - half_h,
                ..editor
            },
        })
    }

    /// The strip to shade while a shell is dragged over the hidden terminal area's place.
    pub(crate) fn terminal_strip_shade(&self) -> Option<Rect> {
        match &self.drag {
            Some(Drag::Tab(TabDrag {
                target: Some(TabDrop::TerminalStrip),
                ..
            })) => self.hits.terminal_strip,
            _ => None,
        }
    }

    /// Follows the pointer during a tab drag: records the target for the drop marker.
    pub(super) fn tab_drag_to(&mut self, x: u16, y: u16) {
        let Some(Drag::Tab(drag)) = &self.drag else {
            return;
        };
        let target = self.tab_drop_at(drag.from, drag.index, x, y);
        if let Some(Drag::Tab(drag)) = &mut self.drag {
            drag.target = target;
        }
    }

    /// Ends a tab drag at (`x`, `y`): reorders the tab within its own bar, moves it to another
    /// group's bar at the gap there, into the editor it is over, or splits that editor's group
    /// on the side its pointer is in and moves it into the new split. A drop on its own group's
    /// editor's centre, with no target, or after Esc moves nothing. The drag is cleared either way.
    pub(super) fn finish_tab_drag(&mut self, x: u16, y: u16) {
        let Some(Drag::Tab(drag)) = self.drag.take() else {
            return;
        };
        // A key can close, open or move a tab while a drag is in flight (only Esc ends a drag):
        // if the tab at `index` is no longer the one the drag started on, the release moves
        // nothing.
        if self
            .any_group(drag.from)
            .and_then(|g| g.tabs.get(drag.index))
            .map(Tab::identity)
            != drag.tab
        {
            return;
        }
        let Some(target) = self.tab_drop_at(drag.from, drag.index, x, y) else {
            return;
        };
        let (group, at) = match target {
            TabDrop::TabBar { group, index } if group == drag.from => {
                let Some(g) = self.any_group_mut(group) else {
                    return;
                };
                // `index` counts the dragged tab too, while it is still where it started: a gap
                // after it is one place further once it is taken out.
                let to = if index > drag.index { index - 1 } else { index };
                g.move_tab(drag.index, to);
                return;
            }
            TabDrop::TabBar { group, index } => (group, Some(index)),
            TabDrop::Editor {
                group,
                side: Some(side),
            } => {
                if let Some((buf, _)) = self.active_mut() {
                    buf.seal_undo();
                }
                let Some(new) = self.groups_of_mut(group).split_empty_of(group, side) else {
                    return;
                };
                if self.move_tab_across(drag.from, drag.index, new, None) {
                    self.focus_dropped(group);
                } else {
                    // Nothing moved: don't leave the empty split behind.
                    self.groups_of_mut(group).remove(new);
                }
                return;
            }
            TabDrop::TerminalStrip => (self.terminals.active, None),
            TabDrop::Editor { group, side: None } if group == drag.from => return,
            TabDrop::Editor { group, side: None } => (group, None),
        };
        if let Some((buf, _)) = self.active_mut() {
            buf.seal_undo();
        }
        if self.move_tab_across(drag.from, drag.index, group, at) {
            self.focus_dropped(group);
        }
    }

    /// Moves the tab at `index` of group `from` into group `to` (at a gap, or right of its active
    /// tab for `None`), within one area or between the terminal area and an editor group:
    /// `Groups::take_tab` out of `from`'s groups (an emptied group closes), `put_tab` into `to`'s.
    /// The terminal area hides when its last shell leaves. False and nothing changes for an
    /// unknown group or a bad `index`.
    pub(super) fn move_tab_across(&mut self, from: GroupId, index: usize, to: GroupId, at: Option<usize>) -> bool {
        if from == to || self.any_group(to).is_none() || self.any_group(from).is_none_or(|g| index >= g.tabs.len()) {
            return false;
        }
        let moved = if is_terminal_group(from) == is_terminal_group(to) {
            self.groups_of_mut(to).move_tab_between(from, index, to, at)
        } else {
            let tab = self.groups_of_mut(from).take_tab(from, index);
            tab.is_some_and(|tab| self.groups_of_mut(to).put_tab(to, tab, at))
        };
        self.hide_empty_terminals();
        moved
    }

    /// After a tab dropped into `group`: its area has the focus (the terminal area's, shown, or the editor's).
    pub(super) fn focus_dropped(&mut self, group: GroupId) {
        if is_terminal_group(group) {
            self.ui.terminal_visible = true;
            self.ui.focus = Focus::Terminal;
        } else {
            self.ui.focus = Focus::Editor;
            self.reveal_active();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{can_dock, gap_at, snap_side};
    use crate::group::TERMINAL_GROUP_BASE as T;
    use crate::layout::{DropSide, TabHit};
    use ratatui::layout::Rect;

    fn tab(index: usize, x0: u16, x1: u16) -> TabHit {
        TabHit {
            index,
            x0,
            x1,
            close_x: x0,
        }
    }

    #[test]
    fn only_shells_dock_in_the_area() {
        assert!(can_dock(T, T + 1, true) && can_dock(0, 1, false));
        assert!(can_dock(T, 0, true), "a terminal drops on an editor group");
        assert!(can_dock(0, T, true), "a shell from an editor group docks in the area");
        assert!(!can_dock(0, T, false), "no other tab docks in the area");
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

    /// A 40×20 editor: each band is the nearer quarter (10 columns, 5 rows).
    #[test]
    fn the_nearer_quarter_is_a_side_and_the_middle_is_the_centre() {
        let r = Rect::new(0, 0, 40, 20);
        assert_eq!(snap_side(r, 0, 10), Some(DropSide::Left));
        assert_eq!(snap_side(r, 10, 10), Some(DropSide::Left), "exactly a quarter in");
        assert_eq!(snap_side(r, 11, 10), None, "past the band: the centre");
        assert_eq!(snap_side(r, 29, 10), Some(DropSide::Right));
        assert_eq!(snap_side(r, 39, 10), Some(DropSide::Right), "the last column");
        assert_eq!(snap_side(r, 28, 10), None);
        assert_eq!(snap_side(r, 20, 0), Some(DropSide::Up));
        assert_eq!(snap_side(r, 20, 5), Some(DropSide::Up), "exactly a quarter down");
        assert_eq!(snap_side(r, 20, 6), None);
        assert_eq!(snap_side(r, 20, 14), Some(DropSide::Down));
        assert_eq!(snap_side(r, 20, 19), Some(DropSide::Down), "the last row");
        assert_eq!(snap_side(r, 20, 13), None);
        assert_eq!(snap_side(r, 20, 10), None, "the centre");
        assert_eq!(snap_side(r, 40, 10), None, "outside");
        assert_eq!(snap_side(r, 20, 20), None);
    }

    #[test]
    fn the_nearest_edge_wins_with_left_right_up_down_ties() {
        let r = Rect::new(0, 0, 40, 20);
        assert_eq!(snap_side(r, 2, 2), Some(DropSide::Left), "left and up tie: left first");
        assert_eq!(
            snap_side(r, 37, 2),
            Some(DropSide::Right),
            "right and up tie: right first"
        );
        assert_eq!(
            snap_side(r, 2, 17),
            Some(DropSide::Left),
            "left and down tie: left first"
        );
        assert_eq!(
            snap_side(r, 37, 17),
            Some(DropSide::Right),
            "right and down tie: right first"
        );
        assert_eq!(snap_side(r, 20, 2), Some(DropSide::Up), "the top half");
        assert_eq!(
            snap_side(r, 5, 4),
            Some(DropSide::Up),
            "the nearer edge wins, not the first"
        );
        assert_eq!(snap_side(r, 4, 5), Some(DropSide::Left));
    }

    #[test]
    fn snap_side_works_on_a_rect_away_from_the_origin() {
        let r = Rect::new(40, 7, 40, 20);
        assert_eq!(snap_side(r, 40, 17), Some(DropSide::Left));
        assert_eq!(snap_side(r, 50, 17), Some(DropSide::Left), "exactly a quarter in");
        assert_eq!(snap_side(r, 51, 17), None);
        assert_eq!(snap_side(r, 79, 17), Some(DropSide::Right), "the last column");
        assert_eq!(snap_side(r, 60, 7), Some(DropSide::Up));
        assert_eq!(snap_side(r, 60, 26), Some(DropSide::Down), "the last row");
        assert_eq!(snap_side(r, 39, 17), None, "outside on the left");
        assert_eq!(snap_side(r, 80, 17), None, "outside on the right");
        assert_eq!(snap_side(r, 60, 6), None, "outside above");
        assert_eq!(snap_side(r, 60, 27), None, "outside below");
    }

    #[test]
    fn an_edge_that_cannot_hold_a_split_is_no_edge() {
        // 25 is the narrowest width that can hold two minimum groups (12 + a divider + 12).
        let narrow = Rect::new(0, 0, 24, 20);
        assert_eq!(snap_side(narrow, 0, 10), None, "too narrow to split sideways");
        let just = Rect::new(0, 0, 25, 20);
        assert_eq!(snap_side(just, 0, 10), Some(DropSide::Left), "25 fits a split");
        assert_eq!(snap_side(just, 24, 10), Some(DropSide::Right));

        // 9 is the shortest height that can hold two minimum groups (4 + a divider + 4).
        let short = Rect::new(0, 0, 40, 8);
        assert_eq!(snap_side(short, 20, 0), None, "too short to split lengthways");
        let just = Rect::new(0, 0, 40, 9);
        assert_eq!(snap_side(just, 20, 0), Some(DropSide::Up), "9 fits a split");
        assert_eq!(snap_side(just, 20, 8), Some(DropSide::Down));
    }
}
