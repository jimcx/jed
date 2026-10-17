//! Tool logs (`crate::logs`): where the store writes, its write errors, the flush on quit, and
//! the read-only log tabs ("Tools: Open Log" / "Clear Log").

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{App, AppEvent, Focus, Level};
use crate::buffer::pos::{self, ByteIdx};
use crate::buffer::{Buffer, BufferId};
use crate::logs::Tool;
use crate::palette::Picker;
use crate::selection::{Selection, Selections};
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

    /// `AppEvent::LogsChanged`: the turn it ends reads the new lines (`sync_logs`); the store
    /// may wake the loop again `LOG_FRAME` later. Without log tabs it stays unarmed: nothing
    /// wakes the loop until one opens.
    pub(super) fn logs_changed(&mut self) {
        if self.has_log_tabs() {
            self.log_tabs.deadline.get_or_insert(Instant::now() + LOG_FRAME);
        }
    }

    /// `Timer::Logs`: the store may wake the loop again.
    pub(super) fn logs_tick(&mut self) {
        self.log_tabs.deadline = None;
        if self.has_log_tabs() {
            self.logs.rearm();
        }
    }

    /// Once per loop turn (and `pump` / `settle`): each log buffer takes in its tool's lines
    /// since its last read as one change set (`LogTab::update`: new lines appended, ones the ring
    /// dropped deleted from the start, all replaced after a clear), outside the undo history.
    /// A view whose primary cursor was on the last line follows the tail (cursor at the end,
    /// revealed); any other maps through the change and stays on its text.
    pub(super) fn sync_logs(&mut self) {
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
}
