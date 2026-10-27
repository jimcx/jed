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
    /// `/bin/sh`) in a new tab of the terminal area (shown and focused), interactive in the workspace root, with
    /// `JED_IPC` when the socket listens. The title counts open shells of the same program.
    pub(super) fn new_shell(&mut self) {
        self.open_shell(true);
    }

    /// Opens a shell tab in the active editor group instead of the terminal area (a shell tab
    /// can live in either; the tests of editor-group shell tabs use this).
    #[cfg(test)]
    pub(crate) fn new_shell_in_editor(&mut self) {
        self.open_shell(false);
    }

    /// A new shell in the terminal area (showing it, `Focus::Terminal`), or in the active editor
    /// group (`Focus::Editor`).
    fn open_shell(&mut self, in_terminal: bool) {
        let program = self.tasks.shell.clone();
        let root = self.workspace.root.clone();
        let ipc = self.tasks.ipc.clone();
        let (rows, cols) = if in_terminal {
            self.terminal_size()
        } else {
            self.hits
                .group(self.groups.active)
                .map(|hit| (hit.editor.height, hit.editor.width))
                .filter(|&(rows, cols)| rows > 0 && cols > 0)
                .unwrap_or(tasks::DEFAULT_SIZE)
        };
        #[cfg(test)]
        let spec = {
            let mut spec = shell::spec(&program, &root, ipc.as_deref());
            spec.start_gate = self.tasks.start_gate.clone();
            spec
        };
        #[cfg(not(test))]
        let spec = shell::spec(&program, &root, ipc.as_deref());
        let base = spec.name.clone();
        // The smallest number no open shell of `base` has: closing "zsh (2)" gives that name to
        // the next shell instead of repeating "zsh (3)". A shell's number stays put while its
        // title changes.
        let number = (1..)
            .find(|&n| !self.shell_tabs().any(|s| s.base == base && s.number == n))
            .unwrap_or(1);
        let id = self.tasks.new_run_id();
        let tx = self.tx.clone();
        let sink: Sink = Arc::new(move |ev| {
            let _ = tx.send(AppEvent::Shell(ev));
        });
        let tab = ShellTab::new(id, spec, rows, cols, sink, self.settings.scrollback, number);
        tracing::info!(shell = %base, run = id.0, "opened a shell tab");
        if in_terminal {
            self.terminals.active_group_mut().open_tab(Tab::Shell(Box::new(tab)));
            self.ui.terminal_visible = true;
            self.ui.focus = Focus::Terminal;
        } else {
            self.group_mut().open_tab(Tab::Shell(Box::new(tab)));
            self.ui.focus = Focus::Editor;
        }
        self.needs_redraw = true;
    }

    /// The PTY size (rows, columns) for a new shell: the terminal area's active group as last
    /// drawn, else the size it will have at the current height on the last frame's screen,
    /// else the default.
    fn terminal_size(&self) -> (u16, u16) {
        let drawn = self
            .hits
            .group(self.terminals.active)
            .map(|hit| (hit.editor.height, hit.editor.width))
            .filter(|&(rows, cols)| rows > 0 && cols > 0);
        let status = self.hits.regions.status;
        let planned = (status.width > 0)
            .then(|| {
                let screen = ratatui::layout::Rect::new(0, 0, status.width, status.y + status.height);
                let t = crate::layout::compute(
                    screen,
                    self.ui.side_visible,
                    self.ui.side_width,
                    None,
                    Some(self.ui.terminal_height),
                )
                .terminal?;
                // Border and tab bar above the terminal, the scroll bar's column beside it.
                Some((t.height.saturating_sub(2), t.width.saturating_sub(1)))
            })
            .flatten()
            .filter(|&(rows, cols)| rows > 0 && cols > 0);
        drawn.or(planned).unwrap_or(tasks::DEFAULT_SIZE)
    }

    /// Every shell tab, in the terminal area and in editor groups.
    pub(crate) fn shell_tabs(&self) -> impl Iterator<Item = &ShellTab> {
        self.terminals.shell_tabs().chain(self.groups.shell_tabs())
    }

    /// The group `id` names: an editor group's or a terminal area group's.
    pub(super) fn any_group(&self, id: crate::layout::GroupId) -> Option<&crate::group::Group> {
        if crate::group::is_terminal_group(id) {
            self.terminals.get(id)
        } else {
            self.groups.get(id)
        }
    }

    pub(super) fn any_group_mut(&mut self, id: crate::layout::GroupId) -> Option<&mut crate::group::Group> {
        if crate::group::is_terminal_group(id) {
            self.terminals.get_mut(id)
        } else {
            self.groups.get_mut(id)
        }
    }

    /// The groups `id` belongs to: the terminal area's or the editor's.
    pub(super) fn groups_of_mut(&mut self, id: crate::layout::GroupId) -> &mut Groups {
        if crate::group::is_terminal_group(id) {
            &mut self.terminals
        } else {
            &mut self.groups
        }
    }

    /// The groups the tab commands (activate, cycle, close) act on: the terminal area's with
    /// `Focus::Terminal`, else the editor groups'.
    pub(super) fn tab_groups(&self) -> &Groups {
        if self.ui.focus == Focus::Terminal {
            &self.terminals
        } else {
            &self.groups
        }
    }

    pub(super) fn tab_groups_mut(&mut self) -> &mut Groups {
        if self.ui.focus == Focus::Terminal {
            &mut self.terminals
        } else {
            &mut self.groups
        }
    }

    /// After a group command acted on `tab_groups()`: the terminal area keeps its focus, any other
    /// focus (side panel, ...) goes to the editor.
    pub(super) fn focus_tab_groups(&mut self) {
        if self.ui.focus != Focus::Terminal {
            self.ui.focus = Focus::Editor;
        }
    }

    /// The shell tab that has the keys: the terminal area's active one with `Focus::Terminal`, the
    /// active editor group's with `Focus::Editor`.
    pub(super) fn focused_shell(&self) -> Option<&ShellTab> {
        match self.ui.focus {
            Focus::Terminal => self.terminals.active_group().active_shell(),
            Focus::Editor => self.group().active_shell(),
            _ => None,
        }
    }

    /// Hides the terminal area once none of its groups holds a terminal, handing its focus to the editor.
    pub(super) fn hide_empty_terminals(&mut self) {
        if self.terminals.shell_tabs().next().is_none() {
            self.ui.terminal_visible = false;
            if self.ui.focus == Focus::Terminal {
                self.ui.focus = Focus::Editor;
            }
        }
    }

    /// The terminal a new session starts with (`startup_terminal`, workspace mode): one shell in
    /// the shown terminal area, the focus staying where it was (the editor, or the side panel
    /// with no file), so typing goes to the opened file. The area is independent of the editor
    /// groups, so a session restoring afterwards doesn't disturb it, and it is never saved.
    pub(super) fn start_terminal(&mut self) {
        if !self.startup_terminal {
            return;
        }
        let focus = self.ui.focus;
        self.new_shell();
        self.ui.focus = focus;
    }

    /// "Terminal: Move Terminal into Editor Area" (`to_area` false: the terminal area's active
    /// group's shell goes into the active editor group) / "... into Terminal Area" (`to_area`
    /// true: the active editor group's shell goes into the area's active group, showing it). The
    /// same move as a drag (`move_tab_across`), the shell staying alive; the target area has the
    /// focus after. Without a shell to move, a message.
    pub(super) fn move_terminal(&mut self, to_area: bool) {
        let (from, to) = if to_area {
            (self.groups.active, self.terminals.active)
        } else {
            (self.terminals.active, self.groups.active)
        };
        let Some(group) = self.any_group(from).filter(|g| g.active_shell().is_some()) else {
            let msg = if to_area {
                "The active tab is not a terminal"
            } else {
                "No terminal in the terminal area"
            };
            self.notify(Level::Info, msg.into());
            return;
        };
        let index = group.active;
        if self.move_tab_across(from, index, to, None) {
            self.focus_dropped(to);
        }
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
        self.focused_shell().is_some()
    }

    /// A shell tab's scroll commands: the active shell's run, else nothing.
    pub(super) fn shell_command(&mut self, cmd: ShellCommand) {
        let Some(shell) = self.focused_shell() else {
            return;
        };
        if cmd == ShellCommand::Clear {
            shell.run.clear();
            return;
        }
        scroll_run(
            &shell.run,
            match cmd {
                ShellCommand::ScrollUp => Scroll::Up,
                ShellCommand::ScrollDown => Scroll::Down,
                ShellCommand::PageUp => Scroll::PageUp,
                ShellCommand::PageDown => Scroll::PageDown,
                ShellCommand::Top => Scroll::Top,
                ShellCommand::Bottom => Scroll::Bottom,
                ShellCommand::Clear => return,
            },
        );
    }

    /// A key bound to nothing while a shell tab has the keys: to the shell, as a terminal sends
    /// it, the view back on the live screen.
    pub(super) fn shell_key(&mut self, key: KeyEvent) {
        let Some(shell) = self.focused_shell() else {
            return;
        };
        send_key(&shell.run, key);
    }

    /// A paste while a shell tab has the keys: into the shell (bracketed when it asked).
    pub(super) fn shell_paste(&mut self, text: &str) {
        let Some(shell) = self.focused_shell() else {
            return;
        };
        send_paste(&shell.run, text);
    }

    /// `AppEvent::Shell`: output and starts only need a redraw (`handle_app_event` asks for one);
    /// new output clears the mouse selection (the rows moved). A title change renames the tab (its
    /// glyph says running from idle, red when the command failed); an ended shell keeps its tab,
    /// its output and a note below it ("Shell 'zsh' exited with code 3"), also in the status bar.
    /// A tab closed meanwhile leaves events naming an id nothing holds.
    pub(super) fn shell_event(&mut self, ev: Event) {
        match ev {
            Event::Foreground { id, name, idle, error } => {
                let Some(shell) = self
                    .terminals
                    .shell_tabs_mut()
                    .chain(self.groups.shell_tabs_mut())
                    .find(|s| s.run.id == id)
                else {
                    return;
                };
                shell.title = shell::title(&shell.base, shell.number, Some((&name, idle)));
                shell.idle = idle;
                shell.error = error;
            }
            Event::Ended(id) => {
                let Some(shell) = self.shell_tabs().find(|s| s.run.id == id) else {
                    return;
                };
                let Some((level, note)) = end_run_note(&shell.run, "Shell", &shell.base) else {
                    return;
                };
                self.notify(level, note);
            }
            Event::Output(id) => {
                if let Some(shell) = self.shell_tabs().find(|s| s.run.id == id) {
                    shell.run.clear_selection();
                }
            }
            Event::Started(_) => {}
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
pub(super) async fn shutdown_shells(groups: &Groups, terminals: &Groups) {
    let runs: Vec<&Run> = terminals
        .shell_tabs()
        .chain(groups.shell_tabs())
        .map(|s| &s.run)
        .collect();
    tasks::hang_up(runs.into_iter(), "shell").await;
}
