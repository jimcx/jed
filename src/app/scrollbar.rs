//! Scroll bars (`ui::scrollbar` draws them, `scrollbar` has the maths): the editors', the side
//! bar's list (explorer, Search results, Source Control, Tools) and the bottom panel's (Problems,
//! Locations), each named by a `ScrollTarget`. A press on the thumb grabs it; one on the track
//! jumps the view so the thumb centres on the pointer (VS Code's default, `scrollbar.scrollByPage`
//! off) and grabs it there. Dragging scrolls keeping the grab, the pointer anywhere, until the
//! button is released. Nothing is focused and no cursor or list selection moves, as with the
//! wheel.

use crossterm::event::MouseEvent;

use super::App;
use super::mouse::Drag;
use crate::group::Tab;
use crate::layout::{Hit, PanelTab, ScrollTarget, SidePanel};
use crate::problems;
use crate::scrollbar::{Scroll, Scrollable};
use crate::view::TextScroll;

impl App {
    /// The scroll bar being dragged (drawn in the active colour).
    pub fn scrollbar_dragged(&self) -> Option<ScrollTarget> {
        match self.drag {
            Some(Drag::ScrollBar { target, .. }) => Some(target),
            _ => None,
        }
    }

    /// Runs `f` on what `target`'s bar scrolls: a group's active tab, the list the side bar or
    /// the panel shows (`None` when it's gone). A new scrollable list adds its arm here (and
    /// its drawing reports its bar under the same target).
    pub(super) fn with_scrollable<R>(
        &mut self,
        target: ScrollTarget,
        f: impl FnOnce(&mut dyn Scrollable) -> R,
    ) -> Option<R> {
        match target {
            ScrollTarget::Group(g) => {
                let group = self.groups.get_mut(g)?;
                match group.tabs.get_mut(group.active)? {
                    Tab::Text(t) => {
                        let rope = &self.buffers.get(t.buffer)?.rope;
                        Some(f(&mut TextScroll {
                            view: &mut t.view,
                            rope,
                        }))
                    }
                    Tab::Diff(d) => Some(f(d.as_mut())),
                }
            }
            ScrollTarget::Side(panel) if !self.ui.side_visible || self.ui.side_panel != panel => None,
            ScrollTarget::Side(SidePanel::Explorer) => Some(f(&mut self.explorer)),
            ScrollTarget::Side(SidePanel::Search) => {
                let results = self.search.as_ref()?;
                Some(f(&mut self.search_view.list.scrollable(&results.files)))
            }
            ScrollTarget::Side(SidePanel::SourceControl) => {
                let rows = self.scm.rows(self.git.status.as_ref()?);
                Some(f(&mut self.scm.scrollable(&rows)))
            }
            ScrollTarget::Side(SidePanel::Tools) => {
                let rows = self.tools_rows().len();
                Some(f(&mut self.tools_view.scrollable(rows)))
            }
            ScrollTarget::Panel(tab) if !self.ui.panel_visible || self.ui.panel_tab != tab => None,
            ScrollTarget::Panel(PanelTab::Problems) => {
                let files = problems::files(&self.lsp.diagnostics, &self.buffers);
                Some(f(&mut self.problems.list.scrollable(&files)))
            }
            ScrollTarget::Panel(PanelTab::Locations) => {
                let l = self.locations.as_mut()?;
                Some(f(&mut l.list.scrollable(&l.files)))
            }
            ScrollTarget::Panel(PanelTab::Task(_)) => None,
            ScrollTarget::History if !self.ui.side_visible || self.ui.side_panel != SidePanel::Explorer => None,
            ScrollTarget::History => Some(f(&mut self.git_history.scrollable())),
        }
    }

    fn target_scroll(&mut self, target: ScrollTarget) -> Option<Scroll> {
        self.with_scrollable(target, |s| s.scroll())
    }

    fn set_target_top(&mut self, target: ScrollTarget, top: usize) {
        self.with_scrollable(target, |s| s.set_top(top));
    }

    /// A left press on `target`'s scroll bar at row `y`.
    pub(super) fn scrollbar_click(&mut self, target: ScrollTarget, y: u16) {
        let Some(hit) = self.hits.scrollbar(target) else { return };
        let Some(thumb) = hit.thumb else { return };
        let Some(scroll) = self.target_scroll(target) else {
            return;
        };
        let cell = y.saturating_sub(hit.track.y);
        let grab = if thumb.contains(cell) {
            cell - thumb.start
        } else {
            let grab = thumb.len / 2;
            let top = scroll.top_at(hit.track.height, i32::from(cell) - i32::from(grab));
            self.set_target_top(target, top);
            grab
        };
        self.drag = Some(Drag::ScrollBar { target, grab });
    }

    /// The pointer at row `y` while dragging `target`'s thumb by its cell `grab`.
    pub(super) fn scrollbar_drag(&mut self, target: ScrollTarget, y: u16, grab: u16) {
        let Some(hit) = self.hits.scrollbar(target) else { return };
        let Some(scroll) = self.target_scroll(target) else {
            return;
        };
        let start = i32::from(y) - i32::from(hit.track.y) - i32::from(grab);
        self.set_target_top(target, scroll.top_at(hit.track.height, start));
    }

    /// Every pointer move: which thumb (if any) it is over.
    pub(super) fn scrollbar_hover(&mut self, m: &MouseEvent, hit: Hit) {
        let over = match hit {
            Hit::ScrollBar(t) => self
                .hits
                .scrollbar(t)
                .filter(|s| s.thumb.is_some_and(|th| th.contains(m.row.saturating_sub(s.track.y))))
                .map(|_| t),
            _ => None,
        };
        if over != self.ui.scrollbar_hover {
            self.ui.scrollbar_hover = over;
            self.needs_redraw = true;
        }
    }
}
