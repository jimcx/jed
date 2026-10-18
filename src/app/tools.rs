//! Tool logs (`crate::logs`): where the store writes, its write errors, the flush on quit, the
//! read-only log tabs ("Tools: Open Log" / "Clear Log"), and the Tools view (`app.tools_view`,
//! a `tools_view::ToolsView`; drawn by `ui::tools`): its keys (`Context::Tools`), opening a
//! row's log or focusing a task run's panel tab, the mouse, and keeping it current.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::MouseEvent;
use ratatui::layout::Position;

use super::{App, AppEvent, Focus, Level};
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::{Buffer, BufferId};
use crate::command::ToolsCommand as C;
use crate::layout::{PanelTab, SidePanel};
use crate::logs::Tool;
use crate::palette::Picker;
use crate::selection::{Selection, Selections};
use crate::tools_view::{self, Row};
use crate::view::View;

/// How long quitting waits for the tool logs' last lines to reach the disk.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(500);
/// After a wake, log tabs wait this long before the store may wake the loop again: new lines
/// cost at most one extra loop turn per frame, however fast they come (and a turn that itself
/// logs can't spin the loop).
pub const LOG_FRAME: Duration = Duration::from_millis(20);

/// Log tabs' state beyond their buffers (`Buffer::log`).
#[derive(Default)]
pub struct LogTabs {
    /// When the store's waker is rearmed (`LOG_FRAME` after a wake).
    deadline: Option<Instant>,
    /// Change sets applied to log buffers (tests: one per log per turn).
    #[cfg(test)]
    pub appends: u64,
}

impl LogTabs {
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

impl App {
    /// From `start`: language servers log into `self.logs`, which writes to
    /// `<state dir>/logs/tools` (nothing without a `state_dir`: tests unless set), and wakes the
    /// loop for log tabs (`AppEvent::LogsChanged`).
    pub(super) fn start_tool_logs(&mut self) {
        self.lsp.logs = self.logs.clone();
        if let Some(state) = &self.state_dir {
            self.logs
                .set_disk(state.join("logs").join("tools"), self.workspace.root.clone());
        }
        let tx = self.tx.clone();
        self.logs.set_waker(Arc::new(move || {
            let _ = tx.send(AppEvent::LogsChanged);
        }));
    }

    /// Once per loop turn: the tool logs' first disk write error, if one just happened (once:
    /// logging carries on in memory).
    pub(super) fn report_log_errors(&mut self) {
        if let Some(e) = self.logs.take_disk_error() {
            self.notify(Level::Warning, e);
        }
    }

    /// On quit: waits (bounded) for the queued tool log lines to be written.
    pub(super) async fn flush_tool_logs(&self) {
        let logs = self.logs.clone();
        let _ = tokio::task::spawn_blocking(move || logs.flush(FLUSH_TIMEOUT)).await;
    }

    fn has_log_tabs(&self) -> bool {
        self.buffers.iter().any(|(_, b)| b.log.is_some())
    }

    /// Something shows the logs: a log tab (its lines), or the Tools view (its error badges).
    fn wants_log_wakes(&self) -> bool {
        self.has_log_tabs() || self.tools_shown()
    }

    /// `AppEvent::LogsChanged`: the turn it ends reads the new lines (`sync_logs`); the store
    /// may wake the loop again `LOG_FRAME` later. With neither log tabs nor the Tools view it
    /// stays unarmed: nothing wakes the loop until one shows. Returns whether the Tools view
    /// needs a redraw (an error badge changed); log tabs redraw from `sync_logs`.
    pub(super) fn logs_changed(&mut self) -> bool {
        if self.wants_log_wakes() {
            self.log_tabs.deadline.get_or_insert(Instant::now() + LOG_FRAME);
        }
        self.tools_shown() && self.logs.error_counts() != self.tools_view.errors_drawn
    }

    /// `Timer::Logs`: the store may wake the loop again.
    pub(super) fn logs_tick(&mut self) {
        self.log_tabs.deadline = None;
        if self.wants_log_wakes() {
            self.logs.rearm();
        }
    }

