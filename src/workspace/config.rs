//! The `.workspace` file: per-project settings, commands and file rules. Meant to be committed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub settings: SettingsOverrides,
    pub files: FilesConfig,
    pub lsp: BTreeMap<String, LspServer>,
    pub tasks: BTreeMap<String, Task>,
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SettingsOverrides {
    pub tab_width: Option<usize>,
    pub insert_spaces: Option<bool>,
    /// No parse tree (so no highlighting) for buffers bigger than this; 0 turns parsing off.
    pub syntax_max_bytes: Option<usize>,
    /// Brackets and quotes auto-close, type over and delete in pairs, and wrap a selection.
    pub auto_close_pairs: Option<bool>,
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
    fn unknown_keys_are_errors() {
        assert!(WorkspaceConfig::parse("[settings]\ntab_widht = 2").is_err());
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
