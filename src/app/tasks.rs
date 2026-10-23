//! Tasks: templates (`.workspace` only while trusted), the "Run Task" picker, and runs
//! (`app.tasks`, a `tasks::Tasks`) shown one per bottom-panel tab (`PanelTab::Task`, drawn by
//! `ui::task`): kill, close, scroll, and the keys / pastes the focused tab sends to its task.

use crossterm::event::{KeyEvent, MouseEvent};

use super::{App, Focus, Level};
use crate::command::TaskCommand as T;
use crate::layout::{self, PanelTab};
use crate::palette::Picker;
use crate::tasks::{self, Event, RunId, Status, Template, keys};
use crate::workspace::trust::Trust;

impl App {
    /// Every task template, sorted by name: `.workspace` `[tasks]` only when the workspace is
    /// trusted (restricted or undecided: the user's `tasks.toml` alone), the workspace's winning a
    /// name both define.
    pub fn task_templates(&self) -> Vec<Template> {
        let workspace = (self.trust == Trust::Trusted).then_some(&self.workspace.config.tasks);
        self.tasks.templates(workspace)
    }

    /// "Run Task" (`task.run`): the picker of templates, or a message saying where to define
    /// them (and that `.workspace`'s are off while the workspace isn't trusted).
    pub(super) fn show_run_task(&mut self) {
        if !self.task_templates().is_empty() {
            self.open_palette(Picker::Tasks, "");
            return;
        }
        let user = self.config_dir.join(tasks::templates::FILE_NAME);
        let text = match self.restricted_tasks_note() {
            Some(note) => format!("No tasks to run. {note}; or add [tasks.<name>] to {}", user.display()),
            None => format!(
                "No tasks defined: add [tasks.<name>] (command = \"…\") to {} or {}",
                crate::workspace::FILE_NAME,
                user.display()
            ),
        };
        self.notify(Level::Info, text);
    }

    /// Says that `.workspace` defines tasks that aren't offered because the workspace isn't
    /// trusted (`None` when it defines none or is trusted).
    pub(super) fn restricted_tasks_note(&self) -> Option<String> {
        let n = self.workspace.config.tasks.len();
        if n == 0 || self.trust == Trust::Trusted {
            return None;
        }
        let mode = match self.trust {
            Trust::Restricted => "restricted mode",
            _ => "a workspace not trusted yet",
        };
        let tasks = if n == 1 { "task" } else { "tasks" };
        Some(format!(
            "{n} {tasks} in {} not available in {mode} (Manage Workspace Trust)",
            crate::workspace::FILE_NAME
        ))
    }

    /// Starts the template named `name` (`task_templates`) in a new panel tab, shown and focused;
    /// `None` (and a message) when there's no such template — e.g. a `.workspace` task in
    /// restricted mode.
    pub fn run_task(&mut self, name: &str) -> Option<RunId> {
        let Some(template) = self.task_templates().into_iter().find(|t| t.name == name) else {
            self.notify(Level::Error, format!("No task named '{name}'"));
            return None;
        };
        let id = self.start_task(&template);
        self.focus_panel(PanelTab::Task(id));
        Some(id)
    }

    /// Starts `template` (cwd relative to the workspace root) at the size its tab will have:
    /// the panel's in the last frame's screen, shown at its height (else `tasks.size`, the last
    /// drawn task area's); no tab of its own (`run_task` adds that).
    pub fn start_task(&mut self, template: &Template) -> RunId {
        tracing::info!(task = %template.name, source = ?template.source, "running task");
        let status = self.hits.regions.status;
        if status.width > 0 {
            let screen = ratatui::layout::Rect::new(0, 0, status.width, status.y + status.height);
            let regions = layout::compute(
                screen,
                self.ui.side_visible,
                self.ui.side_width,
                Some(self.ui.panel_height),
            );
            let area = regions.panel.map(layout::task_area).unwrap_or_default();
            if area.width > 0 && area.height > 0 {
                self.tasks.size = (area.height, area.width);
            }
        }
        self.tasks.scrollback = self.settings.scrollback;
        self.tasks.run(template, &self.workspace.root)
    }

    /// The task tab the panel shows, if it shows one.
    pub(super) fn shown_task(&self) -> Option<RunId> {
        match self.ui.panel_tab {
            PanelTab::Task(id) if self.ui.panel_visible => Some(id),
            _ => None,
        }
    }

    /// The task tab that has the keys (`Context::Task`).
    pub(super) fn task_focused(&self) -> Option<RunId> {
        self.shown_task().filter(|_| self.ui.focus == Focus::Panel)
    }

    /// `task.kill`: the shown task tab's run, else the newest one running.
    pub(super) fn kill_task(&mut self) {
        let run = match self.shown_task() {
            Some(id) => self.tasks.get(id),
            None => self.tasks.newest_running(),
        };
        match run {
            Some(run) if run.status().is_running() => {
                tracing::info!(task = %run.name, "killing task");
                run.kill();
            }
            Some(run) => {
                let text = format!("Task '{}' isn't running", run.name);
                self.notify(Level::Info, text);
            }
            None => self.notify(Level::Info, "No task is running".into()),
        }
    }

