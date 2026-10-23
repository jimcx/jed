//! Shell tabs (`Tab::Shell`): "Terminal: New Shell" opens the user's `$SHELL` in the active
//! group; its run's events ask for a redraw. Drawing, keys, resizing and the exit note are
//! shell-b's (`ui::task`, `Context::Shell`).

use std::sync::Arc;

use super::{App, AppEvent, Focus};
use crate::group::Tab;
use crate::shell::{self, ShellTab};
use crate::tasks::{self, Event, Sink};

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

    /// `AppEvent::Shell`: output and starts only need a redraw (shell-b appends the exit note
    /// once a shell ends). A tab closed meanwhile leaves events naming an id nothing holds.
    pub(super) fn shell_event(&mut self, ev: Event) {
        let id = match ev {
            Event::Started(id) | Event::Output(id) | Event::Ended(id) => id,
        };
        if self.groups.shell_tabs().any(|s| s.run.id == id) {
            self.needs_redraw = true;
        }
    }
}
