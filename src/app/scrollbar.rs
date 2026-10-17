//! The editors' scroll bars (`ui::scrollbar` draws them, `scrollbar` has the maths): a press on
//! the thumb grabs it; one on the track jumps the view so the thumb centres on the pointer (VS
//! Code's default, `scrollbar.scrollByPage` off) and grabs it there. Dragging scrolls keeping
//! the grab, the pointer anywhere, until the button is released. The group scrolls without being
//! focused and no cursor moves, as with the wheel.

use crossterm::event::MouseEvent;

use super::App;
use super::mouse::Drag;
use crate::buffer::pos;
use crate::group::Tab;
use crate::layout::{GroupId, Hit, ScrollBarHit};
use crate::scrollbar::Scroll;

impl App {
    /// The group whose scroll bar is being dragged (drawn in the active colour).
    pub fn scrollbar_dragged(&self) -> Option<GroupId> {
        match self.drag {
            Some(Drag::ScrollBar { group, .. }) => Some(group),
            _ => None,
        }
    }

    /// The scroll position of a group's active tab, as its bar draws it.
    fn group_scroll(&self, g: GroupId) -> Option<Scroll> {
        let group = self.groups.get(g)?;
        match group.tabs.get(group.active)? {
            Tab::Text(t) => {
                let lines = pos::line_count(&self.buffers.get(t.buffer)?.rope);
                Some(Scroll::text(lines, t.view.height, t.view.top_line()))
            }
            Tab::Diff(d) => Some(Scroll::rows(d.rows().len(), d.height, d.top)),
        }
    }

    /// Scrolls a group's active tab to `top`, dropping a pending reveal of its cursor (the
    /// pointer decides where the view is).
    fn set_group_top(&mut self, g: GroupId, top: usize) {
        let Some(group) = self.groups.get_mut(g) else { return };
        match group.tabs.get_mut(group.active) {
            Some(Tab::Text(t)) => {
                if let Some(buf) = self.buffers.get(t.buffer) {
                    t.view.set_top_line(&buf.rope, top);
                    t.view.reveal_cursor = false;
                    t.view.reveal_range = None;
                    t.view.reveal_pos = None;
                }
            }
            Some(Tab::Diff(d)) => {
                d.top = top;
                d.reveal = false;
            }
            None => {}
        }
    }

    fn scrollbar_hit(&self, g: GroupId) -> Option<ScrollBarHit> {
        self.hits.group(g)?.scrollbar
    }

    /// A left press on a group's scroll bar at row `y`.
    pub(super) fn scrollbar_click(&mut self, g: GroupId, y: u16) {
        let Some(hit) = self.scrollbar_hit(g) else { return };
        let (Some(thumb), Some(scroll)) = (hit.thumb, self.group_scroll(g)) else {
            return;
        };
        let cell = y.saturating_sub(hit.track.y);
        let grab = if thumb.contains(cell) {
            cell - thumb.start
        } else {
            let grab = thumb.len / 2;
            let top = scroll.top_at(hit.track.height, i32::from(cell) - i32::from(grab));
            self.set_group_top(g, top);
            grab
        };
        self.drag = Some(Drag::ScrollBar { group: g, grab });
    }

    /// The pointer at row `y` while dragging group `g`'s thumb by its cell `grab`.
    pub(super) fn scrollbar_drag(&mut self, g: GroupId, y: u16, grab: u16) {
        let (Some(hit), Some(scroll)) = (self.scrollbar_hit(g), self.group_scroll(g)) else {
            return;
        };
        let start = i32::from(y) - i32::from(hit.track.y) - i32::from(grab);
        self.set_group_top(g, scroll.top_at(hit.track.height, start));
    }

    /// Every pointer move: which thumb (if any) it is over.
    pub(super) fn scrollbar_hover(&mut self, m: &MouseEvent, hit: Hit) {
        self.ui.scrollbar_hover = match hit {
            Hit::ScrollBar(g) => self
                .scrollbar_hit(g)
                .filter(|s| s.thumb.is_some_and(|t| t.contains(m.row.saturating_sub(s.track.y))))
                .map(|_| g),
            _ => None,
        };
    }
}
