//! Recent workspaces: the root recorded at start, "Open Recent Workspace" (Ctrl+R) listing the
//! others in the palette (folders gone since dropped, off-thread), and opening the one picked
//! (`crate::recent` does the disk work and decides how to open it).

use std::path::PathBuf;
use std::sync::Arc;

use super::{App, AppEvent, Level};
use crate::palette::Picker;
use crate::recent::{self, Entry, Open};
use crate::termctl::{self, Backend, Window};

/// The list's disk work and the picker's entries.
#[derive(Default)]
pub struct RecentState {
    /// The entries `Picker::Recent` lists, while it's open.
    pub picker: Option<Vec<Entry>>,
    /// Bumped by each "Open Recent Workspace": an older listing's result is dropped.
    listing: u64,
    /// Listings running (tests wait on these).
    pub listing_running: usize,
    /// Records of the root finished, failed ones too (tests wait on these).
    pub recorded: u64,
    /// The terminal's remote control (`termctl`), found at start.
    backend: Option<Arc<dyn Backend>>,
    /// What `termctl::detect` reads; the process's environment unless a test set it.
    env: Option<termctl::Env>,
    /// Focus / launch calls running (tests wait on these).
    pub opening_running: usize,
}

/// The recent workspaces read for the picker (`AppEvent::RecentListed`).
pub(super) struct Listed {
    listing: u64,
    result: Result<Vec<PathBuf>, String>,
    /// The terminal's windows running jed (none without a backend, or when it failed).
    windows: Vec<Window>,
}

/// A focus or launch finished (`AppEvent::TermCtlDone`).
pub(super) struct TermDone {
    focus: bool,
    path: PathBuf,
    result: Result<(), String>,
}

impl App {
    /// `start`: finds the terminal's remote control (once: it reads the environment).
    pub(super) fn detect_termctl(&mut self) {
        // Tests never see the real terminal: `test_termctl_env` is the only way in.
        let default = if cfg!(test) {
            termctl::Env::default
        } else {
            termctl::Env::from_process
        };
        let env = self.recent.env.take().unwrap_or_else(default);
        self.recent.backend = termctl::detect(&env).map(Arc::from);
        if let Some(b) = &self.recent.backend {
            tracing::info!(terminal = b.name(), "terminal remote control found");
        }
    }

    /// `start`: the workspace becomes the most recent one in `recent.toml` (nothing without a
    /// state dir).
    pub(super) fn record_recent(&mut self) {
        let Some(state) = self.state_dir.clone() else { return };
        let root = self.workspace.root.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = recent::record(&state, &root).map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::RecentRecorded(result));
        });
    }

    pub(super) fn recent_recorded(&mut self, result: Result<(), String>) {
        self.recent.recorded += 1;
        if let Err(e) = result {
            tracing::warn!(error = %e, "recent workspaces not recorded");
        }
    }

    /// "Open Recent Workspace" (`workspace.openRecent`, Ctrl+R): reads the list off-thread (missing
    /// folders pruned) and opens the picker with it when it arrives.
    pub(super) fn show_recent(&mut self) {
        let Some(state) = self.state_dir.clone() else {
            self.notify(Level::Info, "No recent workspaces: there's no state directory".into());
            return;
        };
        self.recent.listing += 1;
        self.recent.listing_running += 1;
        let listing = self.recent.listing;
        let current = self.workspace.root.clone();
        let tx = self.tx.clone();
        let backend = self.recent.backend.clone();
        tokio::task::spawn_blocking(move || {
            let result = recent::list(&state, &current).map_err(|e| e.to_string());
            let windows = backend
                .and_then(|b| match b.windows() {
                    Ok(w) => Some(w),
                    Err(e) => {
                        tracing::warn!(terminal = b.name(), error = %e, "can't list the terminal's windows");
                        None
                    }
                })
                .unwrap_or_default();
            let _ = tx.send(AppEvent::RecentListed(Box::new(Listed {
                listing,
                result,
                windows,
            })));
        });
    }

    /// `AppEvent::RecentListed`: the picker opens with the workspaces (none: a message).
    pub(super) fn recent_listed(&mut self, listed: Listed) {
        self.recent.listing_running -= 1;
        if listed.listing != self.recent.listing {
            return;
        }
        let paths = match listed.result {
            Ok(paths) => paths,
            Err(e) => {
                self.notify(Level::Error, format!("Can't read the recent workspaces: {e}"));
                return;
            }
        };
        if paths.is_empty() {
            self.notify(
                Level::Info,
                "No other recent workspaces: each folder jed opens is added".into(),
            );
            return;
        }
        self.open_palette(Picker::Recent, "");
        let windows = listed.windows;
        self.recent.picker = Some(
            paths
                .into_iter()
                .map(|path| {
                    let window = windows.iter().find(|w| w.workspace == path).map(|w| w.id.clone());
                    Entry { path, window }
                })
                .collect(),
        );
        self.refresh_palette();
    }

    /// The picker's choice: opened in another jed, never in this one. With a window already
    /// running it, that window is raised; else a new terminal window is launched; without a
    /// terminal backend (or when it fails) a message says how to open it by hand.
    pub(super) fn open_recent(&mut self, entry: Entry) {
        let backend = self.recent.backend.clone();
        match (recent::how_to_open(&entry, backend.is_some()), backend) {
            (Open::Focus(window), Some(b)) => self.run_termctl(b, true, entry.path, move |b, _| b.focus(&window)),
            (Open::Launch, Some(b)) => self.run_termctl(b, false, entry.path, |b, p| b.launch(p)),
            _ => self.recent_message(&entry.path, None),
        }
    }

    fn run_termctl(
        &mut self,
        backend: Arc<dyn Backend>,
        focus: bool,
        path: PathBuf,
        call: impl FnOnce(&dyn Backend, &std::path::Path) -> Result<(), String> + Send + 'static,
    ) {
        self.recent.opening_running += 1;
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = call(&*backend, &path);
            let _ = tx.send(AppEvent::TermCtlDone(Box::new(TermDone { focus, path, result })));
        });
    }

    /// `AppEvent::TermCtlDone`: says what happened; a failure is logged and falls back to the
    /// message with the command.
    pub(super) fn termctl_done(&mut self, done: TermDone) {
        self.recent.opening_running -= 1;
        let shown = recent::display(&done.path, &crate::paths::home());
        match done.result {
            Ok(()) if done.focus => self.notify(Level::Info, format!("Switched to {shown}")),
            Ok(()) => self.notify(Level::Info, format!("Opened {shown} in a new window")),
            Err(e) => {
                tracing::warn!(error = %e, path = %done.path.display(), "terminal window not opened");
                self.recent_message(&done.path, Some(&e));
            }
        }
    }

    fn recent_message(&mut self, path: &std::path::Path, why: Option<&str>) {
        let mut text = recent::message(path, &crate::paths::home());
        if let Some(why) = why {
            text.push_str(&format!(" ({why})"));
        }
        self.notify(Level::Info, text);
    }
}

#[cfg(test)]
impl App {
    /// Sets what the terminal detection reads (instead of the process's environment). Call
    /// before `test_start`.
    pub fn test_termctl_env(&mut self, env: termctl::Env) {
        self.recent.env = Some(env);
    }
}
