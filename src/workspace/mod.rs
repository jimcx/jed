//! Workspace: root detection, `.workspace` loading, trust.

pub mod config;
pub mod trust;

use std::path::{Path, PathBuf};

use config::{SettingsOverrides, WorkspaceConfig};

use crate::edit::Settings;

pub const FILE_NAME: &str = ".workspace";

#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub config: WorkspaceConfig,
    /// Parse error from the last load; `config` keeps the last good value.
    pub config_error: Option<String>,
    /// The user's `settings.toml`, which `.workspace` `[settings]` override key by key. Loaded
    /// once at startup (`with_user_settings`); `.workspace` reloads keep it.
    pub user_settings: SettingsOverrides,
}

impl Workspace {
    /// Blocking. A missing `.workspace` is an empty config.
    pub fn load(root: PathBuf) -> Self {
        let mut ws = Self {
            root,
            config: WorkspaceConfig::default(),
            config_error: None,
            user_settings: SettingsOverrides::default(),
        };
        ws.reload();
        ws
    }

    /// Layers `settings.toml`'s values under `.workspace`'s.
    pub fn with_user_settings(mut self, user: SettingsOverrides) -> Self {
        self.user_settings = user;
        self
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(FILE_NAME)
    }

    /// Blocking. Re-reads `.workspace`; on a parse error keeps the previous config.
    pub fn reload(&mut self) {
        match read_config(&self.config_path()) {
            Ok(c) => {
                self.config = c;
                self.config_error = None;
            }
            Err(e) => self.config_error = Some(e),
        }
    }

    /// Defaults ← user `settings.toml` ← `.workspace` `[settings]`, per key.
    pub fn settings(&self) -> Settings {
        crate::config::settings::layer(&self.user_settings, &self.config.settings)
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map_or_else(|| self.root.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

pub fn read_config(path: &Path) -> Result<WorkspaceConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => WorkspaceConfig::parse(&text).map_err(|e| format!("{FILE_NAME}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(WorkspaceConfig::default()),
        Err(e) => Err(format!("{FILE_NAME}: {e}")),
    }
}

/// Absolute, with symlinks resolved when the path exists.
pub fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// For `jed <file>`: the nearest ancestor with a `.workspace`, else the file's directory.
pub fn root_for_file(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("/"));
    dir.ancestors()
        .find(|d| d.join(FILE_NAME).is_file())
        .unwrap_or(dir)
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_detection_walks_up() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        let deep = root.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(root_for_file(&deep.join("f.txt")), deep);
        std::fs::write(root.join(FILE_NAME), "").unwrap();
        assert_eq!(root_for_file(&deep.join("f.txt")), root);
    }

    #[test]
    fn bad_config_keeps_last_good() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        std::fs::write(root.join(FILE_NAME), "[settings]\ntab_width = 2").unwrap();
        let mut ws = Workspace::load(root.clone());
        assert_eq!(ws.settings().tab_width, 2);
        assert!(ws.settings().auto_close_pairs, "on by default");
        std::fs::write(root.join(FILE_NAME), "[settings]\nauto_close_pairs = false").unwrap();
        ws.reload();
        assert!(!ws.settings().auto_close_pairs);
        std::fs::write(root.join(FILE_NAME), "[settings]\ntab_width = 2").unwrap();
        ws.reload();
        std::fs::write(root.join(FILE_NAME), "[settings\n").unwrap();
        ws.reload();
        assert!(ws.config_error.is_some());
        assert_eq!(ws.settings().tab_width, 2);
    }

    #[test]
    fn reload_keeps_user_settings_under_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        let user = SettingsOverrides {
            tab_width: Some(2),
            auto_close_pairs: Some(false),
            ..Default::default()
        };
        let mut ws = Workspace::load(root.clone()).with_user_settings(user);
        assert_eq!(ws.settings().tab_width, 2);
        assert!(!ws.settings().auto_close_pairs);
        std::fs::write(root.join(FILE_NAME), "[settings]\nauto_close_pairs = true").unwrap();
        ws.reload();
        assert!(ws.settings().auto_close_pairs, "the workspace wins");
        assert_eq!(
            ws.settings().tab_width,
            2,
            "the user's value for a key the workspace doesn't set"
        );
    }
}