    /// `task.close` (Cmd+W in a task tab): closes the shown task tab.
    pub(super) fn close_shown_task(&mut self) {
        match self.shown_task() {
            Some(id) => self.close_task(id),
            None => self.notify(Level::Info, "No task tab is shown".into()),
        }
    }

    /// Closes a run's tab (its × button, a middle click), killing the run if it still runs. The
    /// panel then shows the task tab before it (else after it), keeping the keys; with none
    /// left, the Problems list, and the editor gets the keys back.
    pub(super) fn close_task(&mut self, id: RunId) {
        let runs: Vec<RunId> = self.tasks.runs().iter().map(|r| r.id).collect();
        let Some(i) = runs.iter().position(|r| *r == id) else {
            return;
        };
        self.tasks.close(id);
        if self.ui.panel_tab != PanelTab::Task(id) {
            return;
        }
        let next = if i > 0 { runs.get(i - 1) } else { runs.get(i + 1) };
        match next {
            Some(&next) => self.ui.panel_tab = PanelTab::Task(next),
            None => {
                self.ui.panel_tab = PanelTab::Problems;
                if self.panel_focused() {
                    self.focus_editor_or_side();
                }
            }
        }
    }

    /// A task tab's keys: scrolling (a page = the screen's rows less one), back to the editor.
    /// Run from the palette, they act on the shown task tab.
    pub(super) fn task_command(&mut self, cmd: T) {
        if cmd == T::FocusEditor {
            if self.task_focused().is_some() {
                self.focus_editor_or_side();
            }
            return;
        }
        let Some(run) = self.shown_task().and_then(|id| self.tasks.get(id)) else {
            return;
        };
        let page = (run.screen().screen().size().0 as isize - 1).max(1);
        match cmd {
            T::ScrollUp => run.scroll(-1),
            T::ScrollDown => run.scroll(1),
            T::PageUp => run.scroll(-page),
            T::PageDown => run.scroll(page),
            T::Top => run.scroll_to(true),
            T::Bottom => run.scroll_to(false),
            T::FocusEditor => {}
        }
    }

    /// A key bound to nothing while a task tab has the keys: to the task's terminal, as a
    /// terminal would send it, the view back on the live screen. Nothing once the task ended.
    pub(super) fn task_key(&mut self, key: KeyEvent) {
        let Some(run) = self.task_focused().and_then(|id| self.tasks.get(id)) else {
            return;
        };
        let application_cursor = run.screen().screen().application_cursor();
        if let Some(bytes) = keys::encode(&key, application_cursor)
            && run.write(&bytes)
        {
            run.scroll_to(false);
        }
    }

    /// A paste while a task tab has the keys: into its terminal (bracketed when the task asked).
    pub(super) fn task_paste(&mut self, text: &str) {
        let Some(run) = self.task_focused().and_then(|id| self.tasks.get(id)) else {
            return;
        };
        let bracketed = run.screen().screen().bracketed_paste();
        if run.write(&keys::paste(text, bracketed)) {
            run.scroll_to(false);
        }
    }

    /// The wheel over a task tab: through its scrollback (negative: up).
    pub(super) fn task_scroll(&mut self, delta: isize) {
        if let Some(run) = self.shown_task().and_then(|id| self.tasks.get(id)) {
            run.scroll(delta);
        }
    }

    /// A click in a task tab's terminal: the tab gets the keys.
    pub(super) fn task_click(&mut self, _m: MouseEvent) {
        if let Some(id) = self.shown_task() {
            self.focus_panel(PanelTab::Task(id));
        }
    }

    /// `AppEvent::Task`: output and starts only need a redraw (`handle_app_event` asks for one).
    /// An ended run gets its end note below its output ("Task 'build' exited with code 2"), also
    /// shown in the status bar.
    pub(super) fn task_event(&mut self, ev: Event) {
        let Event::Ended(id) = ev else { return };
        let Some(run) = self.tasks.get(id) else { return };
        let status = run.status();
        let Some(note) = tasks::end_note(&run.name, &status) else {
            return;
        };
        run.append_line(&note);
        let level = match &status {
            Status::Failed(_) => Level::Error,
            Status::Exited(e) if e.success() || e.killed => Level::Info,
            _ => Level::Warning,
        };
        self.notify(level, note);
    }

    /// On quit (and when the terminal goes away): language servers asked to exit and running
    /// tasks killed, both bounded (`lsp::SHUTDOWN_TIMEOUT`, `tasks::SHUTDOWN_TIMEOUT`), and the
    /// session written (`save_session`), all at once.
    pub async fn shutdown(&mut self) {
        self.ipc_shutdown();
        let session = self.save_session();
        let session = async {
            if let Some(task) = session
                && let Err(e) = task.await
            {
                tracing::warn!("session not saved: {e}");
            }
        };
        tokio::join!(self.lsp.shutdown(), self.tasks.shutdown(), session);
        // The servers' last lines (their "Stopped") onto disk.
        self.flush_tool_logs().await;
        // After the session: a clean quit removes the backups, else they're brought up to date.
        self.finish_backups().await;
    }
}
