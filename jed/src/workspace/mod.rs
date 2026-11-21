//! Workspace: root detection, `.workspace` loading, trust.

pub mod config;
pub mod trust;

use std::path::{Path, PathBuf};

use config::{SettingsOverrides, StatusBarColor, WindowConfig, WorkspaceConfig};

use crate::edit::Settings;

pub const FILE_NAME: &str = ".workspace";

/// `[lsp] idle_stop_minutes` when neither file sets it.
pub const DEFAULT_LSP_IDLE_STOP_MINUTES: u64 = 60;

#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub config: WorkspaceConfig,
    /// Parse error from the last load; `config` keeps the last good value.
    pub config_error: Option<String>,
    /// The user's `settings.toml`, which `.workspace` `[settings]` override key by key. Loaded at
    /// startup (`with_user_settings`) and replaced on a live reload; `.workspace` reloads keep it.
    pub user_settings: SettingsOverrides,
    /// The user's `settings.toml` `[window]`, under `.workspace`'s (same lifecycle as `user_settings`).
    pub user_window: WindowConfig,
    /// The user's `settings.toml` `[lsp] idle_stop_minutes`, under `.workspace`'s (`lsp_idle_stop`).
    pub user_lsp_idle_stop_minutes: Option<u64>,
}

impl Workspace {
    /// Blocking. A missing `.workspace` is an empty config.
    pub fn load(root: PathBuf) -> Self {
        let mut ws = Self {
            root,
            config: WorkspaceConfig::default(),
            config_error: None,
            user_settings: SettingsOverrides::default(),
            user_window: WindowConfig::default(),
            user_lsp_idle_stop_minutes: None,
        };
        ws.reload();
        ws
    }

    /// Layers `settings.toml`'s values under `.workspace`'s.
    pub fn with_user_settings(mut self, user: SettingsOverrides) -> Self {
        self.user_settings = user;
        self
    }

    /// Layers `settings.toml`'s `[window]` under `.workspace`'s.
    pub fn with_user_window(mut self, window: WindowConfig) -> Self {
        self.user_window = window;
        self
    }

    /// Layers `settings.toml`'s `[lsp] idle_stop_minutes` under `.workspace`'s.
    pub fn with_user_lsp_idle_stop(mut self, minutes: Option<u64>) -> Self {
        self.user_lsp_idle_stop_minutes = minutes;
        self
    }

    /// `[lsp] idle_stop_minutes`: `.workspace`'s, else `settings.toml`'s, else 60; `None` for 0
    /// (never stop). Read live, so a reload of either file applies.
    pub fn lsp_idle_stop(&self) -> Option<std::time::Duration> {
        let minutes = self
            .config
            .lsp
            .idle_stop_minutes
            .or(self.user_lsp_idle_stop_minutes)
            .unwrap_or(DEFAULT_LSP_IDLE_STOP_MINUTES);
        (minutes > 0).then(|| std::time::Duration::from_secs(minutes.saturating_mul(60)))
    }

    /// `[window] title`: `.workspace`'s, else `settings.toml`'s.
    pub fn window_title(&self) -> Option<String> {
        self.user_window.layered(&self.config.window).title
    }

    /// The layered `[window]` (`.workspace`'s keys over `settings.toml`'s).
    pub fn window(&self) -> WindowConfig {
        self.user_window.layered(&self.config.window)
    }

    /// `[window] status_bar`: `.workspace`'s, else `settings.toml`'s, else blue.
    pub fn status_bar(&self) -> StatusBarColor {
        self.user_window
            .layered(&self.config.window)
            .status_bar
            .unwrap_or_default()
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
        #[allow(unused_mut)]
        let mut settings = crate::config::settings::layer(&self.user_settings, &self.config.settings);
        // Off in tests unless a file sets it: spelling marks would show up in every other feature's
        // hovers and squiggles. `spell_check_tests` turns it on.
        #[cfg(test)]
        if self
            .user_settings
            .spell_check_scope
            .or(self.config.settings.spell_check_scope)
            .is_none()
        {
            settings.spell_check_scope = crate::edit::SpellScope::Off;
        }
        settings
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
    fn lsp_idle_stop_layers_workspace_over_user_over_default() {
        use std::time::Duration;
        let min = |m: u64| Some(Duration::from_secs(m * 60));
        let dir = tempfile::tempdir().unwrap();
        let root = normalize(dir.path());
        let mut ws = Workspace::load(root.clone());
        assert_eq!(ws.lsp_idle_stop(), min(60), "default");
        ws.user_lsp_idle_stop_minutes = Some(10);
        assert_eq!(ws.lsp_idle_stop(), min(10), "settings.toml");
        ws.user_lsp_idle_stop_minutes = Some(1);
        assert_eq!(ws.lsp_idle_stop(), min(1), "1 is a minute, not never");
        ws.user_lsp_idle_stop_minutes = Some(0);
        assert_eq!(ws.lsp_idle_stop(), None, "0 = never");
        std::fs::write(root.join(FILE_NAME), "[lsp]\nidle_stop_minutes = 5\n").unwrap();
        ws.reload();
        assert_eq!(ws.lsp_idle_stop(), min(5), ".workspace beats settings.toml");
        std::fs::write(root.join(FILE_NAME), "[lsp]\nidle_stop_minutes = 0\n").unwrap();
        ws.reload();
        ws.user_lsp_idle_stop_minutes = Some(10);
        assert_eq!(ws.lsp_idle_stop(), None, ".workspace 0 beats a user value");
        std::fs::write(root.join(FILE_NAME), "[lsp]\nidle_stop_minutes = \"x\"\n").unwrap();
        ws.reload();
        assert!(ws.config_error.is_some());
        assert_eq!(ws.lsp_idle_stop(), None, "the last good config stays");
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
