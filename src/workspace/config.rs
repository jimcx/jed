//! The `.workspace` file: per-project settings, commands and file rules. Meant to be committed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `[settings]`: the same keys as the user's `settings.toml` (see `config::settings`).
pub use crate::config::settings::SettingsOverrides;

#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub settings: SettingsOverrides,
    pub files: FilesConfig,
    pub lsp: BTreeMap<String, LspServer>,
    pub tasks: BTreeMap<String, Task>,
    pub history: HistoryConfig,
    pub window: WindowConfig,
}

/// `[window]`: the workspace's name in the window title and the status bar's colour, so several
/// jed windows are told apart. Neither names a command, so neither needs trust.
#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    /// `jed <title>` instead of `jed <root>`; control characters are dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The status bar's background (`Theme::dark_plus` has `statusBar.<name>` for each).
    pub status_bar: StatusBarColor,
}

/// `[window] status_bar`'s colours, by their theme key's suffix.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusBarColor {
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    #[default]
    Blue,
    Purple,
    Magenta,
    Grey,
    /// A colour picked from a stable hash of the workspace root (`Theme::status_bar_auto`).
    Auto,
}

impl StatusBarColor {
    /// Every fixed colour (not `Auto`), for the tests that cover them all.
    #[cfg(test)]
    pub const ALL: [StatusBarColor; 9] = [
        StatusBarColor::Red,
        StatusBarColor::Orange,
        StatusBarColor::Yellow,
        StatusBarColor::Green,
        StatusBarColor::Cyan,
        StatusBarColor::Blue,
        StatusBarColor::Purple,
        StatusBarColor::Magenta,
        StatusBarColor::Grey,
    ];

    /// The config / theme key name (`red`, …), for the tests.
    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self {
            StatusBarColor::Red => "red",
            StatusBarColor::Orange => "orange",
            StatusBarColor::Yellow => "yellow",
            StatusBarColor::Green => "green",
            StatusBarColor::Cyan => "cyan",
            StatusBarColor::Blue => "blue",
            StatusBarColor::Purple => "purple",
            StatusBarColor::Magenta => "magenta",
            StatusBarColor::Grey => "grey",
            StatusBarColor::Auto => "auto",
        }
    }

    pub fn theme_key(self) -> &'static str {
        match self {
            StatusBarColor::Red => "statusBar.red",
            StatusBarColor::Orange => "statusBar.orange",
            StatusBarColor::Yellow => "statusBar.yellow",
            StatusBarColor::Green => "statusBar.green",
            StatusBarColor::Cyan => "statusBar.cyan",
            StatusBarColor::Blue => "statusBar.blue",
            StatusBarColor::Purple => "statusBar.purple",
            StatusBarColor::Magenta => "statusBar.magenta",
            StatusBarColor::Grey => "statusBar.grey",
            // Not a theme key: the bar asks `Theme::status_bar_auto`; blue is the fallback.
            StatusBarColor::Auto => "statusBar.blue",
        }
    }
}

/// `[history]`: file history limits (see `crate::history`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryConfig {
    /// Record a snapshot on every save.
    pub enabled: bool,
    /// Entries kept per file; the oldest are dropped.
    pub max_entries: usize,
    /// Files larger than this (bytes, as saved) aren't snapshotted.
    pub max_file_size: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 50,
            max_file_size: 5 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilesConfig {
    /// Globs hidden from the explorer, quick open and search, on top of `.gitignore`.
    pub exclude: Vec<String>,
    /// Glob → language, e.g. `"SConstruct" = "python"`.
    pub associations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LspServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_options: Option<toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The parts of the config that run programs; trust covers exactly these.
#[derive(Serialize)]
struct Commands<'a> {
    lsp: &'a BTreeMap<String, LspServer>,
    tasks: &'a BTreeMap<String, Task>,
}

