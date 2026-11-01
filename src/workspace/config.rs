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
    /// The status bar's background (`Theme::dark_plus` has `statusBar.<name>` for each); unset is blue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_bar: Option<StatusBarColor>,
    /// A row at the top of the screen naming the window (`jed <root>` or `jed <title>`) with
    /// `buttons` at its right; unset is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_bar: Option<bool>,
    /// `[[window.buttons]]`: the title bar's buttons, left to right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<TitleButton>>,
}

/// What a title bar button does.
#[derive(Debug, Clone, PartialEq)]
pub enum ButtonAction {
    /// A palette command, by its name (`file.save`); checked against `Command::all()` when read.
    Command(crate::command::Command),
    /// A task template by name (`[tasks.<name>]`, `.workspace` or `tasks.toml`).
    Task(String),
}

/// `[[window.buttons]]`: exactly one of `command` / `task`, and an optional `label` replacing
/// the button's text (the command's title, or the task's name).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "RawButton", into = "RawButton")]
pub struct TitleButton {
    pub action: ButtonAction,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawButton {
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

impl TryFrom<RawButton> for TitleButton {
    type Error = String;

    fn try_from(raw: RawButton) -> Result<Self, String> {
        let action = match (raw.command, raw.task) {
            (Some(name), None) => ButtonAction::Command(
                crate::command::Command::from_name(&name).ok_or_else(|| format!("unknown command `{name}`"))?,
            ),
            (None, Some(task)) => ButtonAction::Task(task),
            _ => return Err("a button needs exactly one of `command` and `task`".into()),
        };
        Ok(TitleButton {
            action,
            label: raw.label,
        })
    }
}

impl From<TitleButton> for RawButton {
    fn from(b: TitleButton) -> Self {
        let (command, task) = match b.action {
            ButtonAction::Command(c) => (Some(c.name()), None),
            ButtonAction::Task(t) => (None, Some(t)),
        };
        RawButton {
            command,
            task,
            label: b.label,
        }
    }
}

impl TitleButton {
    /// The button's text: `label`, else the command's title or the task's name.
    pub fn text(&self) -> String {
        match (&self.label, &self.action) {
            (Some(l), _) => l.clone(),
            (None, ButtonAction::Command(c)) => c.title(),
            (None, ButtonAction::Task(t)) => t.clone(),
        }
    }
}

impl WindowConfig {
    /// `self` (the user's `settings.toml`) with every key `over` (`.workspace`) sets replaced.
    pub fn layered(&self, over: &WindowConfig) -> WindowConfig {
        WindowConfig {
            title: over.title.clone().or_else(|| self.title.clone()),
            status_bar: over.status_bar.or(self.status_bar),
            title_bar: over.title_bar.or(self.title_bar),
            buttons: over.buttons.clone().or_else(|| self.buttons.clone()),
        }
    }
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

/// What a new `.workspace` starts as: every section commented out, so it parses to the default.
pub const TEMPLATE: &str = "\
# jed workspace settings. Meant to be committed; uncomment a line to use it.
# Changes apply as soon as the file is saved.

# Overrides the user's settings.toml [settings], key by key (same keys; see that file's template).
# [settings]
# tab_width = 2
# insert_spaces = true

# [window]
# title = \"my project\"  # the window title is \"jed <title>\" instead of the root's path
# status_bar = \"blue\"  # red, orange, yellow, green, cyan, blue, purple, magenta, grey or auto

# [files]
# exclude = [\"build/\", \"*.log\"]  # globs hidden from the explorer, quick open and search
# associations = { \"SConstruct\" = \"python\" }  # glob = language

# The language server for a language id. command: a name on PATH; args and init_options are
# optional. Commands run only once the workspace is trusted.
# [lsp.python]
# command = \"pyright-langserver\"
# args = [\"--stdio\"]

# A task (Run Task): command, and optionally cwd and env.
# [tasks.build]
# command = \"cargo build\"
# cwd = \".\"
# env = { RUST_LOG = \"debug\" }
";

/// Blocking. Writes `TEMPLATE` to `path` unless the file exists; never overwrites one, even if it
/// appears meanwhile (`create_new`).
pub fn create_if_missing(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let fail = |e: std::io::Error| format!("Can't create {}: {e}", path.display());
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => f.write_all(TEMPLATE.as_bytes()).map_err(fail)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(fail(e)),
    }
    Ok(path.to_path_buf())
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
    fn template_parses_to_default_and_commented_examples_parse() {
        assert_eq!(WorkspaceConfig::parse(TEMPLATE).unwrap(), WorkspaceConfig::default());
        // Uncommenting every example line must give a valid config.
        let live: String = TEMPLATE
            .lines()
            .map(|l| {
                l.strip_prefix("# ")
                    .filter(|r| r.starts_with('[') || r.contains(" = "))
                    .unwrap_or(l)
            })
            .map(|l| l.split("  #").next().unwrap().to_string() + "\n")
            .collect();
        let c = WorkspaceConfig::parse(&live).unwrap();
        assert!(c.tasks.contains_key("build") && c.lsp.contains_key("python") && c.window.title.is_some());
    }

    #[test]
    fn create_if_missing_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".workspace");
        assert_eq!(create_if_missing(&path).unwrap(), path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
        std::fs::write(&path, "[files]\n").unwrap();
        create_if_missing(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[files]\n");
        // Any other failure is an error naming the file.
        let missing = dir.path().join("no-such-dir").join(".workspace");
        let e = create_if_missing(&missing).unwrap_err();
        assert!(e.starts_with("Can't create") && e.contains("no-such-dir"), "{e}");
    }

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
        assert_eq!(c.window.status_bar, None);
        let c = WorkspaceConfig::parse("[window]\ntitle = \"api\"").unwrap();
        assert_eq!(c.window.title.as_deref(), Some("api"));
        assert_eq!(c.window.status_bar, None, "the title alone");
        assert!(WorkspaceConfig::parse("[window]\ntitel = \"api\"").is_err());
    }

    #[test]
    fn status_bar_auto_parses() {
        let c = WorkspaceConfig::parse("[window]\nstatus_bar = \"auto\"").unwrap();
        assert_eq!(c.window.status_bar, Some(StatusBarColor::Auto));
    }

    #[test]
    fn every_status_bar_colour_parses() {
        for colour in StatusBarColor::ALL {
            let text = format!("[window]\nstatus_bar = \"{}\"", colour.name());
            let c = WorkspaceConfig::parse(&text).unwrap();
            assert_eq!(c.window.status_bar, Some(colour), "{text}");
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
