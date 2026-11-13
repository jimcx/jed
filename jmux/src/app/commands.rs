//! The command history's feed and the shell integration's jobs: a run's `CommandRecord`s go into
//! `App::history` when its shell reports a command boundary (`Event::Foreground`) or ends; the
//! integration is checked once, on the first shell (`check_integration`), and installed on demand
//! (`install_integration`), both off-thread, reporting back as `AppEvent::Integration`.

use jterm::run::RunId;
use jterm::shell::{self, ShellTab};

use super::{App, AppEvent, IntegrationEvent};
use crate::integration;

fn tab_name(tab: &ShellTab) -> String {
    if tab.number <= 1 {
        tab.base.clone()
    } else {
        format!("{} ({})", tab.base, tab.number)
    }
}

impl App {
    /// The tab title of `run` without its state glyph (`zsh`, `zsh (2)`), for the history's rows.
    fn run_name(&self, run: RunId) -> Option<String> {
        self.groups.tabs().find(|t| t.run.id == run).map(tab_name)
    }

    /// Hands the records of a tab just taken out of the groups (`close_tab`) to the history.
    pub(super) fn feed_history_from(&mut self, tab: &ShellTab) {
        let records = tab.run.commands();
        if !records.is_empty() {
            self.history.update(tab.run.id, &tab_name(tab), records);
        }
    }

    /// Hands `run`'s current records to the history (a run without the integration has none).
    pub(super) fn feed_history(&mut self, run: RunId) {
        let Some(tab) = self.groups.tabs().find(|t| t.run.id == run) else {
            return;
        };
        let records = tab.run.commands();
        if records.is_empty() {
            return;
        }
        let Some(title) = self.run_name(run) else {
            return;
        };
        self.history.update(run, &title, records);
        self.needs_redraw = true;
    }

    /// On the first shell started: reads the integration's state off-thread, once. Not without
    /// directories (and a shell with no snippet, `sh`, answers `None`: no notice).
    pub(super) fn check_integration(&mut self) {
        if self.integration_checked {
            return;
        }
        let Some(dirs) = self.config.dirs.clone() else {
            return;
        };
        self.integration_checked = true;
        let shell = shell::base(&self.config.program);
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            let status = integration::check(&dirs, &shell);
            let _ = events.send(AppEvent::Integration(IntegrationEvent::Checked { shell, status }));
        });
    }

    /// `Command::InstallShellIntegration`: writes the snippets and adds the line to the shell's
    /// init file off-thread; the result is the notice.
    pub(super) fn install_integration(&mut self) {
        let Some(dirs) = self.config.dirs.clone() else {
            self.notice = Some("No shell integration: there's no state directory".into());
            return;
        };
        let shell = shell::base(&self.config.program);
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            let result = integration::install(&dirs, &shell);
            let _ = events.send(AppEvent::Integration(IntegrationEvent::Installed(result)));
        });
    }

    pub(super) fn integration_event(&mut self, event: IntegrationEvent) {
        match event {
            IntegrationEvent::Checked { shell, status } => {
                tracing::debug!(%shell, ?status, "shell integration checked");
                if let Some(text) = integration::notice(&shell, status) {
                    self.notice = Some(text);
                    self.needs_redraw = true;
                }
            }
            IntegrationEvent::Installed(result) => {
                self.notice = Some(match result {
                    Ok(done) => done.message(self.config.dirs.as_ref().map_or(std::path::Path::new(""), |d| &d.home)),
                    Err(e) => format!("Can't install the shell integration: {e}"),
                });
                self.needs_redraw = true;
            }
        }
    }
}