    /// Once per loop turn (and `pump` / `settle`): each log buffer takes in its tool's lines
    /// since its last read as one change set (`LogTab::update`: new lines appended, ones the ring
    /// dropped deleted from the start, all replaced after a clear), outside the undo history.
    /// A view whose primary cursor was on the last line follows the tail (cursor at the end,
    /// revealed); any other maps through the change and stays on its text.
    pub(super) fn sync_logs(&mut self) {
        // The Tools view just appeared: the store may wake the loop (its badges stay current).
        let shown = self.tools_shown();
        if shown && !self.tools_view.was_shown && self.log_tabs.deadline.is_none() {
            self.logs.rearm();
        }
        self.tools_view.was_shown = shown;
        let ids: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.log.is_some())
            .map(|(id, _)| id)
            .collect();
        let mut changed = false;
        for id in ids {
            let Some(buf) = self.buffers.get_mut(id) else { continue };
            let Some(log) = buf.log.as_mut() else { continue };
            let read = self.logs.read(&log.tool, log.cursor());
            let changes = log.update(&buf.rope, read);
            if changes.is_empty() {
                continue;
            }
            let old = buf.rope.clone();
            let last = pos::line_count(&old) - 1;
            buf.append(changes);
            #[cfg(test)]
            {
                self.log_tabs.appends += 1;
            }
            let applied = buf.take_applied();
            let end = ByteIdx(buf.rope.len());
            for (_, group) in self.groups.iter_mut() {
                for (_, tab) in group.text_tabs_mut() {
                    if tab.buffer != id {
                        continue;
                    }
                    let view = &mut tab.view;
                    let follow = pos::line_of(&old, view.selections.primary().head) == last;
                    view.map_through(&applied, &buf.rope);
                    if follow {
                        view.selections = Selections::single(Selection::cursor(end));
                        view.cursor_history.reset(&view.selections);
                        view.reveal_cursor = true;
                    }
                }
            }
            changed = true;
        }
        if changed {
            self.needs_redraw = true;
            self.refresh_find();
        }
    }

    /// The tools with a log, "jed" always among them (first).
    pub(super) fn log_tools(&self) -> Vec<Tool> {
        let mut tools = self.logs.tools();
        if !tools.contains(&Tool::Jed) {
            tools.insert(0, Tool::Jed);
        }
        tools
    }

    /// "Tools: Open Log" (`tools.openLog`): the palette's `Picker::Logs`.
    pub(super) fn show_log_picker(&mut self) {
        self.open_palette(Picker::Logs, "");
    }

    /// Shows `tool`'s log tab in the active group: its tab there if it has one, else a new tab
    /// on its buffer (another group's, or a new one), the cursor at the end (following the tail).
    pub(super) fn open_tool_log(&mut self, tool: Tool) {
        let existing = self
            .buffers
            .iter()
            .find(|(_, b)| b.log.as_ref().is_some_and(|l| l.tool == tool))
            .map(|(id, _)| id);
        if let Some(id) = existing
            && let Some(index) = self.group().find(id)
        {
            self.activate_tab(index);
        } else {
            if let Some((buf, _)) = self.active_mut() {
                buf.seal_undo();
            }
            let id = existing.unwrap_or_else(|| self.buffers.insert(Buffer::log(tool)));
            let mut view = View::new();
            if let Some(buf) = self.buffers.get(id) {
                view.selections = Selections::single(Selection::cursor(ByteIdx(buf.rope.len())));
                view.cursor_history.reset(&view.selections);
            }
            self.group_mut().open(id, view);
        }
        self.ui.focus = Focus::Editor;
        self.logs.rearm();
        self.sync_logs();
    }

    /// "Tools: Clear Log" (`tools.clearLog`): empties the active log tab's tool's log (its tab
    /// too; the file on disk is kept). Elsewhere it says how to get one.
    pub(super) fn clear_tool_log(&mut self) {
        let Some(tool) = self
            .active_buffer()
            .and_then(|b| b.log.as_ref())
            .map(|l| l.tool.clone())
        else {
            self.notify(
                Level::Info,
                "Tools: Clear Log clears the active log tab (Tools: Open Log opens one)".into(),
            );
            return;
        };
        self.logs.clear(&tool);
        self.sync_logs();
    }

    /// The server "Tools: Toggle Protocol Trace" / "Tools: Restart Server" act on: the Tools
    /// view's selected server's name while the view is shown, else the active file's language
    /// server. `Err` says why there is none.
    fn tools_server(&self) -> Result<String, String> {
        if self.tools_shown() {
            let rows = self.tools_rows();
            return match self.tools_view.selected_row(&rows).map(|i| &rows[i]) {
                Some(Row::Server(s)) => Ok(s.name.clone()),
                Some(_) => Err("select a language server's row in the Tools view".into()),
                None => Err("the Tools view lists no language servers".into()),
            };
        }
        let Some(buf) = self.active_file_buffer().and_then(|id| self.buffers.get(id)) else {
            return Err("open a file, or show the Tools view and select a server".into());
        };
        let language = buf.syntax.language();
        match self.lsp.server_for(language) {
            Some(s) => Ok(s.config.name.clone()),
            None => Err(format!("no language server is running for {}", language.name())),
        }
    }

