//! Settings and keybindings: defaults plus the user's `~/.config/jed/settings.toml` and
//! `keybindings.toml` (`paths::config_dir()`), both loaded at startup and reloaded live.

pub mod keymap;
#[allow(dead_code)] // the completer (settings-f) is the reader of `candidates` and the key lists
pub mod schema;
pub mod settings;
