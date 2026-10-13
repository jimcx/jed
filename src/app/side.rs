//! Side panel: explorer commands, listings, file operations, file watching, `.workspace` reloads.

use std::path::{Path, PathBuf};

use super::{App, AppEvent, FileOp, Focus, InputKind, Level, Prompt};
use crate::app::prompt::Input;
use crate::command::ExplorerCommand as X;
use crate::explorer::{self, Entry};
use crate::workspace;
use crate::workspace::config::WorkspaceConfig;
use crate::workspace::trust::Trust;

impl App {
    pub(super) fn explorer_command(&mut self, cmd: X) {
        let page = self.explorer.height.max(1) as isize;
        match cmd {
            X::Up => self.explorer.move_selection(-1),
            X::Down => self.explorer.move_selection(1),
            X::PageUp => self.explorer.move_selection(-page),
            X::PageDown => self.explorer.move_selection(page),
            X::First => self.explorer.select_index(0),
            X::Last => self.explorer.select_last(),
            X::Left => self.explorer.left(),
            X::Right => {
                if let Some(dir) = self.explorer.right() {
                    self.request_listing(dir);
                }
            }
            X::Open => self.open_selected(true),
            X::Preview => self.open_selected(false),
            X::NewFile => {
                let dir = self.explorer.target_dir();
                self.prompt = Some(Prompt::Input(Input::new(InputKind::NewFile { dir }, String::new())));
            }
            X::NewFolder => {
                let dir = self.explorer.target_dir();
                self.prompt = Some(Prompt::Input(Input::new(InputKind::NewFolder { dir }, String::new())));
            }
            X::Rename => {
                if let Some(from) = self.explorer.selected.clone() {
                    let name = from.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    self.prompt = Some(Prompt::Input(Input::new(InputKind::Rename { from }, name)));
                }
            }
            X::Delete => {
                if let Some(path) = self.explorer.selected.clone() {
                    let is_dir = self.explorer.find(&path).is_some_and(|e| e.is_dir);
                    self.prompt = Some(Prompt::ConfirmDelete { path, is_dir });
                }
            }
        }
    }

    /// A file opens (focusing the editor if `focus`); a folder toggles.
    pub(super) fn open_selected(&mut self, focus: bool) {
        let Some(path) = self.explorer.selected.clone() else {
            return;
        };
        self.toggle_or_open(&path, focus);
    }

    pub(super) fn toggle_or_open(&mut self, path: &Path, focus: bool) {
        match self.explorer.find(path) {
            Some(e) if e.is_dir && e.expanded => self.explorer.collapse(path),
            Some(e) if e.is_dir => {
                if let Some(dir) = self.explorer.expand(path) {
                    self.request_listing(dir);
                }
            }
            Some(_) => self.open_file(path.to_path_buf(), focus),
            None => {}
        }
    }