    /// "Tools: Toggle Protocol Trace" (`tools.toggleTrace`): every JSON-RPC message to and from
    /// that server also goes to its log, or stops doing so. A failure / stopped server has no
    /// task to trace (Restart Server starts it again).
    pub(super) fn toggle_tool_trace(&mut self) {
        let name = match self.tools_server() {
            Ok(name) => name,
            Err(why) => {
                self.notify(Level::Info, format!("Tools: Toggle Protocol Trace: {why}"));
                return;
            }
        };
        let state = self
            .lsp
            .server_named(&name)
            .map(|s| (!matches!(s.state, crate::lsp::State::Failed), s.trace()));
        let Some((running, on)) = state else {
            self.notify(
                Level::Info,
                format!("{name}: not running (Tools: Restart Server starts it again)"),
            );
            return;
        };
        if !running {
            self.notify(
                Level::Info,
                format!("{name}: not running (Tools: Restart Server starts it again)"),
            );
            return;
        }
        if let Some(server) = self.lsp.server_named(&name) {
            server.set_trace(!on);
        }
        self.notify(
            Level::Info,
            format!("{name}: protocol trace {}", if on { "off" } else { "on" }),
        );
    }

    /// "Tools: Restart Server" (`tools.restartServer`): stops the server and starts a new one
    /// with the same config (also a failed one, which `sync` alone would never retry, and a
    /// stopped one); its log keeps its lines.
    pub(super) fn restart_server(&mut self) {
        let name = match self.tools_server() {
            Ok(name) => name,
            Err(why) => {
                self.notify(Level::Info, format!("Tools: Restart Server: {why}"));
                return;
            }
        };
        let Some(restarted) = self.lsp.restart(&name) else {
            self.notify(Level::Warning, format!("No language server named '{name}'"));
            return;
        };
        if let Some(old) = restarted.old {
            // The new instance answers none of the old one's requests.
            self.requests.retain(|(server, _), _| *server != old);
        }
        self.drop_cleared_diagnostics();
        self.lsp.sync_documents(&mut self.buffers);
        self.needs_redraw = true;
        self.notify(Level::Info, format!("Restarted {name}"));
    }
}

impl App {
    /// The Tools view is on screen.
    pub(super) fn tools_shown(&self) -> bool {
        self.ui.side_visible && self.ui.side_panel == SidePanel::Tools
    }

    /// The Tools view's rows, from the servers, task runs and logs as they are now.
    pub fn tools_rows(&self) -> Vec<Row> {
        tools_view::rows(
            &self.lsp,
            self.tasks.runs(),
            &self.logs.error_counts(),
            &self.workspace.root,
        )
    }

    /// With nothing selected, selects the first row (from `focus_side`: "Tools: Show", the
    /// activity bar icon).
    pub(super) fn tools_select_first(&mut self) {
        let rows = self.tools_rows();
        if self.tools_view.selected_row(&rows).is_none() {
            self.tools_view.select(&rows, 0);
        }
    }

    pub(super) fn tools_command(&mut self, cmd: C) {
        let rows = self.tools_rows();
        let page = self.tools_view.height.max(1) as isize;
        let view = &mut self.tools_view;
        match cmd {
            C::Up => view.move_selection(&rows, -1),
            C::Down => view.move_selection(&rows, 1),
            C::PageUp => view.move_selection(&rows, -page),
            C::PageDown => view.move_selection(&rows, page),
            C::First => view.select(&rows, 0),
            C::Last => view.select(&rows, rows.len().saturating_sub(1)),
            C::Open => self.tools_open(&rows),
            C::FocusEditor => {
                if !self.group().tabs.is_empty() {
                    self.ui.focus = Focus::Editor;
                }
            }
        }
    }

    /// The selected row: a tool's log tab, shown and focused (`open_tool_log`), or a task run's
    /// panel tab, shown and focused (its output is there: no second copy).
    fn tools_open(&mut self, rows: &[Row]) {
        let Some(i) = self.tools_view.selected_row(rows) else {
            return;
        };
        match &rows[i] {
            Row::Task { id, .. } => self.focus_panel(PanelTab::Task(*id)),
            row => {
                if let Some(tool) = row.tool() {
                    self.open_tool_log(tool);
                }
            }
        }
    }

    /// A click in the view: the row under it is selected and opened.
    pub(super) fn tools_click(&mut self, m: MouseEvent) {
        self.ui.focus = Focus::SidePanel;
        let Some(hit) = self.hits.tools.clone() else { return };
        let at = Position { x: m.column, y: m.row };
        if !hit.list.contains(at) {
            return;
        }
        let Some(key) = hit.rows.get((at.y - hit.list.y) as usize) else {
            return;
        };
        let rows = self.tools_rows();
        if self.tools_view.select_key(&rows, key) {
            self.tools_open(&rows);
        }
    }

    /// The wheel over the view.
    pub(super) fn tools_scroll(&mut self, delta: isize) {
        let rows = self.tools_rows();
        self.tools_view.scroll_by(&rows, delta);
    }
}
