//! Shell tabs (`Tab::Shell`): "Terminal: New Shell" opens the user's `$SHELL` in the active
//! group; its keys and pastes go to the shell (`Context::Shell`, `tasks::keys`), its run's events
//! rename it while a command runs and append the exit note, and quitting hangs every shell up.
//! Drawing is `ui::task::render_shell`.

use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::KeyEvent;

use super::tasks::{Scroll, end_run_note, scroll_run, send_key, send_paste};
use super::{App, AppEvent, Focus, Level, Prompt};
use crate::command::ShellCommand;
use crate::group::{Groups, Tab};
use crate::shell::{self, ShellTab};
use crate::tasks::{self, Event, Run, Sink};
use crate::{paths, shell_integration};

impl App {
    /// "Terminal: New Shell" (`terminal.newShell`, Ctrl+`): the user's `$SHELL` (else
    /// `/bin/sh`) in a new tab of the active group, interactive in the workspace root, with
    /// `JED_IPC` when the socket listens. The title counts open shells of the same program.
    pub(super) fn new_shell(&mut self) {
        let program = self.tasks.shell.clone();
        let root = self.workspace.root.clone();
        let ipc = self.tasks.ipc.clone();
        let (rows, cols) = self
            .hits
            .group(self.groups.active)
            .map(|hit| (hit.editor.height, hit.editor.width))
            .filter(|&(rows, cols)| rows > 0 && cols > 0)
            .unwrap_or(tasks::DEFAULT_SIZE);
        let spec = shell::spec(&program, &root, ipc.as_deref());
        let base = spec.name.clone();
        // The smallest number no open shell of `base` has: closing "zsh (2)" gives that name to
        // the next shell instead of repeating "zsh (3)". A shell's number stays put while its
        // title changes.
        let number = (1..)
            .find(|&n| !self.groups.shell_tabs().any(|s| s.base == base && s.number == n))
            .unwrap_or(1);
        let id = self.tasks.new_run_id();
        let tx = self.tx.clone();
        let sink: Sink = Arc::new(move |ev| {
            let _ = tx.send(AppEvent::Shell(ev));
        });
        let tab = ShellTab::new(id, spec, rows, cols, sink, self.settings.scrollback, number);
        tracing::info!(shell = %base, run = id.0, "opened a shell tab");
        self.group_mut().open_tab(Tab::Shell(Box::new(tab)));
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
    }

    /// "Terminal: Install Shell Integration" (`terminal.installShellIntegration`): writes the three
    /// snippets into the state dir off-thread and opens the wizard with the detected shell's init
    /// file and the line sourcing its snippet. A shell without a snippet (`sh`) gets a message
    /// instead; the shell itself is never run, so a new shell tab is needed to pick the line up.
    pub(super) fn install_shell_integration(&mut self) {
        let Some(state) = self.state_dir.clone() else {
            self.notify(
                Level::Warning,
                "No shell integration: there's no state directory".into(),
            );
            return;
        };
        let base = shell::base(&self.tasks.shell);
        let Some(init) = shell_integration::init_for(&paths::home(), &paths::config_home(), &state, &base) else {
            self.notify(
                Level::Warning,
                format!("No shell integration for '{base}': zsh, bash and fish have one"),
            );
            return;
        };
        let dir = state.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = shell_integration::write_snippets(&dir);
            let _ = tx.send(AppEvent::ShellIntegrationWritten(result));
        });
        self.prompt = Some(Prompt::InstallShellIntegration {
            snippet: init.snippet,
            init: init.init,
            line: init.source,
        });
    }

    /// Whether a shell tab has the keys (`Context::Shell`).
    pub(super) fn shell_focused(&self) -> bool {
        self.ui.focus == Focus::Editor && self.group().active_shell().is_some()
    }

    /// A shell tab's scroll commands: the active shell's run, else nothing.
    pub(super) fn shell_command(&mut self, cmd: ShellCommand) {
        let Some(shell) = self.group().active_shell() else {
            return;
        };
        scroll_run(
            &shell.run,
            match cmd {
                ShellCommand::ScrollUp => Scroll::Up,
                ShellCommand::ScrollDown => Scroll::Down,
                ShellCommand::PageUp => Scroll::PageUp,
                ShellCommand::PageDown => Scroll::PageDown,
                ShellCommand::Top => Scroll::Top,
                ShellCommand::Bottom => Scroll::Bottom,
            },
        );
    }

    /// A key bound to nothing while a shell tab has the keys: to the shell, as a terminal sends
    /// it, the view back on the live screen.
    pub(super) fn shell_key(&mut self, key: KeyEvent) {
        let Some(shell) = self.group().active_shell() else {
            return;
        };
        send_key(&shell.run, key);
    }

    /// A paste while a shell tab has the keys: into the shell (bracketed when it asked).
    pub(super) fn shell_paste(&mut self, text: &str) {
        let Some(shell) = self.group().active_shell() else {
            return;
        };
        send_paste(&shell.run, text);
    }

    /// `AppEvent::Shell`: output and starts only need a redraw (`handle_app_event` asks for one);
    /// a title change renames the tab (its glyph says running from idle, red when the command
    /// failed); an ended shell keeps its tab, its output and a note below it ("Shell 'zsh' exited
    /// with code 3"), also in the status bar. A tab closed meanwhile leaves events naming an id
    /// nothing holds.
    pub(super) fn shell_event(&mut self, ev: Event) {
        match ev {
            Event::Foreground { id, name, idle, error } => {
                let Some(shell) = self.groups.shell_tabs_mut().find(|s| s.run.id == id) else {
                    return;
                };
                shell.title = shell::title(&shell.base, shell.number, Some((&name, idle)));
                shell.idle = idle;
                shell.error = error;
            }
            Event::Ended(id) => {
                let Some(shell) = self.groups.shell_tabs().find(|s| s.run.id == id) else {
                    return;
                };
                let Some((level, note)) = end_run_note(&shell.run, "Shell", &shell.base) else {
                    return;
                };
                self.notify(level, note);
            }
            Event::Started(_) | Event::Output(_) => {}
        }
    }

    /// `AppEvent::ShellIntegrationWritten`: only a failure needs saying (the wizard still opens:
    /// copying works, and a later install writes the files again).
    pub(super) fn shell_integration_written(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => tracing::debug!("shell integration snippets written"),
            Err(e) => self.notify(Level::Error, format!("Can't write the shell integration snippets: {e}")),
        }
    }

    /// `AppEvent::ShellIntegrationInstalled`: the line is in the init file — a new shell tab picks
    /// it up from there — or the append failed.
    pub(super) fn shell_integration_installed(&mut self, result: Result<PathBuf, String>) {
        match result {
            Ok(init) => self.notify(
                Level::Info,
                format!(
                    "Appended to {} — open a new shell tab",
                    crate::recent::display(&init, &paths::home())
                ),
            ),
            Err(e) => self.notify(Level::Error, format!("Can't install the shell integration: {e}")),
        }
    }
}

/// On quit: SIGHUP to every shell, up to `tasks::SHUTDOWN_TIMEOUT` for them to end, then SIGKILL
/// for the rest (as tasks do). Their tabs stay, so a frame drawn meanwhile shows the note.
pub(super) async fn shutdown_shells(groups: &Groups) {
    let runs: Vec<&Run> = groups.shell_tabs().map(|s| &s.run).collect();
    tasks::hang_up(runs.into_iter(), "shell").await;
}
