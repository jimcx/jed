//! Shell tabs (`Tab::Shell`): "Terminal: New Shell" opens the user's `$SHELL` in the active
//! group; its keys and pastes go to the shell (`Context::Shell`, `tasks::keys`), its run's events
//! redraw it and append the exit note, and quitting hangs every shell up. Drawing is
//! `ui::task::render_shell`.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::KeyEvent;

use super::{App, AppEvent, Focus, Level};
use crate::command::ShellCommand;
use crate::group::{Groups, Tab};
use crate::shell::{self, ShellTab};
use crate::tasks::{self, Event, Sink, Status, keys};

impl App {
    /// "Terminal: New Shell" (`terminal.newShell`, unbound): the user's `$SHELL` (else
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
        let base = shell::base(&program);
        // The smallest number no open shell of `base` has: closing "zsh (2)" gives that name to
        // the next shell instead of repeating "zsh (3)".
        let number = (1..)
            .find(|&n| {
                let title = shell::title(&base, n);
                !self.groups.shell_tabs().any(|s| s.base == base && s.title == title)
            })
            .unwrap_or(1);
        let id = self.tasks.new_run_id();
        let tx = self.tx.clone();
        let sink: Sink = Arc::new(move |ev| {
            let _ = tx.send(AppEvent::Shell(ev));
        });
        let mut tab = ShellTab::new(
            id,
            &program,
            &root,
            ipc.as_deref(),
            rows,
            cols,
            sink,
            self.settings.scrollback,
        );
        tab.title = shell::title(&base, number);
        tracing::info!(shell = %base, run = id.0, "opened a shell tab");
        self.group_mut().open_tab(Tab::Shell(Box::new(tab)));
        self.ui.focus = Focus::Editor;
        self.needs_redraw = true;
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
        let run = &shell.run;
        let page = (run.screen().screen().size().0 as isize - 1).max(1);
        match cmd {
            ShellCommand::ScrollUp => run.scroll(-1),
            ShellCommand::ScrollDown => run.scroll(1),
            ShellCommand::PageUp => run.scroll(-page),
            ShellCommand::PageDown => run.scroll(page),
            ShellCommand::Top => run.scroll_to(true),
            ShellCommand::Bottom => run.scroll_to(false),
        }
    }

    /// A key bound to nothing while a shell tab has the keys: to the shell, as a terminal sends
    /// it, the view back on the live screen.
    pub(super) fn shell_key(&mut self, key: KeyEvent) {
        let Some(shell) = self.group().active_shell() else {
            return;
        };
        let run = &shell.run;
        let application_cursor = run.screen().screen().application_cursor();
        if let Some(bytes) = keys::encode(&key, application_cursor)
            && run.write(&bytes)
        {
            run.scroll_to(false);
        }
    }

    /// A paste while a shell tab has the keys: into the shell (bracketed when it asked).
    pub(super) fn shell_paste(&mut self, text: &str) {
        let Some(shell) = self.group().active_shell() else {
            return;
        };
        let run = &shell.run;
        let bracketed = run.screen().screen().bracketed_paste();
        if run.write(&keys::paste(text, bracketed)) {
            run.scroll_to(false);
        }
    }

    /// `AppEvent::Shell`: output and starts only need a redraw; an ended shell keeps its tab,
    /// its output and a note below it ("Shell 'zsh' exited with code 3"), also in the status
    /// bar. A tab closed meanwhile leaves events naming an id nothing holds.
    pub(super) fn shell_event(&mut self, ev: Event) {
        let id = match ev {
            Event::Started(id) | Event::Output(id) | Event::Ended(id) => id,
        };
        let (note, level) = {
            let Some(shell) = self.groups.shell_tabs().find(|s| s.run.id == id) else {
                return;
            };
            self.needs_redraw = true;
            let Event::Ended(_) = ev else { return };
            let status = shell.run.status();
            let Some(note) = shell::end_note(&shell.title, &status) else {
                return;
            };
            shell.run.append_line(&note);
            let level = match &status {
                Status::Failed(_) => Level::Error,
                Status::Exited(e) if e.success() || e.killed => Level::Info,
                _ => Level::Warning,
            };
            (note, level)
        };
        self.notify(level, note);
    }
}

/// On quit: SIGHUP to every shell, up to `tasks::SHUTDOWN_TIMEOUT` for them to end, then SIGKILL
/// for the rest (as tasks do). Their tabs stay, so a frame drawn meanwhile shows the note.
pub(super) async fn shutdown_shells(groups: &Groups) {
    let running = || groups.shell_tabs().filter(|s| s.run.status().is_running()).count();
    if running() == 0 {
        return;
    }
    for shell in groups.shell_tabs() {
        shell.run.kill();
    }
    let deadline = tokio::time::Instant::now() + tasks::SHUTDOWN_TIMEOUT;
    while running() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for shell in groups.shell_tabs() {
        if shell.run.status().is_running() {
            tracing::warn!(shell = %shell.title, "shell still running at quit: killing it");
            shell.run.force_kill();
        }
    }
}
