//! Mouse: clicks, double/triple clicks, drags, wheel.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::{App, Focus};
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::column::{self, Cell};
use crate::command::{Command, ExplorerCommand};
use crate::layout::{Divider, Hit, MIN_PANEL_HEIGHT, MIN_SIDE_WIDTH, PanelTab, ScrollTarget, SidePanel, SplitDir};
use crate::selection::{Selection, Selections};
use crate::terminal::Pointer;
use crate::view::View;

const MULTI_CLICK: Duration = Duration::from_millis(500);
const WHEEL_LINES: isize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    SideBorder,
    /// Resizing the bottom panel by its top border.
    PanelBorder,
    /// Resizing a split.
    Divider(Divider),
    /// Extending a selection; the anchor range is kept for word/line drags.
    Text {
        anchor: (ByteIdx, ByteIdx),
    },
    /// Alt+drag: extending the newest selection (the one Alt+Click added) from `anchor`.
    AddedText {
        anchor: ByteIdx,
    },
    /// Shift+Alt+drag: a box selection (its corners live in `view.column`).
    Column,
    /// A scroll bar's thumb, held by its cell `grab` (`app/scrollbar.rs`).
    ScrollBar {
        target: ScrollTarget,
        grab: u16,
    },
    /// The Explorer view's Git History divider (its row sets the section's height).
    HistoryDivider,
    /// Selecting bytes in a group's hex tab (`app/hex.rs`).
    Hex(crate::layout::GroupId),
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
        self.mouse_event(m);
        self.pointer = self.pointer_at(m.column, m.row);
    }

    /// The pointer's shape: a resize arrow over a border that drags (and while dragging one,
    /// wherever the pointer is), else the terminal's own.
    fn pointer_at(&self, x: u16, y: u16) -> Pointer {
        let divider = |d: &Divider| match d.dir {
            SplitDir::Right => Pointer::ColResize,
            SplitDir::Down => Pointer::RowResize,
        };
        match &self.drag {
            Some(Drag::SideBorder) => Pointer::ColResize,
            Some(Drag::PanelBorder | Drag::HistoryDivider) => Pointer::RowResize,
            Some(Drag::Divider(d)) => divider(d),
            Some(_) => Pointer::Default,
            None => match self.hits.hit(x, y) {
                Hit::SideBorder => Pointer::ColResize,
                Hit::PanelBorder | Hit::HistoryDivider => Pointer::RowResize,
                Hit::Divider(i) => self.hits.dividers.get(i).map_or(Pointer::Default, divider),
                _ => Pointer::Default,
            },
        }
    }

    fn mouse_event(&mut self, m: MouseEvent) {
        let hit = match self.hits.hit(m.column, m.row) {
            // The wheel over a scroll bar scrolls what it scrolls, as the wheel over that.
            Hit::ScrollBar(t) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) => match t {
                ScrollTarget::Group(g) => Hit::Editor(g),
                ScrollTarget::Side(_) => Hit::SidePanel,
                ScrollTarget::Panel(_) => Hit::Panel,
                // The wheel over the history's bar scrolls its list, as over its rows.
                ScrollTarget::History => Hit::HistoryRow(0),
            },
            hit => hit,
        };
        if self.code_actions_mouse(&m, hit) {
            return;
        }
        if self.completion_mouse(&m, hit) {
            return;
        }
        if self.hover_mouse(&m, hit) {
            return;
        }
        if self.peek_mouse(&m, hit) {
            return;
        }
        if self.find_mouse(m) {
            return;
        }
        if self.diff_mouse(&m, hit) || self.hex_mouse(&m, hit) {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if m.kind == MouseEventKind::ScrollUp {
                    -WHEEL_LINES
                } else {
                    WHEEL_LINES
                };
                match hit {
                    Hit::SidePanel if self.ui.side_panel == SidePanel::Search => self.search_scroll(delta, &m),
                    Hit::SidePanel if self.ui.side_panel == SidePanel::SourceControl => self.scm_scroll(delta),
                    Hit::SidePanel if self.ui.side_panel == SidePanel::Tools => self.tools_scroll(delta),
                    Hit::HistoryRow(_) | Hit::HistoryHeader => self.git_history.scroll_by(delta),
                    Hit::ExplorerRow(_) | Hit::SidePanel => self.explorer.scroll_by(delta),
                    // The wheel over the tab bar moves it a tab at a time (never focusing it).
                    Hit::TabBar(g) | Hit::Tab(g, _) | Hit::TabClose(g, _) => {
                        if let Some(group) = self.groups.get_mut(g) {
                            group.scroll_tabs(delta.signum());
                        }
                    }
                    Hit::Panel | Hit::PanelTab(_) | Hit::PanelTabClose(_) | Hit::PanelBorder => match self.ui.panel_tab
                    {
                        PanelTab::Problems => self.problems_scroll(delta),
                        PanelTab::Locations => self.locations_scroll(delta),
                        PanelTab::Task(_) => self.task_scroll(delta),
                    },
                    // Scrolls the group under the mouse, without focusing it.
                    Hit::Editor(g) => {
                        let tab = self.groups.get_mut(g).and_then(|g| g.active_tab_mut());
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
            MouseEventKind::Down(MouseButton::Middle) => match hit {
                Hit::Tab(g, i) | Hit::TabClose(g, i) => self.close_tab(g, i),
                Hit::PanelTab(PanelTab::Task(id)) | Hit::PanelTabClose(id) => self.close_task(id),
                _ => {}
            },
            MouseEventKind::Down(MouseButton::Right) => self.right_click(hit),
            MouseEventKind::Drag(MouseButton::Left) => match self.drag.clone() {
                Some(Drag::SideBorder) => {
                    if let Some(side) = self.hits.regions.side {
                        self.ui.side_width = (m.column + 1).saturating_sub(side.x).max(MIN_SIDE_WIDTH);
                    }
                }
                Some(Drag::PanelBorder) => {
                    // Its top row follows the pointer; `layout::compute` keeps both minimums.
                    let bottom = self.hits.regions.status.y;
                    self.ui.panel_height = bottom
                        .saturating_sub(m.row)
                        .clamp(MIN_PANEL_HEIGHT, bottom.max(MIN_PANEL_HEIGHT));
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
                Some(Drag::AddedText { anchor }) => {
                    let at = self.text_position(m.column, m.row, true);
                    if let (Some(at), Some((_, view))) = (at, self.active_mut()) {
                        let mut list = view.selections.as_slice().to_vec();
                        let newest = view.selections.newest_index();
                        list[newest] = Selection::new(anchor, at);
                        view.selections = Selections::with_newest(list, view.selections.primary_index(), newest);
                        view.reveal_pos = Some(at);
                    }
                }
                Some(Drag::Column) => {
                    if let Some(to) = self.text_cell(m.column, m.row, true) {
                        self.column_select_to(to);
                    }
                }
                Some(Drag::ScrollBar { target, grab }) => self.scrollbar_drag(target, m.row, grab),
                // Taken by `hex_mouse`.
                Some(Drag::Hex(_)) => {}
                Some(Drag::HistoryDivider) => {
                    // The divider's row is the section's top; the section reaches the body's end
                    // (the side panel's last row, above the status bar).
                    if let Some(side) = self.hits.regions.side {
                        let bottom = side.y + side.height;
                        let body = bottom.saturating_sub(side.y + 2);
                        self.git_history.set_section_height(bottom.saturating_sub(m.row), body);
                    }
                }
                None => {}
            },
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                // No moves arrive during a drag: the thumb released away from the pointer
                // mustn't keep the hover colour from before the press.
                self.scrollbar_hover(&m, hit);
            }
            MouseEventKind::Moved => {
                self.scm_hover(&m, hit);
                self.search_hover(&m, hit);
                self.scrollbar_hover(&m, hit);
            }
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
                self.git_history.set_focused(false);
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
            Hit::SidePanel if self.ui.side_panel == SidePanel::Search => self.search_click(m, count),
            Hit::SidePanel if self.ui.side_panel == SidePanel::SourceControl => self.scm_click(m, count),
            Hit::SidePanel if self.ui.side_panel == SidePanel::Tools => self.tools_click(m),
            // The history's header toggles the collapse; a commit row selects it.
            Hit::HistoryHeader => {
                self.ui.focus = Focus::SidePanel;
                self.git_history.set_focused(true);
                self.git_history.toggle_collapsed();
            }
            Hit::HistoryRow(row) => {
                self.ui.focus = Focus::SidePanel;
                self.git_history.set_focused(true);
                let top = self.hits.history.map_or(0, |h| h.top);
                self.git_history.select_row(top + row as usize);
                // A single click opens the commit (Enter's too).
                if let Some(id) = self.git_history.selected().map(str::to_string) {
                    let file = self.git_history_file();
                    self.open_commit(&id, file);
                }
            }
            Hit::HistoryDivider => self.drag = Some(Drag::HistoryDivider),
            Hit::ExplorerRow(_) | Hit::SidePanel => {
                self.ui.focus = Focus::SidePanel;
                if self.ui.side_panel == SidePanel::Explorer {
                    self.git_history.set_focused(false);
                }
            }
            Hit::Divider(i) => self.drag = self.hits.dividers.get(i).cloned().map(Drag::Divider),
            Hit::Tab(g, i) => {
                self.focus_group(g);
                self.activate_tab(i);
            }
            Hit::TabClose(g, i) => self.close_tab(g, i),
            Hit::TabBar(g) => self.focus_group(g),
            Hit::TabArrow(g, forward) => {
                self.focus_group(g);
                if let Some(group) = self.groups.get_mut(g) {
                    group.scroll_tabs(if forward { 1 } else { -1 });
                }
            }
            Hit::Banner(g, action) => {
                self.focus_group(g);
                if let Some(a) = action {
                    self.execute(a.command());
                }
            }
            Hit::Editor(_) if m.modifiers.is_empty() && self.bulb_click(&m) => {}
            Hit::Editor(_) if m.modifiers.is_empty() && self.gutter_click(&m) => {}
            Hit::Editor(g) => {
                self.focus_group(g);
                self.editor_click(m, count);
            }
            // Scrolls without focusing anything or moving a cursor or a list's selection.
            Hit::ScrollBar(t) => self.scrollbar_click(t, m.row),
            Hit::PanelBorder => self.drag = Some(Drag::PanelBorder),
            Hit::PanelTab(tab) => self.focus_panel(tab),
            Hit::PanelTabClose(id) => self.close_task(id),
            Hit::Panel => match self.ui.panel_tab {
                PanelTab::Problems => self.problems_click(m, count),
                PanelTab::Locations => self.locations_click(m, count),
                PanelTab::Task(_) => self.task_click(m),
            },
            Hit::StatusProblems => self.problem_counts_clicked(),
            Hit::Status | Hit::Hover | Hit::Completion | Hit::CodeActions | Hit::QuickDiff | Hit::Nothing => {}
        }
    }

    /// Right-click (jed has no context menus): on a tab, its file's history ("File History:
    /// Show", the tab activated first); on an explorer file, that file's; on an explorer folder,
    /// "Find in Folder…" (the row selected first).
    fn right_click(&mut self, hit: Hit) {
        match hit {
            Hit::Tab(g, i) | Hit::TabClose(g, i) => {
                self.focus_group(g);
                self.activate_tab(i);
                self.execute(Command::ShowFileHistory);
            }
            Hit::ExplorerRow(row) if self.ui.side_panel == SidePanel::Explorer => {
                let index = self.explorer.scroll + row as usize;
                let Some(path) = self.explorer.rows().get(index).map(|r| r.entry.path.clone()) else {
                    return;
                };
                // A folder: "Find in Folder…"; a file: its history.
                let is_dir = self.explorer.find(&path).is_some_and(|e| e.is_dir);
                self.ui.focus = Focus::SidePanel;
                self.explorer.selected = Some(path);
                let cmd = if is_dir {
                    ExplorerCommand::FindInFolder
                } else {
                    ExplorerCommand::ShowHistory
                };
                self.execute(Command::Explorer(cmd));
            }
            _ => {}
        }
    }

    fn editor_click(&mut self, m: MouseEvent, count: u8) {
        self.ui.focus = Focus::Editor;
        let alt = m.modifiers.contains(KeyModifiers::ALT);
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
        if alt && shift {
            // Shift+Alt+click / drag: a box from the current selection's anchor (VS Code).
            if let Some(to) = self.text_cell(m.column, m.row, false) {
                self.column_select_to(to);
                self.drag = Some(Drag::Column);
            }
            return;
        }
        let Some(at) = self.text_position(m.column, m.row, false) else {
            return;
        };
        let Some((buf, view)) = self.active_mut() else { return };
        buf.seal_undo();
        let rope = &buf.rope;
        // A commit tab's stat line: a click opens that file's change.
        let stat = buf.commit.as_ref().and_then(|c| c.file_at(pos::line_of(rope, at)));
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
        if alt {
            self.drag = alt_click(view, at, (start, end), count);
            return;
        }
        view.selections = if shift && count == 1 {
            Selections::single(Selection::new(view.selections.primary().anchor, at))
        } else {
            Selections::single(Selection::new(start, end))
        };
        // Cmd+Click (Ctrl+Click on Linux; mouse reports carry Ctrl, rarely Cmd): go to definition.
        let go = m.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER);
        if go && !shift && count == 1 {
            self.go_to_definition();
            return;
        }
        if let Some(k) = stat
            && !shift
            && count == 1
        {
            self.open_commit_diff(self.groups.active, k);
            return;
        }
        self.drag = Some(Drag::Text { anchor: (start, end) });
    }

    /// Box-selects in the active view from its current box corner (or its primary selection's
    /// anchor) to `to`.
    fn column_select_to(&mut self, to: Cell) {
        let settings = self.settings.clone();
        if let Some((buf, view)) = self.active_mut() {
            buf.seal_undo();
            let tab_width = settings.for_buffer(buf).tab_width;
            let (from, _) = column::current(&buf.rope, view, tab_width);
            column::select(&buf.rope, view, from, to, tab_width);
        }
    }

    /// Screen cell to line and display column in the active group, the column not limited to
    /// the line's text. With `clamp`, as `text_position`.
    fn text_cell(&self, x: u16, y: u16, clamp: bool) -> Option<Cell> {
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
        let line = (tab.view.top_line() + (y - a.y) as usize).min(pos::line_count(&buf.rope) - 1);
        Some((line, DisplayCol(tab.view.left_col + (x - a.x) as usize)))
    }

    /// Screen cell to buffer position in the active group. With `clamp`, points outside
    /// the text area (while dragging) are pulled to its edge.
    fn text_position(&self, x: u16, y: u16, clamp: bool) -> Option<ByteIdx> {
        let (line, col) = self.text_cell(x, y, clamp)?;
        let buf = self.buffers.get(self.group().active_tab()?.buffer)?;
        Some(pos::byte_at_col(
            &buf.rope,
            line,
            col,
            self.settings.for_buffer(buf).tab_width,
        ))
    }
}

