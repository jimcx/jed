//! The commands side bar's behaviour (`side` holds its state, `ui_side` draws it): its commands and
//! keys, the filter box, the mouse (rows, scroll bar, wheel, the border drag), jumping to a
//! command's terminal and output, and the once-a-second tick that keeps durations and ages current
//! while the bar shows.

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use jtui::field::{FieldCommand, Outcome};
use jtui::scrollbar::Scroll;

use super::{App, Drag, WHEEL_LINES};
use crate::command::SideCommand;
use crate::history::RowKey;
use crate::hit::Hit;
use crate::side;

impl App {
    pub(super) fn side_command(&mut self, cmd: SideCommand) {
        let h = self.side.view_rows;
        match cmd {
            SideCommand::Toggle => {
                self.side.visible = !self.side.visible;
                if !self.side.visible {
                    self.side.focused = false;
                    if matches!(self.drag, Some(Drag::SideBorder | Drag::SideScroll { .. })) {
                        self.drag = None;
                    }
                }
            }
            SideCommand::Focus => {
                self.side.visible = true;
                self.side.focused = true;
            }
            SideCommand::Up => self.history.move_selection(-1, h),
            SideCommand::Down => self.history.move_selection(1, h),
            SideCommand::PageUp => self.history.move_selection(-(h.saturating_sub(1).max(1) as isize), h),
            SideCommand::PageDown => self.history.move_selection(h.saturating_sub(1).max(1) as isize, h),
            SideCommand::First => self.history.select_first(h),
            SideCommand::Last => self.history.select_last(h),
            SideCommand::Open => {
                if let Some(key) = self.history.selected().map(|r| r.key()) {
                    self.open_row(key);
                }
            }
            SideCommand::Back => self.side.focused = false,
        }
    }

    /// A filter edit command (`Command::Field` while the bar has the keys).
    pub(super) fn side_edit(&mut self, cmd: FieldCommand) {
        if self.side.filter.run(cmd) == Outcome::Edited {
            self.apply_filter();
        }
    }

    /// A key no binding took, while the bar has the keys: typed into the filter, else swallowed.
    pub(super) fn side_type(&mut self, key: KeyEvent) {
        if self.side.filter.key(key) == Outcome::Edited {
            self.apply_filter();
        }
    }

    pub(super) fn side_paste(&mut self, text: &str) {
        self.side.filter.insert(&text.replace(['\r', '\n'], ""));
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let text = self.side.filter.text.clone();
        self.history.set_filter(&text);
        self.history.reveal_selected(self.side.view_rows);
    }

    /// Goes to the command's terminal and shows its first row at the top of the view. A row of a
    /// closed terminal does nothing; output the scrollback no longer holds only focuses the tab,
    /// with a notice.
    fn open_row(&mut self, key: RowKey) {
        let Some(row) = self.history.rows().iter().find(|r| r.key() == key) else {
            return;
        };
        if row.closed {
            return;
        }
        let (run, id) = key;
        let Some((group, index)) = self.groups.find(run) else {
            return;
        };
        self.groups.active = group;
        self.side.focused = false;
        let mut found = false;
        if let Some(g) = self.groups.get_mut(group) {
            g.active = index;
            found = g.tabs.get(index).is_some_and(|t| t.run.scroll_to_command(id));
        }
        if !found {
            self.notice = Some("That command's output is no longer in the scrollback".into());
        }
    }

    /// Once a second: the durations and ages in the rows move on, so a frame is due, but only
    /// while the bar shows them.
    pub(super) fn tick(&mut self) {
        if self.side.visible && self.attached.is_some() && !self.history.is_empty() {
            self.needs_redraw = true;
        }
    }

    /// A pointer event over the side bar (`hit` says which part); false when it is not one of
    /// the bar's.
    pub(super) fn side_mouse(&mut self, hit: Hit, m: MouseEvent) -> bool {
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        let h = self.side.view_rows;
        match (hit, m.kind) {
            (Hit::SideBorder, _) if left => self.drag = Some(Drag::SideBorder),
            (Hit::SideFilter, _) if left => {
                self.side.focused = true;
                if let Some(s) = &self.hit.side {
                    let at = self
                        .side
                        .filter
                        .offset_at_col(i64::from(m.column) - i64::from(s.filter.x), s.filter_scroll);
                    self.side.filter.click(at, 1, false);
                }
            }
            (Hit::SideRow(i), _) if left => {
                self.history.select_index(i, h);
                if let Some(key) = self.history.selected().map(|r| r.key()) {
                    self.open_row(key);
                }
            }
            (Hit::SideScrollBar, _) if left => self.side_scroll_click(m.row),
            (Hit::Side, _) if left => self.side.focused = true,
            (Hit::SideBorder | Hit::SideFilter | Hit::SideRow(_) | Hit::SideScrollBar | Hit::Side, kind) => {
                match kind {
                    MouseEventKind::ScrollUp => self.history.scroll_by(-WHEEL_LINES, h),
                    MouseEventKind::ScrollDown => self.history.scroll_by(WHEEL_LINES, h),
                    _ => return true,
                }
            }
            _ => return false,
        }
        true
    }

    /// The border dragged to column `x`.
    pub(super) fn side_resize(&mut self, x: u16) {
        self.side.width = side::clamp_width(x, self.size.1);
    }

    /// The list's scroll bar as the terminal's (`scrollbar_click`): on the thumb it is grabbed
    /// where it was pressed, off it the thumb jumps there and is grabbed.
    fn side_scroll_click(&mut self, y: u16) {
        let Some(hit) = self.hit.side.as_ref().map(|s| s.scroll) else {
            return;
        };
        let Some(thumb) = hit.thumb else {
            return;
        };
        let scroll = self.list_scroll();
        let cell = y.saturating_sub(hit.track.y);
        let (pos, len) = (cell * 2, hit.track.height * 2);
        let grab = if thumb.contains(cell) {
            pos.saturating_sub(scroll.thumb(len).map_or(0, |t| t.start))
        } else {
            let grab = scroll.thumb(len).map_or(0, |t| t.len / 2);
            self.set_list_top(scroll.top_at(len, i32::from(pos) - i32::from(grab)));
            grab
        };
        self.drag = Some(Drag::SideScroll { grab });
    }

    /// The scroll bar's thumb dragged to row `y`, held by `grab`.
    pub(super) fn side_scroll_drag(&mut self, y: u16, grab: u16) {
        let Some(hit) = self.hit.side.as_ref().map(|s| s.scroll) else {
            return;
        };
        let scroll = self.list_scroll();
        let len = hit.track.height * 2;
        let start = 2 * (i32::from(y) - i32::from(hit.track.y)) - i32::from(grab);
        self.set_list_top(scroll.top_at(len, start));
    }

    fn list_scroll(&self) -> Scroll {
        Scroll::rows(self.history.visible().len(), self.side.view_rows, self.history.scroll())
    }

    fn set_list_top(&mut self, top: usize) {
        let delta = top as isize - self.history.scroll() as isize;
        self.history.scroll_by(delta, self.side.view_rows);
    }
}
