//! Per-user directories (named by the app), and `file:line:col` positions.

use std::path::{Path, PathBuf};

/// `$XDG_STATE_HOME/<app>`, defaulting to `~/.local/state/<app>`.
pub fn state_dir(app: &str) -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state", app)
}

/// `$XDG_DATA_HOME/<app>`, defaulting to `~/.local/share/<app>`: files the app ships and writes
/// out (a bundled dictionary, notices), not state to keep.
pub fn data_dir(app: &str) -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share", app)
}

/// `$XDG_CONFIG_HOME/<app>`, defaulting to `~/.config/<app>` (on macOS too).
pub fn config_dir(app: &str) -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config", app)
}

/// `$XDG_CONFIG_HOME`, defaulting to `~/.config` (on macOS too): the folder other programs keep
/// their config in (fish's `fish/config.fish`).
pub fn config_home() -> PathBuf {
    xdg_base("XDG_CONFIG_HOME", ".config")
}

fn xdg_dir(var: &str, default: &str, app: &str) -> PathBuf {
    xdg_base(var, default).join(app)
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

/// `path` with `home` shown as `~` (`~` itself, `~/rest`); other paths as they are.
pub fn display(path: &Path, home: &Path) -> String {
    if home.as_os_str().is_empty() || home == Path::new("/") {
        return path.display().to_string();
    }
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
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

/// A terminal word read as a file link: `src/a.rs:12:5` → (`src/a.rs`, Some((12, 5))). Trailing
/// `:` / `.` (a sentence's or a compiler's punctuation) are dropped. `None` unless it looks like
/// a path (has a `/` or a `.`), so a plain word or a time like `10:30` isn't one.
pub fn terminal_link(word: &str) -> Option<(&str, Option<(usize, usize)>)> {
    let word = word.trim_end_matches([':', '.']);
    let (path, pos) = match strip_position(word) {
        Some((path, pos)) => (path, Some(pos)),
        None => (word, None),
    };
    (path.contains(['/', '.']) && path.chars().any(|c| c.is_ascii_alphanumeric())).then_some((path, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_dirs_are_named_by_the_caller() {
        for (dir, name) in [(state_dir("x"), "x"), (data_dir("y"), "y"), (config_dir("z"), "z")] {
            assert_eq!(dir.file_name().unwrap(), name, "{dir:?}");
            assert!(!dir.to_string_lossy().contains("jed"), "{dir:?}");
        }
    }

    #[test]
    fn display_abbreviates_home() {
        let home = Path::new("/home/a");
        assert_eq!(display(Path::new("/home/a"), home), "~");
        assert_eq!(display(Path::new("/home/a/b"), home), "~/b");
        assert_eq!(display(Path::new("/etc"), home), "/etc");
        assert_eq!(display(Path::new("/etc"), Path::new("/")), "/etc");
    }

    #[test]
    fn terminal_links() {
        assert_eq!(terminal_link("src/a.rs:12:5"), Some(("src/a.rs", Some((12, 5)))));
        assert_eq!(terminal_link("src/a.rs:12:"), Some(("src/a.rs", Some((12, 1)))));
        assert_eq!(terminal_link("./a.rs."), Some(("./a.rs", None)));
        assert_eq!(terminal_link("Makefile"), None);
        assert_eq!(terminal_link("10:30"), None);
        assert_eq!(terminal_link("..."), None);
    }

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
