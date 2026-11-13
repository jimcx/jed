//! Opt-in shell integration (`jterm::shell_integration`: the zsh / bash / fish snippets and the
//! line that sources one), re-exported, with jed's `NAMES` (`jed`, `JED_IPC`, `_jed`) applied by
//! `snippets`, `status` and `write_snippets`, which keep their jed signatures here.
//! "Terminal: Install Shell Integration" (`app/shell.rs`) opens the wizard with this data and
//! `app/prompt.rs` copies or appends the line; `ui/dialog.rs` draws it.

use std::path::Path;

pub use jterm::shell_integration::{Names, Status, append_line, init_for};

/// What jed's snippets name.
pub const NAMES: Names = Names {
    app: crate::APP,
    guard: crate::ipc::ENV,
    prefix: "_jed",
};

/// The three snippets, jed's (tests compare them).
#[cfg(test)]
pub fn snippets() -> [jterm::shell_integration::Snippet; 3] {
    jterm::shell_integration::snippets(&NAMES)
}

/// Whether `shell_base`'s integration is installed and current (`jterm::shell_integration::status`).
///
/// Blocking: call from `spawn_blocking`.
pub fn status(home: &Path, config_home: &Path, state_dir: &Path, shell_base: &str) -> Option<Status> {
    jterm::shell_integration::status(&NAMES, home, config_home, state_dir, shell_base)
}

/// Writes jed's three snippets into `state_dir`.
///
/// Blocking: call from `spawn_blocking`.
pub fn write_snippets(state_dir: &Path) -> Result<(), String> {
    jterm::shell_integration::write_snippets(&NAMES, state_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// jed's names render the snippets exactly as they were before they became templates.
    #[test]
    fn jeds_snippets_are_byte_identical_to_the_fixtures() {
        let fixtures = [
            include_str!("testdata/shell-integration.zsh"),
            include_str!("testdata/shell-integration.bash"),
            include_str!("testdata/shell-integration.fish"),
        ];
        let snippets = snippets();
        assert_eq!(snippets.len(), fixtures.len());
        for (snippet, fixture) in snippets.iter().zip(fixtures) {
            assert_eq!(snippet.text, fixture, "{}", snippet.file);
        }
    }

    #[test]
    fn written_files_are_jeds_snippets() {
        let dir = tempfile::tempdir().unwrap();
        write_snippets(dir.path()).unwrap();
        for snippet in snippets() {
            assert_eq!(
                std::fs::read_to_string(dir.path().join(snippet.file)).unwrap(),
                snippet.text
            );
        }
    }
}
