//! Mouse: clicks, double/triple clicks, drags, wheel.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ropey::Rope;
use vt100::MouseProtocolMode;

use super::tab_drag::TabDrag;
use super::{App, Focus};
use crate::buffer::pos::{self, ByteIdx, DisplayCol};
use crate::column::{self, Cell};
use crate::command::{Command, ExplorerCommand};
use crate::field::TextField;
use crate::find::Field;
use crate::group::{Tab, is_terminal_group};
use crate::layout::{
    Divider, Hit, MIN_PANEL_HEIGHT, MIN_SIDE_WIDTH, MIN_TERMINAL_HEIGHT, MergeButton, PanelTab, ScrollTarget,
    SidePanel, SplitDir, task_area,
};
use crate::search_view::Input;
use crate::selection::{Selection, Selections};
use crate::tasks::{Run, RunId, mouse, select};
use crate::terminal::Pointer;
use crate::view::View;

const MULTI_CLICK: Duration = Duration::from_millis(500);
pub(super) const WHEEL_LINES: isize = 3;
/// Columns text and diff views scroll sideways per wheel step (and a diff's Left / Right).
pub(super) const SIDE_STEP: isize = 4;

/// A sideways wheel step in columns: `ScrollLeft` / `ScrollRight`, or the vertical wheel with
/// Shift (VS Code); `None` for anything else.
pub(super) fn wheel_columns(m: &MouseEvent) -> Option<isize> {
    let shift = m.modifiers.contains(KeyModifiers::SHIFT);
    match m.kind {
        MouseEventKind::ScrollLeft => Some(-SIDE_STEP),
        MouseEventKind::ScrollRight => Some(SIDE_STEP),
        MouseEventKind::ScrollUp if shift => Some(-SIDE_STEP),
        MouseEventKind::ScrollDown if shift => Some(SIDE_STEP),
        _ => None,
    }
}

/// The screen cell `(row, col)` the pointer at `(x, y)` is over in `area`, in the visible-viewport
/// cells `vt100::Screen::cell` uses: the pointer clamped into `area`, shifted by the row a taller
/// screen is drawn from (`select::top_row`), then clamped into the screen.
fn screen_cell(screen: &vt100::Screen, area: Rect, x: u16, y: u16) -> (u16, u16) {
    let top = select::top_row(screen, area.height);
    let (rows, cols) = select::size(screen);
    let col = x.saturating_sub(area.x).min(area.width.saturating_sub(1));
    let row = y.saturating_sub(area.y).min(area.height.saturating_sub(1));
    (
        row.saturating_add(top).min(rows.saturating_sub(1)),
        col.min(cols.saturating_sub(1)),
    )
}

/// The (anchor, head) of a word-wise drag in an editor: the anchor word `anchor` stays selected
/// and the far end is the end (forwards) or start (backwards) of the word under `at`.
fn round_word_drag(rope: &Rope, anchor: (ByteIdx, ByteIdx), at: ByteIdx) -> (ByteIdx, ByteIdx) {
    if at < anchor.0 {
        (anchor.1, pos::word_range(rope, at).0)
    } else if at >= anchor.1 {
        (anchor.0, pos::word_range(rope, at).1.max(anchor.1))
    } else {
        anchor
    }
}

