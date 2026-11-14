//! `~/.config/jmux/settings.toml`: a flat file of `key = value` lines, read leniently. A key that
//! is unknown, badly typed or out of range is reported with its line and skipped; the rest applies
//! (the same rule as jed's `settings.toml`). A TOML syntax error makes the whole file unusable:
//! one error, defaults. The server reads it before its first shell and on `Command::ReloadSettings`
//! (a reload affects the next terminal, not the running ones); the client reads `cursor_style` when
//! it attaches.

use std::path::{Path, PathBuf};

use jtui::terminal::CursorStyle;

pub const FILE_NAME: &str = "settings.toml";

/// The default `command_history`.
pub const COMMAND_HISTORY: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The shell to run; `None`: `$SHELL`, else `/bin/sh`.
    pub shell: Option<PathBuf>,
    /// Lines of output a terminal keeps above its screen.
    pub scrollback: usize,
    /// Keep old scrollback lines zstd-compressed.
    pub scrollback_compression: bool,
    /// The most commands the side bar's history keeps.
    pub command_history: usize,
    /// The side bar's width in columns.
    pub side_bar_width: u16,
    pub cursor_style: CursorStyle,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            shell: None,
            scrollback: jterm::run::SCROLLBACK,
            scrollback_compression: false,
            command_history: COMMAND_HISTORY,
            side_bar_width: crate::side::DEFAULT_WIDTH,
            cursor_style: CursorStyle::Block,
        }
    }
}

impl Settings {
    /// What a new terminal's scrollback is made of.
    pub fn scrollback(&self) -> jterm::run::Scrollback {
        jterm::run::Scrollback {
            lines: self.scrollback,
            compress: self.scrollback_compression,
        }
    }
}

/// The settings file under `config_dir` (`jtui::paths::config_dir(APP)`).
pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE_NAME)
}

/// Blocking: call from spawn_blocking (or before the runtime starts). A missing file is the
/// defaults and no error; an unreadable one is the defaults and one error.
pub fn load(path: &Path) -> (Settings, Vec<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), Vec::new()),
        Err(e) => (Settings::default(), vec![format!("{FILE_NAME}: {e}")]),
    }
}

/// Parses the file's text: the settings it gives, and one message per problem.
pub fn parse(text: &str) -> (Settings, Vec<String>) {
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => {
            let at = e.span().map_or(0, |s| s.start.min(text.len()));
            let line = text[..at].matches('\n').count() + 1;
            return (
                Settings::default(),
                vec![format!("{FILE_NAME}: line {line}: {}", e.message().trim_end())],
            );
        }
    };
    let mut settings = Settings::default();
    let mut errors = Vec::new();
    for (key, value) in table {
        if let Err(why) = apply(&mut settings, &key, &value) {
            errors.push((
                line_of(text, &key),
                format!("{FILE_NAME}: line {}: {why}", line_of(text, &key)),
            ));
        }
    }
    errors.sort();
    (settings, errors.into_iter().map(|(_, e)| e).collect())
}

fn apply(s: &mut Settings, key: &str, value: &toml::Value) -> Result<(), String> {
    let bad = |want: &str| format!("{key} must be {want}");
    let count = |max: u64| match value.as_integer() {
        Some(n) if n >= 0 && n as u64 <= max => Ok(n as u64),
        Some(_) => Err(bad(&format!("between 0 and {max}"))),
        None => Err(bad("a whole number")),
    };
    match key {
        "shell" => {
            let path = value.as_str().ok_or_else(|| bad("a string"))?;
            if path.is_empty() {
                return Err(bad("not empty"));
            }
            s.shell = Some(PathBuf::from(path));
        }
        "scrollback" => s.scrollback = count(10_000_000)? as usize,
        "scrollback_compression" => s.scrollback_compression = value.as_bool().ok_or_else(|| bad("true or false"))?,
        "command_history" => s.command_history = count(100_000)? as usize,
        "side_bar_width" => s.side_bar_width = count(500)?.max(1) as u16,
        "cursor_style" => {
            s.cursor_style = match value.as_str() {
                Some("block") => CursorStyle::Block,
                Some("line") => CursorStyle::Line,
                Some("underline") => CursorStyle::Underline,
                _ => return Err(bad("\"block\", \"line\" or \"underline\"")),
            }
        }
        _ => return Err(format!("unknown setting {key}")),
    }
    Ok(())
}

/// The 1-based line `key` is set on (the first line starting with it, then `=`); 1 if not found.
fn line_of(text: &str, key: &str) -> usize {
    let starts = |l: &str| {
        let l = l.trim_start().trim_start_matches(['"', '\'']);
        l.strip_prefix(key)
            .is_some_and(|r| r.trim_start_matches(['"', '\'']).trim_start().starts_with('='))
    };
    text.lines().position(starts).map_or(1, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_text_is_the_defaults() {
        assert_eq!(parse(""), (Settings::default(), Vec::new()));
        let d = Settings::default();
        assert_eq!((d.scrollback, d.command_history, d.side_bar_width), (5_000, 500, 32));
    }

    #[test]
    fn every_key_is_read() {
        let (s, errors) = parse(
            "shell = \"/bin/zsh\"\nscrollback = 100\nscrollback_compression = true\ncommand_history = 7\n\
             side_bar_width = 40\ncursor_style = \"underline\"\n",
        );
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(
            s,
            Settings {
                shell: Some("/bin/zsh".into()),
                scrollback: 100,
                scrollback_compression: true,
                command_history: 7,
                side_bar_width: 40,
                cursor_style: CursorStyle::Underline,
            }
        );
    }

    #[test]
    fn a_bad_key_is_reported_with_its_line_and_the_rest_applies() {
        let text = "scrollback = 10\nscrollback_compression = \"yes\"\ncommand_history = -1\nbogus = 1\n\
                    cursor_style = \"star\"\nshell = \"/bin/dash\"\n";
        let (s, errors) = parse(text);
        assert_eq!((s.scrollback, s.shell), (10, Some("/bin/dash".into())));
        assert!(!s.scrollback_compression && s.command_history == 500 && s.cursor_style == CursorStyle::Block);
        assert_eq!(errors.len(), 4, "{errors:?}");
        for (error, line) in errors.iter().zip([2, 3, 4, 5]) {
            assert!(error.starts_with(&format!("settings.toml: line {line}:")), "{errors:?}");
        }
        assert!(errors[2].contains("unknown setting bogus"), "{errors:?}");
    }

    #[test]
    fn a_syntax_error_reports_its_line_and_uses_the_defaults() {
        let (s, errors) = parse("scrollback = 10\nshell = = 3\n");
        assert_eq!(s, Settings::default());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("settings.toml: line 2:"), "{errors:?}");
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_no_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("none.toml")), (Settings::default(), Vec::new()));
    }
}