impl WorkspaceConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn has_commands(&self) -> bool {
        !self.lsp.is_empty() || !self.tasks.is_empty()
    }

    /// One line per command, for the trust dialog.
    pub fn describe_commands(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (lang, s) in &self.lsp {
            out.push(format!("LSP {lang}: {}", join_command(&s.command, &s.args)));
        }
        for (name, t) in &self.tasks {
            let mut line = format!("Task {name}: {}", t.command);
            if let Some(cwd) = &t.cwd {
                line.push_str(&format!("  (cwd {cwd})"));
            }
            if !t.env.is_empty() {
                let env: Vec<String> = t.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
                line.push_str(&format!("  (env {})", env.join(" ")));
            }
            out.push(line);
        }
        out
    }

    /// SHA-256 of the command entries (BTreeMaps, so the encoding is stable).
    pub fn commands_hash(&self) -> String {
        let canonical = toml::to_string(&Commands {
            lsp: &self.lsp,
            tasks: &self.tasks,
        })
        .unwrap_or_default();
        Sha256::digest(canonical.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

fn join_command(command: &str, args: &[String]) -> String {
    std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[settings]
tab_width = 2
syntax_max_bytes = 2_000_000
auto_close_pairs = false

[files]
exclude = ["build/", "*.o"]
associations = { "SConstruct" = "python", "*.scons" = "python" }

[lsp.python]
command = "pyright-langserver"
args = ["--stdio"]

[tasks.build]
command = "scons -j8"
env = { CC = "clang" }
"#;

    #[test]
    fn parses_sample() {
        let c = WorkspaceConfig::parse(SAMPLE).unwrap();
        assert_eq!(c.settings.tab_width, Some(2));
        assert_eq!(c.settings.syntax_max_bytes, Some(2_000_000));
        assert_eq!(c.settings.auto_close_pairs, Some(false));
        assert_eq!(c.files.exclude, vec!["build/", "*.o"]);
        assert_eq!(c.files.associations["SConstruct"], "python");
        assert!(c.has_commands());
        assert_eq!(
            c.describe_commands(),
            vec![
                "LSP python: pyright-langserver --stdio",
                "Task build: scons -j8  (env CC=clang)"
            ]
        );
    }

    #[test]
    fn history_limits() {
        let c = WorkspaceConfig::parse("").unwrap();
        assert_eq!(c.history, HistoryConfig::default());
        assert!(c.history.enabled);
        assert_eq!((c.history.max_entries, c.history.max_file_size), (50, 5 * 1024 * 1024));
        let c = WorkspaceConfig::parse("[history]\nenabled = false\nmax_entries = 3").unwrap();
        assert_eq!(
            c.history,
            HistoryConfig {
                enabled: false,
                max_entries: 3,
                max_file_size: 5 * 1024 * 1024
            }
        );
        assert!(WorkspaceConfig::parse("[history]\nmax_entrys = 3").is_err());
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert!(WorkspaceConfig::parse("[settings]\ntab_widht = 2").is_err());
    }

    #[test]
    fn window_defaults_and_title() {
        let c = WorkspaceConfig::parse("").unwrap();
        assert_eq!(c.window.title, None);
        assert_eq!(c.window.status_bar, StatusBarColor::Blue);
        let c = WorkspaceConfig::parse("[window]\ntitle = \"api\"").unwrap();
        assert_eq!(c.window.title.as_deref(), Some("api"));
        assert_eq!(c.window.status_bar, StatusBarColor::Blue, "the title alone");
        assert!(WorkspaceConfig::parse("[window]\ntitel = \"api\"").is_err());
    }

    #[test]
    fn status_bar_auto_parses() {
        let c = WorkspaceConfig::parse("[window]\nstatus_bar = \"auto\"").unwrap();
        assert_eq!(c.window.status_bar, StatusBarColor::Auto);
    }

    #[test]
    fn every_status_bar_colour_parses() {
        for colour in StatusBarColor::ALL {
            let text = format!("[window]\nstatus_bar = \"{}\"", colour.name());
            let c = WorkspaceConfig::parse(&text).unwrap();
            assert_eq!(c.window.status_bar, colour, "{text}");
            assert_eq!(colour.theme_key(), format!("statusBar.{}", colour.name()));
        }
    }

    #[test]
    fn an_unknown_status_bar_colour_is_an_error() {
        let e = WorkspaceConfig::parse("[window]\nstatus_bar = \"pink\"").unwrap_err();
        assert!(e.contains("unknown variant"), "{e}");
        assert!(e.contains("pink"), "{e}");
    }

    #[test]
    fn hash_covers_commands_only() {
        let a = WorkspaceConfig::parse(SAMPLE).unwrap();
        let mut b = a.clone();
        b.settings.tab_width = Some(8);
        b.files.exclude.clear();
        assert_eq!(a.commands_hash(), b.commands_hash());
        b.tasks.get_mut("build").unwrap().env.insert("X".into(), "1".into());
        assert_ne!(a.commands_hash(), b.commands_hash());
    }
}
