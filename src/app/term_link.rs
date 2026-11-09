//! Cmd/Ctrl+click on a `path[:line[:col]]` in a shell or task tab's output: opens the file in the
//! editor area at that line. Nothing is scanned or resolved until the click (the one cell's row
//! is read, then a `stat` or two off the UI thread). A path that is not one file (missing,
//! ambiguous, a folder) goes into quick open's input instead, to pick from.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::files::Goto;
use super::{App, AppEvent, Focus, Level};
use crate::command::Command;
use crate::group::is_terminal_group;
use crate::layout::{DropSide, GroupId, Hit};
use crate::palette::Picker;
use crate::tasks::select;

impl App {
    /// A Cmd/Ctrl+left press on a link-like word of a shell or task tab: starts resolving it.
    /// False for any other event or cell, which then goes on as a normal click.
    pub(super) fn terminal_link_click(&mut self, hit: Hit, m: MouseEvent) -> bool {
        let link = KeyModifiers::CONTROL | KeyModifiers::SUPER;
        if m.kind != MouseEventKind::Down(MouseButton::Left) || !m.modifiers.intersects(link) {
            return false;
        }
        let Some((run, area)) = self.terminal_run(hit) else {
            return false;
        };
        let (url, word) = {
            let screen = run.screen();
            let s = screen.screen();
            let (row, col) = super::mouse::screen_cell(s, area, m.column, m.row);
            let url = select::wrapped_line(s, (row, col))
                .and_then(|(line, at)| crate::url::url_at(&line, at).map(str::to_string));
            (url, select::path_text(s, (row, col)))
        };
        if let Some(url) = url {
            self.open_url(url);
            return true;
        }
        let Some(word) = word else { return false };
        let Some((path, pos)) = crate::paths::terminal_link(&word) else {
            return false;
        };
        let (path, goto) = (path.to_string(), pos.map(|(l, c)| Goto::Cursor(l, c)));
        let cwd = run.cwd.clone();
        // Quick open matches workspace-relative paths: an absolute one is searched by file name.
        let name = if path.starts_with(['/', '~']) {
            path.rsplit('/').next().unwrap_or(&path)
        } else {
            path.trim_start_matches("./")
        };
        let query = match pos {
            Some((l, c)) => format!("{name}:{l}:{c}"),
            None => name.to_string(),
        };
        // A shell in an editor group stays on screen: the file opens beside it.
        let from = match hit {
            Hit::Editor(g) if !is_terminal_group(g) => Some(g),
            _ => None,
        };
        let (root, tx) = (self.workspace.root.clone(), self.tx.clone());
        tokio::task::spawn_blocking(move || {
            let found = resolve(&path, &cwd, &root);
            let _ = tx.send(AppEvent::Terminal(super::shell::TerminalEvent::Link {
                found,
                goto,
                query,
                from,
            }));
        });
        true
    }

    /// Opens a Cmd/Ctrl+clicked web link in the system's browser, off the UI thread; a failure
    /// comes back as `TerminalEvent::UrlFailed`.
    pub(super) fn open_url(&mut self, url: String) {
        tracing::info!(target: "jed::app", %url, "opening link");
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = crate::url::open(&url) {
                let _ = tx.send(AppEvent::Terminal(super::shell::TerminalEvent::UrlFailed(format!(
                    "Couldn't open {url}: {e}"
                ))));
            }
        });
    }

    /// A clicked link resolved: open the file, or offer `query` in quick open.
    pub(super) fn terminal_link_resolved(
        &mut self,
        found: Option<PathBuf>,
        goto: Option<Goto>,
        query: String,
        from: Option<GroupId>,
    ) {
        match found {
            Some(path) => {
                if let Some(from) = from.filter(|g| self.groups.get(*g).is_some()) {
                    // Another group if there is one, else a new one on the right.
                    let other = self.groups.ids().into_iter().find(|g| *g != from);
                    if let Some(target) = other.or_else(|| self.groups.split_empty_of(from, DropSide::Right)) {
                        self.focus_group(target);
                    }
                }
                // The tab commands act on the focused area's groups: with the terminal focused
                // `open_file` would activate a tab of the terminal area, not the editor's.
                self.ui.focus = Focus::Editor;
                if from.is_none()
                    && let Some(id) = self.buffers.find_path(&path)
                {
                    // Where the file is already showing: a group with it active, else any with it.
                    let ids = self.groups.ids();
                    let shows = |g: &GroupId| {
                        self.groups
                            .get(*g)
                            .and_then(|g| g.active_tab())
                            .is_some_and(|t| t.buffer == id)
                    };
                    let has = |g: &GroupId| self.groups.get(*g).is_some_and(|g| g.find(id).is_some());
                    let target = ids
                        .iter()
                        .copied()
                        .find(|g| *g == self.groups.active && shows(g))
                        .or_else(|| ids.iter().copied().find(shows))
                        .or_else(|| ids.iter().copied().find(has));
                    if let Some(target) = target {
                        self.focus_group(target);
                    }
                }
                self.open_file(path, true, goto)
            }
            None if self.mode.allows(Command::QuickOpen) => self.open_palette(Picker::QuickOpen, &query),
            None => self.notify(Level::Warning, format!("No file {query} in the workspace")),
        }
    }
}

/// The one file `path` names: as given when absolute, under `~`, else under the terminal's
/// directory, else the workspace root. `None` when none of those is a file.
/// Blocking: call from spawn_blocking
fn resolve(path: &str, cwd: &Path, root: &Path) -> Option<PathBuf> {
    let direct = if let Some(rest) = path.strip_prefix("~/") {
        vec![crate::paths::home().join(rest)]
    } else if Path::new(path).is_absolute() {
        vec![PathBuf::from(path)]
    } else {
        vec![cwd.join(path), root.join(path)]
    };
    direct
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.components().collect())
}
