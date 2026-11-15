//! Per-user directories, and `file:line:col` positions: `jtui::paths` with jed's directory name
//! (`APP`) filled in.

use std::path::PathBuf;

pub use jtui::paths::{config_home, home, strip_position, terminal_link};

/// `$XDG_STATE_HOME/jed`, defaulting to `~/.local/state/jed`.
pub fn state_dir() -> PathBuf {
    jtui::paths::state_dir(crate::APP)
}

/// `$XDG_DATA_HOME/jed`, defaulting to `~/.local/share/jed`: files jed ships and writes out
/// (the bundled dictionary and the notices), not state to keep.
pub fn data_dir() -> PathBuf {
    jtui::paths::data_dir(crate::APP)
}

/// `$XDG_CONFIG_HOME/jed`, defaulting to `~/.config/jed` (on macOS too).
pub fn config_dir() -> PathBuf {
    jtui::paths::config_dir(crate::APP)
}