/// The (anchor, head) of a word-wise drag in a terminal: the anchor run stays selected and the
/// far end is the end (forwards) or start (backwards) of the run under the pointer (`under`),
/// or the pointer's cell when it is not on a path run.
fn round_run_drag(
    anchor: (select::Cell, select::Cell),
    head: select::Cell,
    under: Option<(select::Cell, select::Cell)>,
) -> (select::Cell, select::Cell) {
    if head < anchor.0 {
        (anchor.1, under.map_or(head, |r| r.0.min(head)))
    } else if head >= anchor.1 {
        (anchor.0, under.map_or(head, |r| r.1.max(head)))
    } else {
        anchor
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    SideBorder,
    /// Resizing the bottom panel by its top border.
    PanelBorder,
    TerminalBorder,
    /// Resizing a split.
    Divider(Divider),
    /// Extending a selection; the anchor range is kept for word/line drags.
    Text {
        anchor: (ByteIdx, ByteIdx),
        /// Set by a double-click: the pointer rounds out to whole words (`round_word_drag`).
        word: bool,
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
    /// Selecting text in a group's diff tab (`app/diff_view.rs`).
    Diff(crate::layout::GroupId),
    /// Selecting text on a terminal screen (`tasks::select`): the run, the anchor cell
    /// (scroll-independent) and the area the screen is drawn in.
    Terminal {
        run: RunId,
        /// The seeded path run of a double-click (`word`), else the pressed cell twice.
        anchor: (select::Cell, select::Cell),
        /// A drag from a double-click rounds its head out to the run under the pointer.
        word: bool,
        area: Rect,
    },
    /// Selecting text in one of the one-line inputs (`app/find.rs`, `app/workspace_search.rs`,
    /// `app/palette.rs`).
    Field(FieldTarget),
    /// Dragging a tab along a tab bar, or onto another group or editor (`app/tab_drag.rs`).
    Tab(TabDrag),
}

/// Which text field a `Drag::Field` extends the selection of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTarget {
    Find(Field),
    Search(Input),
    Palette,
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

/// The height of a bottom area whose top border is dragged to `row`, above `bottom`.
fn border_height(row: u16, bottom: u16, min: u16) -> u16 {
    bottom.saturating_sub(row).clamp(min, bottom.max(min))
}

impl App {
    pub(super) fn handle_mouse(&mut self, m: MouseEvent) {
        self.mouse_event(m);
        self.pointer = self.pointer_at(m.column, m.row);
    }

    /// The pointer's shape: a resize arrow over a border that drags (and while dragging one,
    /// wherever the pointer is), the I-beam over text you can type or select in (and while
    /// selecting text), else the terminal's own.
    pub(super) fn pointer_at(&self, x: u16, y: u16) -> Pointer {
        let p = Position { x, y };
        let divider = |d: &Divider| match d.dir {
            SplitDir::Right => Pointer::ColResize,
            SplitDir::Down => Pointer::RowResize,
        };
        match &self.drag {
            Some(Drag::SideBorder) => Pointer::ColResize,
            Some(Drag::PanelBorder | Drag::TerminalBorder | Drag::HistoryDivider) => Pointer::RowResize,
            Some(Drag::Divider(d)) => divider(d),
            // Selecting text keeps the I-beam wherever the pointer goes.
            Some(Drag::Text { .. } | Drag::AddedText { .. } | Drag::Column) => Pointer::Text,
            Some(_) => Pointer::Default,
            None if self.prompt.is_some() => {
                self.text_pointer_if(self.hits.prompt_input.is_some_and(|r| r.contains(p)))
            }
            None if self.palette.is_some() => {
                self.text_pointer_if(self.hits.palette.is_some_and(|h| h.input.contains(p)))
            }
            None => match self.hits.hit(x, y) {
                Hit::SideBorder => Pointer::ColResize,
                Hit::PanelBorder | Hit::TerminalBorder | Hit::HistoryDivider => Pointer::RowResize,
                Hit::Divider(i) => self.hits.dividers.get(i).map_or(Pointer::Default, divider),
                Hit::Editor(g) => self.text_pointer_if(self.editor_text_at(g, p)),
                Hit::SidePanel => {
                    let s = self.hits.search.as_ref();
                    self.text_pointer_if(
                        s.is_some_and(|s| s.inputs.iter().any(|r| r.contains(p)) || s.replace.contains(p)),
                    )
                }
                _ => Pointer::Default,
            },
        }
    }

    fn text_pointer_if(&self, over: bool) -> Pointer {
        if over { Pointer::Text } else { Pointer::Default }
    }

    /// Whether `p`, in group `g`'s editor, is over text you can type or select in: the find
    /// widget's inputs, a text tab's text area, a diff view's text panes (not its buttons).
    fn editor_text_at(&self, g: crate::layout::GroupId, p: Position) -> bool {
        if let Some(f) = &self.hits.find
            && f.area.contains(p)
        {
            return f.query.contains(p) || f.replace.is_some_and(|r| r.contains(p));
        }
        let (Some(group), Some(hit)) = (self.any_group(g), self.hits.group(g)) else {
            return false;
        };
        if group.active_tab().is_some() {
            return hit.text.contains(p) && !hit.merge_buttons.iter().any(|(r, _, _)| r.contains(p));
        }
        match (&hit.diff, group.active_diff()) {
            (Some(d), Some(_)) => {
                d.rows.contains(p)
                    && !d.buttons.iter().chain(&d.reverts).any(|(r, _)| r.contains(p))
                    && !d.moves.iter().any(|(r, _, _)| r.contains(p))
                    && hit.scrollbar.is_none_or(|s| !s.track.contains(p))
            }
            _ => false,
        }
    }

    /// The run a mouse event over a shell or task tab goes to, and the screen area its cells are
    /// in: `Hit::Editor(g)` with a shell tab active there, or `Hit::Panel` showing a task run.
    fn terminal_run(&self, hit: Hit) -> Option<(&Run, Rect)> {
        match hit {
            Hit::Editor(g) => Some((&self.any_group(g)?.active_shell()?.run, self.hits.group(g)?.editor)),
            Hit::Panel => match self.ui.panel_tab {
                PanelTab::Task(id) => Some((self.tasks.get(id)?, task_area(self.hits.regions.panel?))),
                _ => None,
            },
            _ => None,
        }
    }

    /// A mouse event over a shell or task tab whose program reports the mouse: to its terminal as
    /// xterm sends it (`tasks::mouse`), its cells counted from `area`; a left click also focuses
    /// what it lands on. Nothing while the program hasn't asked for the mouse or has ended, or
    /// the view is scrolled back (the wheel scrolls the scrollback then). True when the program
    /// has the event, so jed's click / wheel handling stays out of it.
    fn terminal_mouse(&mut self, hit: Hit, m: MouseEvent) -> bool {
        // A drag jed started elsewhere (text, a scroll bar, a tab, a border) keeps its moves
        // and release.
        if self.drag.is_some() {
            return false;
        }
        let Some((run, area)) = self.terminal_run(hit) else {
            return false;
        };
        if !run.status().is_running() {
            return false;
        }
        let bytes = {
            let screen = run.screen();
            let s = screen.screen();
            if s.mouse_protocol_mode() == MouseProtocolMode::None || s.scrollback() != 0 {
                return false;
            }
            // 0-based cells in the run's screen (`encode` makes them the protocol's 1-based
            // ones), the pointer clamped into the area.
            let cell = MouseEvent {
                column: m.column.saturating_sub(area.x).min(area.width.saturating_sub(1)),
                row: m.row.saturating_sub(area.y).min(area.height.saturating_sub(1)),
                ..m
            };
            mouse::encode(&cell, s.mouse_protocol_mode(), s.mouse_protocol_encoding())
        };
        if let Some(bytes) = &bytes {
            run.write(bytes);
        }
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            match hit {
                Hit::Editor(g) => self.focus_group(g),
                Hit::Panel => {
                    if let PanelTab::Task(id) = self.ui.panel_tab {
                        self.focus_panel(PanelTab::Task(id));
                    }
                }
                _ => {}
            }
        }
        true
    }

    /// A mouse event over a shell or task tab whose program hasn't asked for the mouse (so
    /// `terminal_mouse` let it through): a left press seeds a selection, a drag extends it and a
    /// click without a drag clears it. The cells are the screen's visible-viewport cells, the
    /// space `tasks::select` copies from. False for other events (the wheel scrolls the
    /// scrollback), and while a program reports the mouse it keeps every event.
    fn terminal_select(&mut self, hit: Hit, m: MouseEvent) -> bool {
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            // Counted only once this press is the selection's: a press a program's mouse
            // reporting keeps (scrolled back) falls through to `mouse_event`, which counts it.
            let reports = match self.terminal_run(hit) {
                None => return false,
                Some((run, _)) => run.screen().screen().mouse_protocol_mode() != MouseProtocolMode::None,
            };
            if reports {
                return false;
            }
            let count = self.clicks.click(Instant::now(), m.column, m.row);
            let Some((run, area)) = self.terminal_run(hit) else {
                return false;
            };
            let id = run.id;
            let (cell, seed) = {
                let screen = run.screen();
                let s = screen.screen();
                if s.mouse_protocol_mode() != MouseProtocolMode::None {
                    return false;
                }
                let (row, col) = screen_cell(s, area, m.column, m.row);
                let cell = select::cell_at(row, col, s.scrollback());
                // A double-click selects the path-like run under the pointer (none on a
                // non-path cell: the empty selection a click makes).
                let seed = if count == 2 {
                    select::path_run(s, (row, col))
                } else {
                    None
                };
                (cell, seed)
            };
            // A press is an empty selection until the pointer moves, so a click clears it.
            let (anchor, head) = seed.unwrap_or((cell, cell));
            run.set_selection(select::Selection { anchor, head });
            match hit {
                Hit::Editor(g) => self.focus_group(g),
                Hit::Panel => {
                    if let PanelTab::Task(id) = self.ui.panel_tab {
                        self.focus_panel(PanelTab::Task(id));
                    }
                }
                _ => {}
            }
            self.drag = Some(Drag::Terminal {
                run: id,
                anchor: (anchor, head),
                word: seed.is_some(),
                area,
            });
            return true;
        }
        if m.kind == MouseEventKind::Drag(MouseButton::Left)
            && let Some(Drag::Terminal {
                run,
                anchor,
                word,
                area,
            }) = self.drag.clone()
        {
            if let Some(run) = self.terminal_run_by_id(run) {
                let screen = run.screen();
                let s = screen.screen();
                let (row, col) = screen_cell(s, area, m.column, m.row);
                let head = select::cell_at(row, col, s.scrollback());
                let word_run = if word { select::path_run(s, (row, col)) } else { None };
                drop(screen);
                let (anchor, head) = if word {
                    round_run_drag(anchor, head, word_run)
                } else {
                    (anchor.0, head)
                };
                run.set_selection(select::Selection { anchor, head });
            }
            return true;
        }
        // A press released without a drag selected no cell: nothing stays to copy.
        if m.kind == MouseEventKind::Up(MouseButton::Left)
            && let Some(Drag::Terminal { run, .. }) = self.drag.clone()
            && let Some(run) = self.terminal_run_by_id(run)
            && run.selection().is_some_and(|s| s.anchor == s.head)
        {
            run.clear_selection();
        }
        false
    }

    /// The run `id` names, in the task tabs or in any group's shell tab.
    fn terminal_run_by_id(&self, id: RunId) -> Option<&Run> {
        self.tasks
            .get(id)
            .or_else(|| self.shell_tabs().find(|s| s.run.id == id).map(|s| &s.run))
    }

    fn mouse_event(&mut self, m: MouseEvent) {
        // The mouse's back / forward buttons navigate wherever the pointer is, and over a terminal
        // or a popup too, so they are handled before anything looks at the hit. A drag while one
        // is held and its release are consumed here too: the button is jed's, so a terminal
        // program must never see it.
        match m.kind {
            MouseEventKind::Down(MouseButton::Back) => {
                self.execute(Command::NavBack);
                return;
            }
            MouseEventKind::Down(MouseButton::Forward) => {
                self.execute(Command::NavForward);
                return;
            }
            MouseEventKind::Drag(MouseButton::Back)
            | MouseEventKind::Drag(MouseButton::Forward)
            | MouseEventKind::Up(MouseButton::Back)
            | MouseEventKind::Up(MouseButton::Forward) => return,
            _ => {}
        }
        let hit = match self.hits.hit(m.column, m.row) {
            // The wheel over a scroll bar scrolls what it scrolls, as the wheel over that.
            Hit::ScrollBar(t)
                if matches!(
                    m.kind,
                    MouseEventKind::ScrollUp
                        | MouseEventKind::ScrollDown
                        | MouseEventKind::ScrollLeft
                        | MouseEventKind::ScrollRight
                ) =>
            {
                match t {
                    ScrollTarget::Group(g) | ScrollTarget::Columns(g) => Hit::Editor(g),
                    ScrollTarget::Side(_) => Hit::SidePanel,
                    ScrollTarget::Panel(_) => Hit::Panel,
                    // The wheel over the history's bar scrolls its list, as over its rows.
                    ScrollTarget::History => Hit::HistoryRow(0),
                    // Taken by `cheat_sheet_mouse` while it's open.
                    ScrollTarget::CheatSheet => Hit::Nothing,
                    // Taken by `keyboard_shortcuts_mouse` while it's open.
                    ScrollTarget::Keys => Hit::Nothing,
                }
            }
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
        if self.terminal_mouse(hit, m) {
            return;
        }
        if self.terminal_select(hit, m) {
            return;
        }
        // Sideways: the text tab under the mouse, without focusing it or moving its cursor.
        if let Some(delta) = wheel_columns(&m)
            && let Hit::Editor(g) = hit
        {
            if let Some(tab) = self.any_group_mut(g).and_then(|g| g.active_tab_mut()) {
                tab.view.scroll_columns(delta);
            }
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
                        if let Some(group) = self.any_group_mut(g) {
                            group.scroll_tabs(delta.signum());
                        }
                    }
                    Hit::Panel | Hit::PanelTab(_) | Hit::PanelTabClose(_) | Hit::PanelBorder => match self.ui.panel_tab
                    {
                        PanelTab::Problems => self.problems_scroll(delta),
                        PanelTab::Locations => self.locations_scroll(delta),
                        PanelTab::Task(_) => self.task_scroll(delta),
                    },
                    // Scrolls the group under the mouse, without focusing it (a shell tab
                    // scrolls its scrollback).
                    Hit::Editor(g) => {
                        if let Some(shell) = self.any_group(g).and_then(|g| g.active_shell()) {
                            shell.run.scroll(delta);
                            return;
                        }
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
                // A press while a drag is already armed means the previous drag's release never
                // arrived (the button went up outside the terminal, or a popup swallowed it): that
                // drag is stale, and the marker / shade it left must go. `left_click` re-arms a
                // fresh drag where the press starts one.
                self.drag = None;
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
                // A border's top row follows the pointer; `layout::compute` keeps both minimums.
                Some(Drag::PanelBorder) => {
                    let regions = &self.hits.regions;
                    let bottom = regions.terminal.map_or(regions.status.y, |t| t.y);
                    self.ui.panel_height = border_height(m.row, bottom, MIN_PANEL_HEIGHT);
                }
                Some(Drag::TerminalBorder) => {
                    self.ui.terminal_height = border_height(m.row, self.hits.regions.status.y, MIN_TERMINAL_HEIGHT);
                }
                Some(Drag::Divider(d)) => {
                    let groups = if d.terminal {
                        &mut self.terminals
                    } else {
                        &mut self.groups
                    };
                    groups.tree.set_ratio(&d, d.ratio_at(m.column, m.row));
                }
                Some(Drag::Text { anchor, word }) => {
                    let at = self.text_position(m.column, m.row, true);
                    if let (Some(at), Some((buf, view))) = (at, self.active_mut()) {
                        // Dragging back past the anchor keeps the whole anchor word/line selected;
                        // after a double-click the pointer rounds out to whole words.
                        let (a, h) = if word {
                            round_word_drag(&buf.rope, anchor, at)
                        } else if at < anchor.0 {
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
                Some(Drag::Field(target)) => {
                    if let Some(at) = self.field_position(target, m.column)
                        && let Some(field) = self.field_mut(target)
                    {
                        field.drag_to(at);
                    }
                }
                Some(Drag::ScrollBar { target, grab }) => self.scrollbar_drag(target, m.column, m.row, grab),
                Some(Drag::Tab(_)) => self.tab_drag_to(m.column, m.row),
                // Taken by `hex_mouse`.
                Some(Drag::Hex(_) | Drag::Diff(_)) => {}
                // Taken by `terminal_select`.
                Some(Drag::Terminal { .. }) => {}
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
                if matches!(self.drag, Some(Drag::Tab(_))) {
                    self.finish_tab_drag(m.column, m.row);
                }
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
            Hit::Icon(panel) if !self.mode.allows_view(panel) => {}
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
                // A drag that never leaves the tab is just a click: it stays active.
                self.drag = Some(Drag::Tab(TabDrag {
                    from: g,
                    index: i,
                    tab: self.any_group(g).and_then(|group| group.tabs.get(i)).map(Tab::identity),
                    target: None,
                }));
            }
            Hit::TabClose(g, i) => self.close_tab(g, i),
            Hit::TabBar(g) => self.focus_group(g),
            Hit::TabArrow(g, forward) => {
                self.focus_group(g);
                if let Some(group) = self.any_group_mut(g) {
                    group.scroll_tabs(if forward { 1 } else { -1 });
                }
            }
            Hit::Banner(g, action) => {
                self.focus_group(g);
                if let Some(a) = action {
                    self.execute(a.command());
                }
            }
            Hit::Editor(g) if is_terminal_group(g) => self.focus_group(g),
            Hit::Editor(_) if m.modifiers.is_empty() && self.bulb_click(&m) => {}
            Hit::Editor(g) if m.modifiers.is_empty() && self.merge_click(g, &m) => {}
            Hit::Editor(_) if m.modifiers.is_empty() && self.gutter_click(&m) => {}
            Hit::Editor(g) => {
                self.focus_group(g);
                self.editor_click(m, count);
            }
            // Scrolls without focusing anything or moving a cursor or a list's selection.
            Hit::ScrollBar(t) => self.scrollbar_click(t, m.column, m.row),
            Hit::PanelBorder => self.drag = Some(Drag::PanelBorder),
            Hit::TerminalBorder => self.drag = Some(Drag::TerminalBorder),
            Hit::PanelTab(tab) => self.focus_panel(tab),
            Hit::PanelTabClose(id) => self.close_task(id),
            Hit::Panel => match self.ui.panel_tab {
                PanelTab::Problems => self.problems_click(m, count),
                PanelTab::Locations => self.locations_click(m, count),
                PanelTab::Task(_) => self.task_click(m),
            },
            Hit::StatusProblems if self.mode.allows(Command::ToggleProblems) => self.problem_counts_clicked(),
            Hit::StatusProblems => {}
            Hit::StatusIndentation => self.indentation_clicked(),
            Hit::StatusLanguage => self.change_language_mode(),
            Hit::Status | Hit::Hover | Hit::Completion | Hit::CodeActions | Hit::QuickDiff | Hit::Nothing => {}
        }
    }

    /// A click on a merge conflict's action row: Accept Current / Incoming / Both applies that
    /// side of the button's conflict (one undo step); Compare opens the conflict's current and
    /// incoming blocks in a diff tab. True when the click was on a button.
    fn merge_click(&mut self, group: crate::layout::GroupId, m: &MouseEvent) -> bool {
        let at = ratatui::layout::Position { x: m.column, y: m.row };
        let Some(&(_, button, start)) = self
            .hits
            .group(group)
            .and_then(|g| g.merge_buttons.iter().find(|(rect, _, _)| rect.contains(at)))
        else {
            return false;
        };
        self.focus_group(group);
        self.ui.focus = Focus::Editor;
        match button {
            MergeButton::Accept(choice) => {
                if !self.active_writable() {
                    return true;
                }
                let Some((buf, view)) = self.active_mut() else {
                    return true;
                };
                let revision = buf.revision();
                let conflict = buf
                    .conflicts
                    .scan(&buf.rope, revision)
                    .iter()
                    .find(|c| c.start == start)
                    .cloned();
                if let Some(conflict) = conflict {
                    crate::edit::accept_merge(buf, view, choice, std::slice::from_ref(&conflict));
                }
            }
            MergeButton::Compare => self.compare_conflict(Some(start)),
        }
        true
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
        self.drag = Some(Drag::Text {
            anchor: (start, end),
            word: count == 2,
        });
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
    /// the line's text (line-relative, as `column::select` takes). With `clamp`, as
    /// `text_position`.
    fn text_cell(&mut self, x: u16, y: u16, clamp: bool) -> Option<Cell> {
        let (line, _, col) = self.text_hit(x, y, clamp)?;
        Some((line, col))
    }

    /// The text field a `FieldTarget` names, when it is showing.
    fn field_ref(&self, target: FieldTarget) -> Option<&TextField> {
        match target {
            FieldTarget::Find(Field::Find) => Some(&self.find.query),
            FieldTarget::Find(Field::Replace) => Some(&self.find.replace),
            FieldTarget::Search(Input::Query) => Some(&self.search_view.query),
            FieldTarget::Search(Input::Include) => Some(&self.search_view.include),
            FieldTarget::Search(Input::Exclude) => Some(&self.search_view.exclude),
            FieldTarget::Search(Input::Replace) => Some(&self.search_view.replace),
            FieldTarget::Palette => Some(&self.palette.as_ref()?.field),
        }
    }

    /// The same field, to change.
    pub(super) fn field_mut(&mut self, target: FieldTarget) -> Option<&mut TextField> {
        match target {
            FieldTarget::Find(Field::Find) => Some(&mut self.find.query),
            FieldTarget::Find(Field::Replace) => Some(&mut self.find.replace),
            FieldTarget::Search(Input::Query) => Some(&mut self.search_view.query),
            FieldTarget::Search(Input::Include) => Some(&mut self.search_view.include),
            FieldTarget::Search(Input::Exclude) => Some(&mut self.search_view.exclude),
            FieldTarget::Search(Input::Replace) => Some(&mut self.search_view.replace),
            FieldTarget::Palette => Some(&mut self.palette.as_mut()?.field),
        }
    }

    /// A field's drawn rect and sideways scroll (the previous frame's `HitMap`).
    fn field_geometry(&self, target: FieldTarget) -> Option<(Rect, usize)> {
        match target {
            FieldTarget::Find(Field::Find) => {
                let hit = self.hits.find.as_ref()?;
                Some((hit.query, hit.query_scroll))
            }
            FieldTarget::Find(Field::Replace) => {
                let hit = self.hits.find.as_ref()?;
                Some((hit.replace?, hit.replace_scroll))
            }
            FieldTarget::Search(Input::Replace) => {
                let hit = self.hits.search.as_ref()?;
                Some((hit.replace, hit.replace_scroll))
            }
            FieldTarget::Search(input) => {
                let hit = self.hits.search.as_ref()?;
                let i = match input {
                    Input::Query => 0,
                    Input::Include => 1,
                    Input::Exclude => 2,
                    // Handled above (its rect is `replace`, not in `inputs`).
                    Input::Replace => return None,
                };
                Some((hit.inputs[i], hit.input_scrolls[i]))
            }
            FieldTarget::Palette => {
                let hit = self.hits.palette.as_ref()?;
                Some((hit.input, hit.input_scroll))
            }
        }
    }

    /// The byte offset under screen column `x` in a field: mapped through its draw-time scroll;
    /// a pointer past either end (a drag) maps past the visible text, so the field scrolls.
    pub(super) fn field_position(&self, target: FieldTarget, x: u16) -> Option<usize> {
        let (rect, scroll) = self.field_geometry(target)?;
        let field = self.field_ref(target)?;
        (rect.width > 0).then(|| field_at(field, rect, scroll, x))
    }

    /// Screen cell to buffer position in the active group. With `clamp`, points outside
    /// the text area (while dragging) are pulled to its edge.
    fn text_position(&mut self, x: u16, y: u16, clamp: bool) -> Option<ByteIdx> {
        Some(self.text_hit(x, y, clamp)?.1)
    }

    /// Screen cell to (line, byte, line-relative display column) in the active group; the
    /// column may lie past the line's text.
    fn text_hit(&mut self, x: u16, y: u16, clamp: bool) -> Option<(usize, ByteIdx, DisplayCol)> {
        let a = self.hits.group(self.groups.active)?.text;
        let settings = self.settings.clone();
        let (buf, view) = self.active_mut()?;
        let tab_width = settings.for_buffer(buf).tab_width;
        hit_at(&buf.rope, view, a, x, y, clamp, tab_width)
    }
}

/// A screen cell in a view's text area `text` to (line, byte, line-relative display column).
/// While wrapping, a screen row is a row of some line, so the byte comes from that row's break
/// points (`wrap::Layout`), not from the line's whole unwrapped width; a cell in a continuation
/// row's indentation lands at the row's first byte. With `clamp`, points outside `text` (while
/// dragging) are pulled to its edge.
fn hit_at(
    rope: &Rope,
    view: &mut View,
    text: Rect,
    x: u16,
    y: u16,
    clamp: bool,
    tab_width: usize,
) -> Option<(usize, ByteIdx, DisplayCol)> {
    if text.width == 0 || text.height == 0 {
        return None;
    }
    let (x, y) = if clamp {
        (
            x.clamp(text.x, text.x + text.width - 1),
            y.clamp(text.y, text.y + text.height - 1),
        )
    } else {
        (x.max(text.x), y)
    };
    if y < text.y || y >= text.y + text.height {
        return None;
    }
    let (dx, dy) = ((x - text.x) as usize, (y - text.y) as usize);
    if !view.wrapping || view.width == 0 {
        let line = (view.top_line() + dy).min(pos::line_count(rope) - 1);
        let col = DisplayCol(view.left_col + dx);
        let at = pos::byte_at_col(rope, line, col, tab_width);
        return Some((line, at, col));
    }
    let (revision, width) = (view.revision, view.width);
    let (top, top_row) = (view.top_line(), view.top_row());
    let (line, sub) = view.rows_down(rope, top, top_row, dy);
    let layout = view.wrap.layout(rope, revision, line, width, tab_width);
    let sub = sub.min(layout.len() - 1);
    let within = DisplayCol(dx.saturating_sub(layout.rows()[sub].col.0));
    let at = ByteIdx(pos::line_start(rope, line).0 + layout.byte_at(sub, within).0);
    // Past the row's last grapheme: keep the cell's distance from the text, so a box's corner
    // can lie beyond the line's end (the unwrapped mapping's column).
    let extra = within.0.saturating_sub(layout.row_width(sub).0);
    let col = DisplayCol(pos::display_col(rope, at, tab_width).0 + extra);
    Some((line, at, col))
}

/// The byte offset under screen column `x` of a field drawn in `rect` scrolled by `scroll`
/// columns: `x` relative to the input's left edge, through its scroll. A pointer past either end
/// (a drag) maps past the visible text, so the field scrolls to follow the selection; the row is
/// ignored.
pub(super) fn field_at(field: &TextField, rect: Rect, scroll: usize, x: u16) -> usize {
    field.offset_at_col(x as i64 - rect.x as i64, scroll)
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

    /// Without wrapping a cell is its line and display column, as before.
    #[test]
    fn cells_map_to_a_line_and_column_without_wrapping() {
        let rope = Rope::from_str("  aaaa bbbb cccc\ndddd\n");
        let mut view = View::new();
        let text = Rect::new(3, 1, 10, 6);
        let hit = hit_at(&rope, &mut view, text, text.x + 3, text.y + 1, false, 4);
        assert_eq!(hit, Some((1, ByteIdx(20), DisplayCol(3))));
        assert_eq!(hit_at(&rope, &mut view, text, text.x, text.y + 99, false, 4), None);
    }

    /// With wrapping a cell maps through the row drawn there: its text's bytes on the row's
    /// first row, its indentation and the row's first byte on a continuation row, and past its
    /// text the line column keeps the cell's distance (a box's corner past the line's end).
    #[test]
    fn cells_map_through_the_wrapped_rows() {
        let rope = Rope::from_str("  aaaa bbbb cccc\ndddd\n");
        let mut view = View::new();
        (view.width, view.height) = (6, 6);
        view.wrapping = true;
        let text = Rect::new(3, 1, 10, 6);
        // Line 0 is three rows at width 6; each continuation row starts at its two-space
        // indentation and its text follows.
        let hit = |view: &mut View, x: u16, y: u16| hit_at(&rope, view, text, x, y, false, 4);
        assert_eq!(
            hit(&mut view, text.x + 4, text.y + 1),
            Some((0, ByteIdx(9), DisplayCol(9)))
        );
        assert_eq!(
            hit(&mut view, text.x + 1, text.y + 1),
            Some((0, ByteIdx(7), DisplayCol(7))),
            "in the continuation row's indentation: its first byte"
        );
        assert_eq!(
            hit(&mut view, text.x + 2, text.y + 3),
            Some((1, ByteIdx(19), DisplayCol(2))),
            "the next line's first row"
        );
        // Scrolled into the line's second row: the first screen row is that row.
        view.set_top(&rope, 0, 1);
        assert_eq!(hit(&mut view, text.x + 3, text.y), Some((0, ByteIdx(8), DisplayCol(8))));
    }

    /// A cell past a wrapped row's text keeps its distance from the text in the line column,
    /// as the unwrapped mapping does, so a box's corner can lie beyond the line's end.
    #[test]
    fn cells_past_a_wrapped_rows_text_keep_the_line_column() {
        let rope = Rope::from_str("ab cd ef\n");
        let mut view = View::new();
        (view.width, view.height) = (3, 4);
        view.wrapping = true;
        let text = Rect::new(3, 1, 8, 4);
        // Rows "ab " / "cd " / "ef": five columns into the second row is past "cd ".
        assert_eq!(
            hit_at(&rope, &mut view, text, text.x + 5, text.y + 1, false, 4),
            Some((0, ByteIdx(6), DisplayCol(8)))
        );
        // The last row: past "ef", the line's end plus the extra columns.
        assert_eq!(
            hit_at(&rope, &mut view, text, text.x + 4, text.y + 2, false, 4),
            Some((0, ByteIdx(8), DisplayCol(10)))
        );
    }

    #[test]
    fn word_drag_rounds_to_whole_words() {
        let rope = Rope::from_str("aaa bbb ccc\n");
        let b = ByteIdx;
        let anchor = (b(4), b(7));
        assert_eq!(round_word_drag(&rope, anchor, b(9)), (b(4), b(11)));
        assert_eq!(round_word_drag(&rope, anchor, b(1)), (b(7), b(0)));
        assert_eq!(round_word_drag(&rope, anchor, b(5)), anchor);
        assert_eq!(round_word_drag(&rope, anchor, b(4)), anchor, "the anchor word's start");
        assert_eq!(round_word_drag(&rope, anchor, b(3)), (b(7), b(3)), "the space before");
        assert_eq!(round_word_drag(&rope, anchor, b(8)), (b(4), b(11)), "a word's start");
    }

    #[test]
    fn run_drag_rounds_to_the_run_under_the_pointer() {
        let anchor = ((1, 4), (1, 10));
        assert_eq!(
            round_run_drag(anchor, (1, 13), Some(((1, 12), (1, 20)))),
            ((1, 4), (1, 20))
        );
        assert_eq!(
            round_run_drag(anchor, (1, 1), Some(((1, 0), (1, 3)))),
            ((1, 10), (1, 0))
        );
        assert_eq!(round_run_drag(anchor, (1, 15), None), ((1, 4), (1, 15)));
        assert_eq!(round_run_drag(anchor, (1, 2), None), ((1, 10), (1, 2)));
        assert_eq!(
            round_run_drag(anchor, (0, 30), Some(((0, 25), (0, 40)))),
            ((1, 10), (0, 25)),
            "a run on an earlier row"
        );
        assert_eq!(round_run_drag(anchor, (1, 6), None), anchor);
    }
}
