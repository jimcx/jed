//! Recent workspaces: the root recorded at start, "Open Recent Workspace" (Ctrl+R) listing the
//! others in the palette (folders gone since dropped, off-thread), and opening the one picked
//! (`crate::recent` does the disk work and decides how to open it).

use std::path::PathBuf;

use super::{App, AppEvent, Level};
use crate::palette::Picker;
use crate::recent::{self, Entry, Open};

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
}

/// The recent workspaces read for the picker (`AppEvent::RecentListed`).
pub(super) struct Listed {
    listing: u64,
    result: Result<Vec<PathBuf>, String>,
}

impl App {
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
        tokio::task::spawn_blocking(move || {
            let result = recent::list(&state, &current).map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::RecentListed(Box::new(Listed { listing, result })));
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
        self.recent.picker = Some(paths.into_iter().map(Entry::new).collect());
        self.refresh_palette();
    }

    /// The picker's choice: opened in another jed, never in this one (13d: kitty's windows).
    pub(super) fn open_recent(&mut self, entry: Entry) {
        match recent::how_to_open(&entry) {
            Open::Message => {
                let text = recent::message(&entry.path, &crate::paths::home());
                self.notify(Level::Info, text);
            }
        }
    }
}
