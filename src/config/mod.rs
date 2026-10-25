//! Settings and keybindings: defaults plus the user's `~/.config/jed/settings.toml` and
//! `keybindings.toml` (`paths::config_dir()`), both loaded at startup and reloaded live.

pub mod keymap;
pub mod settings;