/// Alt+Click (VS Code): with several selections, clicking on or in one removes it; otherwise a
/// cursor is added there and becomes the newest. A double / triple Alt+Click makes the newest
/// selection the word / line instead. Returns the drag that follows (extending the new cursor).
fn alt_click(view: &mut View, at: ByteIdx, range: (ByteIdx, ByteIdx), count: u8) -> Option<Drag> {
    let sels = &view.selections;
    let (primary, newest) = (sels.primary_index(), sels.newest_index());
    let mut list = sels.as_slice().to_vec();
    if count >= 2 {
        list[newest] = Selection::new(range.0, range.1);
        view.selections = Selections::with_newest(list, primary, newest);
        return None;
    }
    if list.len() > 1
        && let Some(i) = list.iter().position(|s| s.start() <= at && at <= s.end())
    {
        list.remove(i);
        // Keep the primary and newest on their selections; a removed one passes to the last.
        let fix = |k: usize| match k.cmp(&i) {
            std::cmp::Ordering::Less => k,
            std::cmp::Ordering::Equal => list.len() - 1,
            std::cmp::Ordering::Greater => k - 1,
        };
        let (p, n) = (fix(primary), fix(newest));
        view.selections = Selections::with_newest(list, p, n);
        return None;
    }
    list.push(Selection::cursor(at));
    let newest = list.len() - 1;
    view.selections = Selections::with_newest(list, primary, newest);
    Some(Drag::AddedText { anchor: at })
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

    #[test]
    fn alt_click_removal_keeps_primary_and_newest_on_their_selections() {
        let heads = |v: &View| v.selections.iter().map(|s| s.head.0).collect::<Vec<_>>();
        let mut v = View::new();
        let list = [0, 2, 4, 6].map(|p| Selection::cursor(ByteIdx(p))).to_vec();
        v.selections = Selections::with_newest(list, 2, 3);
        let none = (ByteIdx(0), ByteIdx(0));
        // Removing one before both: they shift down with their selections.
        assert!(alt_click(&mut v, ByteIdx(0), none, 1).is_none());
        assert_eq!(heads(&v), vec![2, 4, 6]);
        assert_eq!((v.selections.primary().head.0, v.selections.newest().head.0), (4, 6));
        // Removing the primary passes it to the last selection.
        assert!(alt_click(&mut v, ByteIdx(4), none, 1).is_none());
        assert_eq!(heads(&v), vec![2, 6]);
        assert_eq!(v.selections.primary().head.0, 6);
        // A click inside a selection, at either end, removes it.
        v.selections = Selections::new(
            vec![Selection::new(ByteIdx(1), ByteIdx(3)), Selection::cursor(ByteIdx(6))],
            0,
        );
        alt_click(&mut v, ByteIdx(3), none, 1);
        assert_eq!(heads(&v), vec![6]);
        // The last one is never removed: a click on it adds a cursor that merges into it.
        assert!(alt_click(&mut v, ByteIdx(6), none, 1).is_some());
        assert_eq!(heads(&v), vec![6]);
        // Elsewhere: a new cursor, the newest, the primary unchanged.
        alt_click(&mut v, ByteIdx(1), none, 1);
        assert_eq!(heads(&v), vec![1, 6]);
        assert_eq!((v.selections.primary().head.0, v.selections.newest().head.0), (6, 1));
    }
}
