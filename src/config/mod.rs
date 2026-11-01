//! Settings and keybindings: defaults plus the user's `~/.config/jed/settings.toml` and
//! `keybindings.toml` (`paths::config_dir()`), both loaded at startup and reloaded live.

pub mod keymap;
#[allow(dead_code)] // the highlighter and completer (settings-e, settings-f) are its readers
pub mod schema;
pub mod settings;
