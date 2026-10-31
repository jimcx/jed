//! Side panel: explorer commands, listings, file operations, file watching, `.workspace` reloads.

use std::path::{Path, PathBuf};

use super::{App, AppEvent, FileOp, Focus, InputKind, Level, Prompt};
use crate::app::prompt::Input;
use crate::clipboard::Copied;
use crate::command::ExplorerCommand as X;
use crate::config::keymap::{self, Keymap, Platform};
use crate::config::settings;
use crate::explorer::{self, Entry};
use crate::layout::SidePanel;
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
            X::ShowHistory => self.explorer_file_history(),
            X::FindInFolder => self.find_in_folder(),
            X::CopyPath => self.explorer_copy_path(false),
            X::CopyRelativePath => self.explorer_copy_path(true),
            X::RevealInFinder => {
                if let Some(path) = self.explorer.selected.clone() {
                    self.reveal_in_finder(path);
                }
            }
            X::RevealActive => {
                if let Some(path) = self.active_path() {
                    self.focus_side(SidePanel::Explorer);
                    for dir in self.explorer.reveal(&path) {
                        self.request_listing(dir);
                    }
                }
            }
            X::CollapseAll => self.explorer.collapse_all(),
        }
    }

    /// "Explorer: Copy Path" / "Copy Relative Path": the selected row's path to the clipboard.
    fn explorer_copy_path(&mut self, relative: bool) {
        let Some(path) = self.explorer.selected.clone() else {
            return;
        };
        let text = if relative {
            relative_path(&self.workspace.root, &path)
        } else {
            path.display().to_string()
        };
        self.clipboard.write(Copied {
            text: text.clone(),
            whole_line: false,
            pieces: Vec::new(),
        });
        self.notify(Level::Info, format!("Copied path: {text}"));
    }

    /// Shows `path` in the system file manager, off the UI thread; a failure is reported.
    pub(super) fn reveal_in_finder(&mut self, path: PathBuf) {
        let (program, args) = reveal_command(&path, cfg!(target_os = "macos"));
        // Tests run every command (`Command::all()`): never launch the desktop's file manager there,
        // unless asked (`JED_TEST_REVEAL=1`, for `todo_tests::explorer_path_commands::reveal_opens_the_file_manager`).
        if cfg!(test) && std::env::var_os("JED_TEST_REVEAL").is_none() {
            tracing::debug!("reveal in file manager skipped in tests: {program} {args:?}");
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = std::process::Command::new(&program)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let err = match result {
                Ok(s) if s.success() => return,
                Ok(s) => format!("{program} exited with {s}"),
                Err(e) => format!("{program}: {e}"),
            };
            let _ = tx.send(AppEvent::RevealFailed(format!("Can't reveal in file manager: {err}")));
        });
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
            Some(_) => self.open_file(path.to_path_buf(), focus, None),
            None => {}
        }
    }

    pub(super) fn request_listing(&mut self, dir: PathBuf) {
        // Single file mode lists nothing (no explorer).
        if self.mode.is_single() || !self.listing.insert(dir.clone()) {
            return;
        }
        let excludes = self.excludes.clone();
        let tx = self.tx.clone();
        let ignored = self.explorer.find(&dir).is_some_and(|e| e.ignored);
        tokio::task::spawn_blocking(move || {
            let result = explorer::list_dir_in(&dir, &excludes, ignored);
            let _ = tx.send(AppEvent::Listed { dir, ignored, result });
        });
    }

    /// `ignored`: whether the folder was ignored when the listing was asked for. A parent's
    /// newer listing may have changed that since: an ignored folder's entries all are, and a
    /// folder no longer ignored is listed again.
    pub(super) fn listed(&mut self, dir: PathBuf, ignored: bool, result: std::io::Result<Vec<Entry>>) {
        self.listing.remove(&dir);
        match result {
            Ok(mut entries) => {
                let now = self.explorer.find(&dir).is_some_and(|e| e.ignored);
                if now {
                    entries.iter_mut().for_each(|e| e.ignored = true);
                }
                for next in self.explorer.set_children(&dir, entries) {
                    self.request_listing(next);
                }
                if ignored && !now {
                    self.request_listing(dir);
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
        // A renamed file's history moves with it (once the rename is done); a delete keeps it.
        let history = match &op {
            FileOp::Rename { from, to } => self.history_rename(from, to),
            _ => None,
        };
        tokio::task::spawn_blocking(move || {
            let result = match &op {
                FileOp::NewFile(p) => explorer::ops::new_file(p),
                FileOp::NewFolder(p) => explorer::ops::new_folder(p),
                FileOp::Rename { from, to } => explorer::ops::rename(from, to),
                FileOp::Delete(p) => explorer::ops::delete(p),
            };
            let renamed = result.is_ok();
            let _ = tx.send(AppEvent::FileOpDone {
                op,
                result: result.map_err(|e| e.to_string()),
            });
            if let Some(move_history) = history {
                super::history::send_done(&tx, if renamed { move_history() } else { Ok(()) });
            }
        });
    }

    pub(super) fn file_op_done(&mut self, op: FileOp, result: Result<(), String>) {
        tracing::info!(?op, ?result, "file operation");
        if let Err(e) = result {
            self.notify(Level::Error, e);
            return;
        }
        let refresh = |app: &mut Self, p: &Path| app.refresh_listings_above(p);
        match op {
            FileOp::NewFile(path) => {
                refresh(self, &path);
                self.reveal_path(&path);
                self.open_file(path, true, None);
            }
            FileOp::NewFolder(path) => {
                refresh(self, &path);
                self.reveal_path(&path);
            }
            FileOp::Rename { from, to } => {
                let moved = |path: &Path| {
                    let rest = path.strip_prefix(&from).ok()?;
                    Some(if rest.as_os_str().is_empty() {
                        to.clone()
                    } else {
                        to.join(rest)
                    })
                };
                let mut ids = Vec::new();
                for (id, buf) in self.buffers.iter_mut() {
                    if let Some(path) = moved(&buf.path) {
                        buf.path = path;
                        ids.push(id);
                    }
                }
                for id in ids {
                    self.forget_path_state(id);
                }
                for h in self.groups.hex_tabs_mut() {
                    if let Some(path) = moved(&h.path) {
                        h.path = path;
                    }
                }
                self.redetect_languages();
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
                for h in self.groups.hex_tabs_mut() {
                    if h.path.starts_with(&path) {
                        h.deleted = true;
                    }
                }
                refresh(self, &path);
                self.notify(Level::Info, format!("Moved {} to the Trash", path.display()));
            }
        }
    }

    /// Re-lists every loaded folder above `p`, so new subfolders appear too.
    pub(super) fn refresh_listings_above(&mut self, p: &Path) {
        let dirs: Vec<PathBuf> = p
            .ancestors()
            .skip(1)
            .filter(|d| self.explorer.is_loaded(d))
            .map(Path::to_path_buf)
            .collect();
        for dir in dirs {
            self.request_listing(dir);
        }
    }

    pub(super) fn reveal_path(&mut self, path: &Path) {
        for dir in self.explorer.reveal(path) {
            self.request_listing(dir);
        }
    }

    /// Batched file system events: refresh loaded directories, the git status, open files changed
    /// on disk, reload `.workspace` and the user's `settings.toml` / `keybindings.toml`.
    pub(super) fn flush_fs_changes(&mut self) {
        self.fs_deadline = None;
        let config = self.workspace.config_path();
        let settings_path = self.config_dir.join(settings::FILE_NAME);
        let keymap_path = self.config_dir.join(keymap::FILE_NAME);
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut reload = false;
        let mut reload_settings = false;
        let mut reload_keymap = false;
        let paths: Vec<PathBuf> = std::mem::take(&mut self.fs_pending).into_iter().collect();
        #[cfg(test)]
        self.fs_flushed.extend(paths.iter().cloned());
        self.git_paths_changed(&paths);
        self.disk_paths_changed(&paths);
        self.diffs_paths_changed(&paths);
        let exclude_file = self.workspace.root.join(".git/info/exclude");
        for p in paths {
            // Single file mode watches only its file: no live `.workspace` reload (deferred).
            reload |= p == config && !self.mode.is_single();
            reload_settings |= p == settings_path;
            reload_keymap |= p == keymap_path;
            // An ignore file changed: what is ignored below its folder may have too.
            let ignores = if p == exclude_file {
                Some(self.workspace.root.as_path())
            } else if p.file_name().is_some_and(|n| n == ".gitignore" || n == ".ignore") {
                p.parent()
            } else {
                None
            };
            if let Some(top) = ignores {
                for d in self.explorer.loaded_dirs() {
                    if d.starts_with(top) && !dirs.contains(&d) {
                        dirs.push(d);
                    }
                }
            }
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
        if reload_settings {
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(AppEvent::SettingsLoaded(settings::reload(&settings_path)));
            });
        }
        if reload_keymap {
            let tx = self.tx.clone();
            let platform = Platform::current();
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(AppEvent::KeymapLoaded(Keymap::reload(&keymap_path, platform)));
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
        let associations_changed = config.files.associations != self.workspace.config.files.associations;
        self.workspace.config = config;
        self.settings = self.workspace.settings();
        // `[window] title`: re-sent as OSC 2 (no title-stack push) on the next run turn. The
        // status bar's colour is read from the config when it draws.
        let title = self.workspace.config.window.title.clone();
        if title.as_deref() != self.identity.title_override() {
            self.identity.set_title(title.as_deref());
            self.title_dirty = true;
        }

        if excludes_changed {
            self.excludes = explorer::excludes(&self.workspace.root, &self.workspace.config.files.exclude);
            for d in self.explorer.loaded_dirs() {
                self.request_listing(d);
            }
        }
        self.trust = self.trust_store.evaluate(&self.workspace.root, &self.workspace.config);
        self.servers_dirty = true;
        self.notify(Level::Info, format!("Reloaded {}", workspace::FILE_NAME));
        if associations_changed {
            self.load_associations();
            self.redetect_languages();
        }
        if self.trust == Trust::Undecided {
            self.ask_trust();
        }
        if self.group().tabs.is_empty() && self.ui.focus != Focus::SidePanel {
            self.ui.focus = Focus::SidePanel;
        }
    }
}

/// `path` below `root`, `/`-separated; `.` for the root itself.
fn relative_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect();
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

/// The program and arguments that show `path` in the file manager: macOS `open -R <path>`,
/// elsewhere `xdg-open <parent>`.
fn reveal_command(path: &Path, macos: bool) -> (String, Vec<String>) {
    if macos {
        ("open".into(), vec!["-R".into(), path.display().to_string()])
    } else {
        let parent = path.parent().unwrap_or(path);
        ("xdg-open".into(), vec![parent.display().to_string()])
    }
}

#[cfg(test)]
mod reveal_tests {
    use super::*;

    #[test]
    fn reveal_command_per_platform() {
        let p = Path::new("/w/src/a.rs");
        assert_eq!(
            reveal_command(p, true),
            ("open".into(), vec!["-R".into(), "/w/src/a.rs".into()])
        );
        assert_eq!(reveal_command(p, false), ("xdg-open".into(), vec!["/w/src".into()]));
    }

    #[test]
    fn relative_path_uses_slashes() {
        assert_eq!(relative_path(Path::new("/w"), Path::new("/w/src/a.rs")), "src/a.rs");
        assert_eq!(relative_path(Path::new("/w"), Path::new("/w")), ".");
    }

    #[test]
    fn reveal_failed_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = workspace::normalize(dir.path());
        let mut app = App::new(
            workspace::Workspace::load(root.clone()),
            crate::workspace::trust::TrustStore::load(root.join("trust.toml")),
            None,
            true,
        );
        app.handle_app_event(AppEvent::RevealFailed("Can't reveal in file manager: boom".into()));
        let m = app.message.as_ref().unwrap();
        assert_eq!(
            (m.text.as_str(), m.level),
            ("Can't reveal in file manager: boom", Level::Warning)
        );
    }
}
