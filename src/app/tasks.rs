//! Tasks: templates (`.workspace` only while trusted) and runs (`app.tasks`, a `tasks::Tasks`).
//! No drawing yet: 12b adds the picker and the panel tabs.

// Until 12b's "Run Task" picker calls them.
#![cfg_attr(not(test), allow(dead_code))]

use super::{App, Level};
use crate::tasks::{Event, RunId, Status, Template};
use crate::workspace::trust::Trust;

impl App {
    /// Every task template, sorted by name: `.workspace` `[tasks]` only when the workspace is
    /// trusted (restricted or undecided: the user's `tasks.toml` alone), the workspace's winning a
    /// name both define.
    pub fn task_templates(&self) -> Vec<Template> {
        let workspace = (self.trust == Trust::Trusted).then_some(&self.workspace.config.tasks);
        self.tasks.templates(workspace)
    }

    /// Starts the template named `name` (`task_templates`); `None` (and a message) when there's
    /// no such template — e.g. a `.workspace` task in restricted mode.
    pub fn run_task(&mut self, name: &str) -> Option<RunId> {
        let Some(template) = self.task_templates().into_iter().find(|t| t.name == name) else {
            self.notify(Level::Error, format!("No task named '{name}'"));
            return None;
        };
        Some(self.start_task(&template))
    }

    /// Starts `template` (cwd relative to the workspace root).
    pub fn start_task(&mut self, template: &Template) -> RunId {
        tracing::info!(task = %template.name, source = ?template.source, "running task");
        self.tasks.run(template, &self.workspace.root)
    }

    /// `AppEvent::Task`: output and starts only need a redraw (`handle_app_event` asks for one);
    /// a run that couldn't start says why.
    pub(super) fn task_event(&mut self, ev: Event) {
        if let Event::Ended(id) = ev
            && let Some(run) = self.tasks.get(id)
            && let Status::Failed(e) = run.status()
        {
            let text = format!("Task '{}' failed to start: {e}", run.name);
            self.notify(Level::Error, text);
        }
    }

    /// On quit (and when the terminal goes away): language servers asked to exit and running
    /// tasks killed, both bounded (`lsp::SHUTDOWN_TIMEOUT`, `tasks::SHUTDOWN_TIMEOUT`).
    pub async fn shutdown(&mut self) {
        tokio::join!(self.lsp.shutdown(), self.tasks.shutdown());
    }
}
