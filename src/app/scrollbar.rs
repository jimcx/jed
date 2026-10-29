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
use crate::group::{Tab, is_terminal_group};
use crate::layout::{Hit, PanelTab, ScrollTarget, SidePanel};
use crate::problems;
use crate::scrollbar::{Columns, Scroll, Scrollable};
use crate::tasks::TerminalScroll;
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
                // Either area's group (a field borrow: the text arm borrows `self.buffers` too).
                let groups = if is_terminal_group(g) {
                    &mut self.terminals
                } else {
                    &mut self.groups
                };
                let group = groups.get_mut(g)?;
                match group.tabs.get_mut(group.active)? {
                    Tab::Text(t) => {
                        let rope = &self.buffers.get(t.buffer)?.rope;
                        Some(f(&mut TextScroll {
                            view: &mut t.view,
                            rope,
                        }))
                    }
                    Tab::Diff(d) => Some(f(d.as_mut())),
                    Tab::Hex(h) => Some(f(h.as_mut())),
                    Tab::Shell(s) => Some(f(&mut TerminalScroll { run: &s.run })),
                }
            }
            ScrollTarget::Columns(g) => {
                let group = self.groups.get_mut(g)?;
                match group.tabs.get_mut(group.active)? {
                    Tab::Text(t) => Some(f(&mut Columns {
                        scroll_width: t.view.scroll_width,
                        width: t.view.width,
                        left_col: &mut t.view.left_col,
                    })),
                    Tab::Diff(d) => Some(f(&mut Columns {
                        scroll_width: d.scroll_width,
                        width: d.text_width,
                        left_col: &mut d.left_col,
                    })),
                    Tab::Hex(_) => None,
                    Tab::Shell(_) => None,
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
            ScrollTarget::Panel(PanelTab::Task(id)) => Some(f(&mut TerminalScroll {
                run: self.tasks.get(id)?,
            })),
            ScrollTarget::History if !self.ui.side_visible || self.ui.side_panel != SidePanel::Explorer => None,
            ScrollTarget::History => Some(f(&mut self.git_history.scrollable())),
            ScrollTarget::CheatSheet => Some(f(self.cheat_sheet.as_mut()?)),
            ScrollTarget::Keys => Some(f(self.keys.as_mut()?)),
        }
    }

    fn target_scroll(&mut self, target: ScrollTarget) -> Option<Scroll> {
        self.with_scrollable(target, |s| s.scroll())
    }

    fn set_target_top(&mut self, target: ScrollTarget, top: usize) {
        self.with_scrollable(target, |s| s.set_top(top));
    }

    /// The pointer's cell along `target`'s track (a column of cells, or a row for the horizontal
    /// bar) and the track's length.
    fn along(target: ScrollTarget, track: ratatui::layout::Rect, x: u16, y: u16) -> (u16, u16) {
        match target {
            ScrollTarget::Columns(_) => (x.saturating_sub(track.x), track.width),
            _ => (y.saturating_sub(track.y), track.height),
        }
    }

    /// Thumb positions per track cell: 2 for the vertical bars (half blocks), 1 for the horizontal.
    fn resolution(target: ScrollTarget) -> u16 {
        match target {
            ScrollTarget::Columns(_) => 1,
            _ => 2,
        }
    }

    /// A left press on `target`'s scroll bar at (`x`, `y`).
    pub(super) fn scrollbar_click(&mut self, target: ScrollTarget, x: u16, y: u16) {
        let Some(hit) = self.hits.scrollbar(target) else { return };
        let Some(thumb) = hit.thumb else { return };
        let Some(scroll) = self.target_scroll(target) else {
            return;
        };
        let (cell, len) = Self::along(target, hit.track, x, y);
        // The vertical bar's thumb moves in half rows ([`Self::resolution`]).
        let k = Self::resolution(target);
        let (pos, len) = (cell * k, len * k);
        let grab = if thumb.contains(cell) {
            let start = scroll.thumb(len).map_or(0, |t| t.start);
            pos.saturating_sub(start)
        } else {
            let grab = scroll.thumb(len).map_or(0, |t| t.len / 2);
            let top = scroll.top_at(len, i32::from(pos) - i32::from(grab));
            self.set_target_top(target, top);
            grab
        };
        self.drag = Some(Drag::ScrollBar { target, grab });
    }

    /// The pointer at (`x`, `y`) while dragging `target`'s thumb by its cell `grab`.
    pub(super) fn scrollbar_drag(&mut self, target: ScrollTarget, x: u16, y: u16, grab: u16) {
        let Some(hit) = self.hits.scrollbar(target) else { return };
        let Some(scroll) = self.target_scroll(target) else {
            return;
        };
        let k = i32::from(Self::resolution(target));
        let start = k * match target {
            ScrollTarget::Columns(_) => i32::from(x) - i32::from(hit.track.x),
            _ => i32::from(y) - i32::from(hit.track.y),
        } - i32::from(grab);
        let len = Self::along(target, hit.track, x, y).1 * k as u16;
        self.set_target_top(target, scroll.top_at(len, start));
    }

    /// Every pointer move: which thumb (if any) it is over.
    pub(super) fn scrollbar_hover(&mut self, m: &MouseEvent, hit: Hit) {
        let over = match hit {
            Hit::ScrollBar(t) => self
                .hits
                .scrollbar(t)
                .filter(|s| {
                    s.thumb
                        .is_some_and(|th| th.contains(Self::along(t, s.track, m.column, m.row).0))
                })
                .map(|_| t),
            _ => None,
        };
        if over != self.ui.scrollbar_hover {
            self.ui.scrollbar_hover = over;
            self.needs_redraw = true;
        }
    }
}
