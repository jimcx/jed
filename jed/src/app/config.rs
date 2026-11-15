//! Config results from background work (`AppEvent::Config`): re-reads of the workspace config,
//! `settings.toml` and `keybindings.toml` (`config_loaded` in `side.rs`, `settings_loaded` and
//! `keymap_loaded` in `mod.rs`) and the files made for editing them.

use std::path::PathBuf;

use super::{App, Level};
use crate::config::keymap::Keymap;
use crate::config::settings::UserSettings;
use crate::workspace::config::WorkspaceConfig;

pub(super) enum ConfigEvent {
    /// The workspace config (`.workspace`) was re-read, or why it couldn't be.
    Loaded(Result<WorkspaceConfig, String>),
    /// `settings.toml` was re-read (it changed on disk), or why it couldn't be (the last good
    /// settings stay).
    SettingsLoaded(Result<UserSettings, String>),
    /// `keybindings.toml` was re-read (it changed on disk) and a message per problem, or why it
    /// couldn't be (the last good keymap stays).
    KeymapLoaded(Result<(Keymap, Vec<String>), String>),
    /// `settings.toml` exists (created from the template if it didn't), or why it couldn't be.
    UserSettingsReady(Result<PathBuf, String>),
    /// `.workspace` exists (created from the template if it didn't), or why it couldn't be.
    WorkspaceSettingsReady(Result<PathBuf, String>),
    /// `keybindings.toml` exists with the Keyboard Shortcuts view's stub appended, or why not.
    KeybindingsReady(Result<PathBuf, String>),
}

impl App {
    pub(super) fn handle_config_event(&mut self, ev: ConfigEvent) {
        match ev {
            ConfigEvent::Loaded(result) => self.config_loaded(result),
            ConfigEvent::SettingsLoaded(result) => self.settings_loaded(result),
            ConfigEvent::KeymapLoaded(result) => self.keymap_loaded(result),
            ConfigEvent::UserSettingsReady(result) => match result {
                Ok(path) => {
                    self.watch_config_dir();
                    self.open_file(path, true, None);
                }
                Err(e) => self.notify(Level::Error, e),
            },
            ConfigEvent::WorkspaceSettingsReady(result) => match result {
                Ok(path) => self.open_file(path, true, None),
                Err(e) => self.notify(Level::Error, e),
            },
            ConfigEvent::KeybindingsReady(result) => match result {
                Ok(path) => {
                    self.watch_config_dir();
                    self.notify(Level::Info, "Keybinding written — reloaded live".into());
                    self.open_file(path, true, None);
                }
                Err(e) => self.notify(Level::Error, e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::app::AppEvent;
    use crate::buffer::Buffer;
    use crate::command::{Command, Motion};
    use crate::config::keymap;
    use crossterm::event::KeyCode as K;

    fn app_in(dir: &tempfile::TempDir) -> App {
        let root = crate::workspace::normalize(&dir.path().join("ws"));
        fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt");
        fs::write(&a, "abc\n").unwrap();
        crate::milestone_tests::test_app(&root, Buffer::open(&a).unwrap())
    }

    /// A failed `.workspace` creation is reported at Error level.
    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_settings_ready_error_is_an_error_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        app.test_start();
        app.handle_app_event(AppEvent::Config(ConfigEvent::WorkspaceSettingsReady(Err(
            "Can't create .workspace: boom".into(),
        ))));
        let m = app.message.as_ref().unwrap();
        assert_eq!(
            (m.text.as_str(), m.level),
            ("Can't create .workspace: boom", Level::Error)
        );
    }

    /// `KeybindingsReady` starts watching a config dir that didn't exist at start: a later edit of
    /// `keybindings.toml` reloads live.
    #[tokio::test(flavor = "multi_thread")]
    async fn keybindings_ready_watches_the_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let config_dir = crate::workspace::normalize(dir.path()).join("config");
        app.config_dir = config_dir.clone();
        app.enable_file_watching();
        app.test_start();
        fs::create_dir_all(&config_dir).unwrap();
        let path = config_dir.join(keymap::FILE_NAME);
        fs::write(&path, "").unwrap();
        app.handle_app_event(AppEvent::Config(ConfigEvent::KeybindingsReady(Ok(path.clone()))));
        fs::write(&path, "[[binding]]\nkey = \"ctrl+alt+h\"\ncommand = \"cursor.home\"\n").unwrap();
        let bound = |a: &App| {
            a.keymap
                .binding_for(Command::Move {
                    motion: Motion::LineStart,
                    select: false,
                })
                .is_some_and(|k| k.code == K::Char('h'))
        };
        assert!(
            app.settle(bound).await,
            "keybindings.toml reloaded after the dir was watched"
        );
    }
}
