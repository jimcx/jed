//! Settings and keybindings: defaults plus the user's `~/.config/jed/settings.toml` and
//! `keybindings.toml` (`paths::config_dir()`), both loaded at startup and reloaded live.

pub mod complete;
pub mod keymap;
pub mod schema;
pub mod settings;
