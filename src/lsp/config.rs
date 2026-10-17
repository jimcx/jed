//! Which language server runs for a language: the built-ins (`clangd`, `ty`, `rust-analyzer`), or
//! `.workspace` `[lsp.<language id>]` when the workspace is trusted.

use std::path::Path;

use serde_json::Value;

use crate::syntax::Language;
use crate::workspace::config::WorkspaceConfig;
use crate::workspace::trust::Trust;

/// How to start one server. Two languages resolving to equal configs share one server.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    /// For messages and logs: the command's file name (`clangd`, `ty`).
    pub name: String,
    /// A bare name is looked up on `PATH`; a relative path with a `/` is relative to the
    /// workspace root.
    pub command: String,
    pub args: Vec<String>,
    /// `initializationOptions`.
    pub init_options: Option<Value>,
    /// From `.workspace` (so the not-found message points there).
    pub from_workspace: bool,
}

impl ServerConfig {
    pub fn new(command: &str, args: &[&str]) -> Self {
        Self {
            name: command_name(command),
            command: command.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            init_options: None,
            from_workspace: false,
        }
    }
}

fn command_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map_or_else(|| command.to_string(), |n| n.to_string_lossy().into_owned())
}

/// The servers jed knows without configuration: `clangd` for C and C++, `ty server` for Python,
/// `rust-analyzer` for Rust (no `initializationOptions`: it finds the cargo workspace under the
/// root itself, and checks with `cargo check` on save).
#[derive(Debug, Clone)]
pub struct Builtins {
    pub clangd: ServerConfig,
    pub ty: ServerConfig,
    pub rust_analyzer: ServerConfig,
}

impl Default for Builtins {
    fn default() -> Self {
        Self {
            clangd: ServerConfig::new("clangd", &[]),
            ty: ServerConfig::new("ty", &["server"]),
            rust_analyzer: ServerConfig::new("rust-analyzer", &[]),
        }
    }
}

impl Builtins {
    fn for_language(&self, language: Language) -> Option<&ServerConfig> {
        match language {
            Language::C | Language::Cpp => Some(&self.clangd),
            Language::Python => Some(&self.ty),
            Language::Rust => Some(&self.rust_analyzer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Server(ServerConfig),
    /// `.workspace` names a server for this language but trust isn't decided yet: run nothing
    /// until it is (neither the workspace's nor the built-in one).
    AwaitingTrust,
    None,
}

/// The server for `language`: `.workspace` `[lsp.<id>]` (keyed by `Language::id`) when trusted;
/// in restricted mode the built-in one, as "servers from global config still run".
pub fn resolve(language: Language, workspace: &WorkspaceConfig, trust: Trust, builtins: &Builtins) -> Resolved {
    if let Some(entry) = workspace.lsp.get(language.id()) {
        match trust {
            Trust::Trusted => {
                return Resolved::Server(ServerConfig {
                    name: command_name(&entry.command),
                    command: entry.command.clone(),
                    args: entry.args.clone(),
                    init_options: entry.init_options.as_ref().and_then(|v| serde_json::to_value(v).ok()),
                    from_workspace: true,
                });
            }
            Trust::Undecided => return Resolved::AwaitingTrust,
            // `NotNeeded` can't happen with an `[lsp]` entry; treat it as restricted.
            Trust::Restricted | Trust::NotNeeded => {}
        }
    }
    match builtins.for_language(language) {
        Some(c) => Resolved::Server(c.clone()),
        None => Resolved::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS: &str = r#"
[lsp.python]
command = "/opt/pyright/bin/pyright-langserver"
args = ["--stdio"]
init_options = { python = { venv = ".venv" } }
"#;

    #[test]
    fn workspace_servers_only_when_trusted() {
        let ws = WorkspaceConfig::parse(WS).unwrap();
        let b = Builtins::default();
        let Resolved::Server(s) = resolve(Language::Python, &ws, Trust::Trusted, &b) else {
            panic!()
        };
        assert_eq!(s.name, "pyright-langserver");
        assert_eq!(s.args, ["--stdio"]);
        assert!(s.from_workspace);
        assert_eq!(
            s.init_options,
            Some(serde_json::json!({ "python": { "venv": ".venv" } }))
        );
        assert_eq!(
            resolve(Language::Python, &ws, Trust::Undecided, &b),
            Resolved::AwaitingTrust
        );
        assert_eq!(
            resolve(Language::Python, &ws, Trust::Restricted, &b),
            Resolved::Server(b.ty.clone()),
            "restricted: the built-in one"
        );
        let none = WorkspaceConfig::default();
        assert_eq!(
            resolve(Language::Cpp, &none, Trust::NotNeeded, &b),
            Resolved::Server(b.clangd.clone())
        );
        assert_eq!(
            resolve(Language::C, &ws, Trust::Undecided, &b),
            Resolved::Server(b.clangd.clone())
        );
        assert_eq!(resolve(Language::Markdown, &none, Trust::NotNeeded, &b), Resolved::None);
        assert_eq!(b.ty.command, "ty");
        assert_eq!(b.ty.args, ["server"]);
    }

    #[test]
    fn rust_analyzer_is_built_in_and_overridable() {
        let b = Builtins::default();
        let none = WorkspaceConfig::default();
        let Resolved::Server(s) = resolve(Language::Rust, &none, Trust::NotNeeded, &b) else {
            panic!()
        };
        assert_eq!(s, b.rust_analyzer);
        assert_eq!(
            (s.name.as_str(), s.command.as_str()),
            ("rust-analyzer", "rust-analyzer")
        );
        assert!(s.args.is_empty());
        assert_eq!(s.init_options, None, "rust-analyzer's defaults: `cargo check` on save");
        assert!(!s.from_workspace);

        let ws = WorkspaceConfig::parse("[lsp.rust]\ncommand = \"/opt/ra\"\n").unwrap();
        let Resolved::Server(s) = resolve(Language::Rust, &ws, Trust::Trusted, &b) else {
            panic!()
        };
        assert_eq!((s.name.as_str(), s.from_workspace), ("ra", true));
        assert_eq!(
            resolve(Language::Rust, &ws, Trust::Restricted, &b),
            Resolved::Server(b.rust_analyzer.clone())
        );
        assert_eq!(
            resolve(Language::Rust, &ws, Trust::Undecided, &b),
            Resolved::AwaitingTrust
        );
    }
}
