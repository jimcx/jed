//! Per-user directories, and `file:line:col` positions.

use std::path::PathBuf;

/// `$XDG_STATE_HOME/jed`, defaulting to `~/.local/state/jed`.
pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

/// `$XDG_CONFIG_HOME/jed`, defaulting to `~/.config/jed` (on macOS too).
pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_CONFIG_HOME`, defaulting to `~/.config` (on macOS too): the folder other programs keep
/// their config in (fish's `fish/config.fish`).
pub fn config_home() -> PathBuf {
    xdg_base("XDG_CONFIG_HOME", ".config")
}

fn xdg_dir(var: &str, default: &str) -> PathBuf {
    xdg_base(var, default).join("jed")
}

fn xdg_base(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(default))
}

/// `$HOME`, else `.`.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `name:12:5` → (`name`, (12, 5)); `name:12` → (`name`, (12, 1)). 1-based; `None` without a position.
/// Used for command-line arguments and quick open.
pub fn strip_position(arg: &str) -> Option<(&str, (usize, usize))> {
    let num = |s: &str| s.parse::<usize>().ok().filter(|n| *n > 0);
    let (rest, last) = arg.rsplit_once(':')?;
    let last = num(last)?;
    match rest.rsplit_once(':') {
        Some((path, line)) if !path.is_empty() && num(line).is_some() => Some((path, (num(line)?, last))),
        _ if !rest.is_empty() => Some((rest, (last, 1))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions() {
        assert_eq!(strip_position("a.rs:12:5"), Some(("a.rs", (12, 5))));
        assert_eq!(strip_position("a.rs:12"), Some(("a.rs", (12, 1))));
        assert_eq!(strip_position("a:b.rs:3"), Some(("a:b.rs", (3, 1))));
        assert_eq!(strip_position("a.rs"), None);
        assert_eq!(strip_position("a.rs:x"), None);
        assert_eq!(strip_position("a.rs:0"), None);
        assert_eq!(strip_position(":3"), None);
        assert_eq!(strip_position("a.rs:"), None);
    }
}
