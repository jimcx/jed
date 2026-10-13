//! Mouse: clicks, double/triple clicks, drags, wheel.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::{App, Focus};
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::layout::{Divider, Hit, MIN_SIDE_WIDTH, SidePanel};
use crate::selection::{Selection, Selections};

const MULTI_CLICK: Duration = Duration::from_millis(500);
const WHEEL_LINES: isize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    SideBorder,
    /// Resizing a split.
    Divider(Divider),
    /// Extending a selection; the anchor range is kept for word/line drags.
    Text {
        anchor: (ByteIdx, ByteIdx),
    },
}

/// Counts clicks on the same cell in quick succession (1, 2, 3).
#[derive(Default)]
pub struct ClickTracker {
    last: Option<(Instant, u16, u16)>,
    count: u8,
}

impl ClickTracker {
    pub fn click(&mut self, now: Instant, x: u16, y: u16) -> u8 {
        self.count = match self.last {
            Some((t, lx, ly)) if lx == x && ly == y && now.duration_since(t) < MULTI_CLICK => self.count % 3 + 1,
            _ => 1,
        };
        self.last = Some((now, x, y));
        self.count
    }
}

impl App {
    pub(super) fn handle_mouse(&mut self, m: MouseEvent) {
        let hit = self.hits.hit(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if m.kind == MouseEventKind::ScrollUp {
                    -WHEEL_LINES
                } else {
                    WHEEL_LINES
                };
                match hit {
                    Hit::ExplorerRow(_) | Hit::SidePanel => self.explorer.scroll_by(delta),
                    // Scrolls the group under the mouse, without focusing it.
                    Hit::Editor(g) => {
                        let tab = self.groups.get_mut(g).and_then(|g| g.tabs.get_mut(g.active));
                        if let Some(tab) = tab
                            && let Some(buf) = self.buffers.get(tab.buffer)
                        {
                            tab.view.scroll_lines(&buf.rope, delta);
                        }
                    }
                    _ => {}
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let count = self.clicks.click(Instant::now(), m.column, m.row);
                self.left_click(hit, m, count);
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Hit::Tab(g, i) | Hit::TabClose(g, i) = hit {
                    self.close_tab(g, i);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag.clone() {
                Some(Drag::SideBorder) => {
                    if let Some(side) = self.hits.regions.side {
                        self.ui.side_width = (m.column + 1).saturating_sub(side.x).max(MIN_SIDE_WIDTH);
                    }
                }
                Some(Drag::Divider(d)) => self.groups.tree.set_ratio(&d, d.ratio_at(m.column, m.row)),
                Some(Drag::Text { anchor }) => {
                    let at = self.text_position(m.column, m.row, true);
                    if let (Some(at), Some((_, view))) = (at, self.active_mut()) {
                        // Dragging back past the anchor keeps the whole anchor word/line selected.
                        let (a, h) = if at < anchor.0 {
                            (anchor.1, at)
                        } else {
                            (anchor.0, at.max(anchor.1))
                        };
                        view.selections = Selections::single(Selection::new(a, h));
                        view.reveal_cursor = true;
                    }
                }
                None => {}
            },
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
    }

    fn left_click(&mut self, hit: Hit, m: MouseEvent, count: u8) {
        match hit {
            Hit::Icon(panel) => {
                if self.ui.side_visible && self.ui.side_panel == panel {
                    self.ui.side_visible = false;
                    self.ui.focus = Focus::Editor;
                } else {
                    self.focus_side(panel);
                }
            }
            Hit::SideBorder => self.drag = Some(Drag::SideBorder),
            Hit::ExplorerRow(row) if self.ui.side_panel == SidePanel::Explorer => {
                self.ui.focus = Focus::SidePanel;
                let index = self.explorer.scroll + row as usize;
                let rows = self.explorer.rows();
                let Some(entry) = rows.get(index).map(|r| (r.entry.path.clone(), r.entry.is_dir)) else {
                    return;
                };
                drop(rows);
                self.explorer.selected = Some(entry.0.clone());
                // Folders toggle on a single click; files open on a double click.
                if entry.1 || count >= 2 {
                    self.toggle_or_open(&entry.0, count >= 2);
                }
            }
            Hit::ExplorerRow(_) | Hit::SidePanel => self.ui.focus = Focus::SidePanel,
            Hit::Divider(i) => self.drag = self.hits.dividers.get(i).cloned().map(Drag::Divider),
            Hit::Tab(g, i) => {
                self.focus_group(g);
                self.activate_tab(i);
            }
            Hit::TabClose(g, i) => self.close_tab(g, i),
            Hit::TabBar(g) => self.focus_group(g),
            Hit::Editor(g) => {
                self.focus_group(g);
                self.editor_click(m, count);
            }
            Hit::Status | Hit::Nothing => {}
        }
    }

    fn editor_click(&mut self, m: MouseEvent, count: u8) {
        self.ui.focus = Focus::Editor;
        let Some(at) = self.text_position(m.column, m.row, false) else {
            return;
        };
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
        let Some((buf, view)) = self.active_mut() else { return };
        buf.seal_undo();
        let rope = &buf.rope;
        let (start, end) = match count {
            2 => pos::word_range(rope, at),
            3 => {
                let line = pos::line_of(rope, at);
                let next = if line + 1 < pos::line_count(rope) {
                    pos::line_start(rope, line + 1)
                } else {
                    ByteIdx(rope.len())
                };
                (pos::line_start(rope, line), next)
            }
            _ => (at, at),
        };
        view.selections = if shift && count == 1 {
            Selections::single(Selection::new(view.selections.primary().anchor, at))
        } else {
            Selections::single(Selection::new(start, end))
        };
        self.drag = Some(Drag::Text { anchor: (start, end) });
    }

    /// Screen cell to buffer position in the active group. With `clamp`, points outside
    /// the text area (while dragging) are pulled to its edge.
    fn text_position(&self, x: u16, y: u16, clamp: bool) -> Option<ByteIdx> {
        let a = self.hits.group(self.groups.active)?.text;
        let tab = self.group().active_tab()?;
        let buf = self.buffers.get(tab.buffer)?;
        if a.width == 0 || a.height == 0 {
            return None;
        }
        let (x, y) = if clamp {
            (x.clamp(a.x, a.x + a.width - 1), y.clamp(a.y, a.y + a.height - 1))
        } else {
            (x.max(a.x), y)
        };
        if y < a.y || y >= a.y + a.height {
            return None;
        }
        let rope = &buf.rope;
        let line = (tab.view.top_line() + (y - a.y) as usize).min(pos::line_count(rope) - 1);
        let col = tab.view.left_col + (x - a.x) as usize;
        Some(pos::byte_at_col(rope, line, DisplayCol(col), self.settings.tab_width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_counting() {
        let mut c = ClickTracker::default();
        let t = Instant::now();
        assert_eq!(c.click(t, 1, 1), 1);
        assert_eq!(c.click(t + Duration::from_millis(100), 1, 1), 2);
        assert_eq!(c.click(t + Duration::from_millis(200), 1, 1), 3);
        assert_eq!(c.click(t + Duration::from_millis(300), 1, 1), 1, "wraps after triple");
        assert_eq!(c.click(t + Duration::from_millis(350), 2, 1), 1, "different cell");
        assert_eq!(c.click(t + Duration::from_secs(2), 2, 1), 1, "too slow");
    }
}
