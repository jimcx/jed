//! Dragging a tab by the mouse, as jed's `app/tab_drag.rs` does for editor tabs. A press on a tab
//! starts a drag (the tab activates, so a drag that never leaves it is just a click); motion records
//! where the pointer would drop it (`ui` draws the marker or the shade); the release applies the
//! drop: reorder within the bar, move to another group's bar at the gap, into another group's
//! terminal (right of its active tab), or onto a terminal's edge band to split that group there.
//! The shells keep running through a move; the next frame resizes the visible ones.

use jterm::run::RunId;
use jtui::split::{DropSide, GroupId, gap_at, snap_side};
use ratatui::layout::{Position, Rect};

use crate::app::App;

/// An in-flight tab drag: where the tab started and where it would land (`None` while the pointer
/// is over no target).
#[derive(Debug, Clone, PartialEq)]
pub struct TabDrag {
    pub from: GroupId,
    pub index: usize,
    /// The dragged shell: a shell exiting or a key reordering tabs mid-drag leaves `index` on
    /// another tab, and then the release moves nothing.
    pub run: RunId,
    pub target: Option<TabDrop>,
}

/// Where a dragged tab would land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDrop {
    /// A gap among a group's tabs: the tab at `index` would come after the dropped one.
    TabBar { group: GroupId, index: usize },
    /// A group's terminal: on an edge band (`side`) the group splits there, in the centre (`None`)
    /// the tab joins it, right of its active tab.
    Terminal { group: GroupId, side: Option<DropSide> },
}

/// The part of `term` a drop on `side` would take: all of it for the centre, else the half the
/// split would give the new group.
pub fn shade_rect(term: Rect, side: Option<DropSide>) -> Rect {
    let (half_w, half_h) = (term.width / 2, term.height / 2);
    match side {
        None => term,
        Some(DropSide::Left) => Rect { width: half_w, ..term },
        Some(DropSide::Right) => Rect {
            x: term.x + half_w,
            width: term.width - half_w,
            ..term
        },
        Some(DropSide::Up) => Rect { height: half_h, ..term },
        Some(DropSide::Down) => Rect {
            y: term.y + half_h,
            height: term.height - half_h,
            ..term
        },
    }
}

impl App {
    /// Where a tab dropped at (`x`, `y`) would land: a gap in a group's bar, or a group's terminal
    /// (edge band or centre). Anywhere else (a divider, the status bar) there is no target.
    fn tab_drop_at(&self, x: u16, y: u16) -> Option<TabDrop> {
        let at = Position { x, y };
        if let Some(g) = self.hit.groups.iter().find(|g| g.bar.contains(at)) {
            let index = gap_at(g.tabs.iter().map(|t| (t.index, t.x0, t.x1)), x);
            return Some(TabDrop::TabBar { group: g.id, index });
        }
        let g = self.hit.groups.iter().find(|g| g.term.contains(at))?;
        Some(TabDrop::Terminal {
            group: g.id,
            side: snap_side(g.term, x, y),
        })
    }

    /// The pointer moved during a drag: records the target the marker and shade show.
    pub(crate) fn tab_drag_to(&mut self, x: u16, y: u16) {
        let target = self.tab_drop_at(x, y);
        if let Some(drag) = self.tab_drag_mut() {
            drag.target = target;
        }
    }

    /// The release at (`x`, `y`): applies the drop (see the module doc). A drop on the tab's own
    /// terminal centre, outside every target, or after the tab changed under the drag moves nothing.
    pub(crate) fn finish_tab_drag(&mut self, x: u16, y: u16) {
        let Some(drag) = self.take_tab_drag() else {
            return;
        };
        let (from, index) = (drag.from, drag.index);
        if self.groups.get(from).and_then(|g| g.tabs.get(index)).map(|t| t.run.id) != Some(drag.run) {
            return;
        }
        match self.tab_drop_at(x, y) {
            Some(TabDrop::TabBar { group, index: gap }) if group == from => {
                // `gap` counts the dragged tab too, while it is still where it started.
                let to = if gap > index { gap - 1 } else { gap };
                if let Some(g) = self.groups.get_mut(from) {
                    g.move_tab(index, to);
                }
            }
            Some(TabDrop::TabBar { group, index: gap }) => {
                self.groups.move_between(from, index, group, Some(gap));
            }
            Some(TabDrop::Terminal {
                group,
                side: Some(side),
            }) => {
                self.groups.move_to_split(from, index, group, side);
            }
            Some(TabDrop::Terminal { group, side: None }) if group != from => {
                self.groups.move_between(from, index, group, None);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shade_is_the_half_a_split_would_take() {
        let r = Rect::new(10, 2, 41, 11);
        assert_eq!(shade_rect(r, None), r);
        assert_eq!(shade_rect(r, Some(DropSide::Left)), Rect::new(10, 2, 20, 11));
        assert_eq!(shade_rect(r, Some(DropSide::Right)), Rect::new(30, 2, 21, 11));
        assert_eq!(shade_rect(r, Some(DropSide::Up)), Rect::new(10, 2, 41, 5));
        assert_eq!(shade_rect(r, Some(DropSide::Down)), Rect::new(10, 7, 41, 6));
    }
}
