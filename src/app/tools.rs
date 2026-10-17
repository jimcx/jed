//! Tool logs (`crate::logs`): where the store writes, its write errors, the flush on quit.

use std::time::Duration;

use super::{App, Level};

/// How long quitting waits for the tool logs' last lines to reach the disk.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(500);

impl App {
    /// From `start`: language servers log into `self.logs`, which writes to
    /// `<state dir>/logs/tools` (nothing without a `state_dir`: tests unless set).
    pub(super) fn start_tool_logs(&mut self) {
        self.lsp.logs = self.logs.clone();
        if let Some(state) = &self.state_dir {
            self.logs
                .set_disk(state.join("logs").join("tools"), self.workspace.root.clone());
        }
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
}