    pub(super) fn request_listing(&mut self, dir: PathBuf) {
        if !self.listing.insert(dir.clone()) {
            return;
        }
        let excludes = self.excludes.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = explorer::list_dir(&dir, &excludes);
            let _ = tx.send(AppEvent::Listed { dir, result });
        });
    }

    pub(super) fn listed(&mut self, dir: PathBuf, result: std::io::Result<Vec<Entry>>) {
        self.listing.remove(&dir);
        match result {
            Ok(entries) => {
                for next in self.explorer.set_children(&dir, entries) {
                    self.request_listing(next);
                }
            }
            Err(e) if dir == self.workspace.root => {
                self.notify(Level::Error, format!("Can't list {}: {e}", dir.display()))
            }
            // Usually a directory removed while listing; its parent's refresh drops it.
            Err(e) => tracing::debug!(dir = %dir.display(), "listing failed: {e}"),
        }
    }

    pub(super) fn run_file_op(&mut self, op: FileOp) {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = match &op {
                FileOp::NewFile(p) => explorer::ops::new_file(p),
                FileOp::NewFolder(p) => explorer::ops::new_folder(p),
                FileOp::Rename { from, to } => explorer::ops::rename(from, to),
                FileOp::Delete(p) => explorer::ops::delete(p),
            };
            let _ = tx.send(AppEvent::FileOpDone {
                op,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    pub(super) fn file_op_done(&mut self, op: FileOp, result: Result<(), String>) {
        tracing::info!(?op, ?result, "file operation");
        if let Err(e) = result {
            self.notify(Level::Error, e);
            return;
        }
        // Re-list every loaded folder above the changed path, so new subfolders appear too.
        let refresh = |app: &mut Self, p: &Path| {
            let dirs: Vec<PathBuf> = p
                .ancestors()
                .skip(1)
                .filter(|d| app.explorer.is_loaded(d))
                .map(Path::to_path_buf)
                .collect();
            for dir in dirs {
                app.request_listing(dir);
            }
        };
        match op {
            FileOp::NewFile(path) => {
                refresh(self, &path);
                self.reveal_path(&path);
                self.open_file(path, true);
            }
            FileOp::NewFolder(path) => {
                refresh(self, &path);
                self.reveal_path(&path);
            }
            FileOp::Rename { from, to } => {
                for (_, buf) in self.buffers.iter_mut() {
                    if let Ok(rest) = buf.path.strip_prefix(&from) {
                        buf.path = if rest.as_os_str().is_empty() {
                            to.clone()
                        } else {
                            to.join(rest)
                        };
                    }
                }
                refresh(self, &from);
                refresh(self, &to);
                self.reveal_path(&to);
            }
            FileOp::Delete(path) => {
                for (_, buf) in self.buffers.iter_mut() {
                    if buf.path.starts_with(&path) {
                        buf.deleted = true;
                    }
                }
                refresh(self, &path);
                self.notify(Level::Info, format!("Moved {} to the Trash", path.display()));
            }
        }
    }

    fn reveal_path(&mut self, path: &Path) {
        for dir in self.explorer.reveal(path) {
            self.request_listing(dir);
        }
    }

    /// Batched file system events: refresh loaded directories, reload `.workspace`.
    pub(super) fn flush_fs_changes(&mut self) {
        self.fs_deadline = None;
        let config = self.workspace.config_path();
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut reload = false;
        for p in std::mem::take(&mut self.fs_pending) {
            reload |= p == config;
            for d in [Some(p.as_path()), p.parent()].into_iter().flatten() {
                if self.explorer.is_loaded(d) && !dirs.iter().any(|x| x == d) {
                    dirs.push(d.to_path_buf());
                }
            }
        }
        for d in dirs {
            self.request_listing(d);
        }
        if reload {
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(AppEvent::ConfigLoaded(workspace::read_config(&config)));
            });
        }
    }

    pub(super) fn config_loaded(&mut self, result: Result<WorkspaceConfig, String>) {
        let config = match result {
            Ok(c) => c,
            Err(e) => {
                self.workspace.config_error = Some(e.clone());
                self.notify(Level::Error, e);
                return;
            }
        };
        self.workspace.config_error = None;
        if config == self.workspace.config {
            return;
        }
        let excludes_changed = config.files.exclude != self.workspace.config.files.exclude;
        self.workspace.config = config;
        self.settings = self.workspace.settings();
        if excludes_changed {
            self.excludes = explorer::excludes(&self.workspace.root, &self.workspace.config.files.exclude);
            for d in self.explorer.loaded_dirs() {
                self.request_listing(d);
            }
        }
        self.trust = self.trust_store.evaluate(&self.workspace.root, &self.workspace.config);
        self.notify(Level::Info, format!("Reloaded {}", workspace::FILE_NAME));
        if self.trust == Trust::Undecided {
            self.ask_trust();
        }
        if self.group.tabs.is_empty() && self.ui.focus == Focus::Editor {
            self.ui.focus = Focus::SidePanel;
        }
    }
}
