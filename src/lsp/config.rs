//! Which language server runs for a language: the built-ins (`clangd`, `ty`, `rust-analyzer`), or
//! `.workspace` `[lsp.<language id>]` when the workspace is trusted, or the user's `settings.toml`
//! `[lsp.<language id>]` (always trusted: the user's own file).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::syntax::Language;
use crate::workspace::config::{LspServer, WorkspaceConfig};
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

/// The servers jed knows without configuration: `clangd` for C, C++, Objective-C and Objective-C++, `ty server` for Python,
/// `rust-analyzer` for Rust (no `initializationOptions`: it finds the cargo workspace under the
/// root itself, and checks with `cargo check` on save).
#[derive(Debug, Clone)]
pub struct Builtins {
    pub clangd: ServerConfig,
    pub ty: ServerConfig,
    pub rust_analyzer: ServerConfig,
    /// The user's `settings.toml` `[lsp.<id>]` servers by language id: they beat the three above.
    pub user: BTreeMap<String, ServerConfig>,
}

impl Default for Builtins {
    fn default() -> Self {
        Self {
            clangd: ServerConfig::new("clangd", &[]),
            ty: ServerConfig::new("ty", &["server"]),
            rust_analyzer: ServerConfig::new("rust-analyzer", &[]),
            user: BTreeMap::new(),
        }
    }
}

impl Builtins {
    fn for_language(&self, language: Language) -> Option<&ServerConfig> {
        if let Some(c) = self.user.get(language.id()) {
            return Some(c);
        }
        match language {
            Language::C | Language::Cpp | Language::ObjC | Language::ObjCpp => Some(&self.clangd),
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

fn server_config(entry: &LspServer, from_workspace: bool) -> ServerConfig {
    ServerConfig {
        name: command_name(&entry.command),
        command: entry.command.clone(),
        args: entry.args.clone(),
        init_options: entry.init_options.as_ref().and_then(|v| serde_json::to_value(v).ok()),
        from_workspace,
    }
}

/// The user's `[lsp.<id>]` entries as server configs; an id that is neither a built-in language
/// nor a registered syntax plugin is reported and dropped (call after the plugins are registered).
pub fn user_servers(entries: &BTreeMap<String, LspServer>) -> (BTreeMap<String, ServerConfig>, Vec<String>) {
    let mut servers = BTreeMap::new();
    let mut errors = Vec::new();
    for (id, entry) in entries {
        if Language::from_id(id).is_some() {
            servers.insert(id.clone(), server_config(entry, false));
        } else {
            errors.push(format!("settings.toml: [lsp.{id}]: unknown language \"{id}\""));
        }
    }
    (servers, errors)
}

/// The server for `language`: `.workspace` `[lsp.<id>]` (keyed by `Language::id`) when trusted
/// (undecided: none yet); then the user's `settings.toml` `[lsp.<id>]`; then the built-in one. In
/// restricted mode the latter two still run, as "servers from global config still run".
pub fn resolve(language: Language, workspace: &WorkspaceConfig, trust: Trust, builtins: &Builtins) -> Resolved {
    if let Some(entry) = workspace.lsp.get(language.id()) {
        match trust {
            Trust::Trusted => {
                return Resolved::Server(server_config(entry, true));
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

    #[test]
    fn user_servers_sit_between_the_workspace_and_the_built_ins() {
        let ws = WorkspaceConfig::parse(WS).unwrap();
        let none = WorkspaceConfig::default();
        let mut b = Builtins::default();
        let user: BTreeMap<String, LspServer> = toml::from_str(
            "[python]\ncommand = \"pyright\"\nargs = [\"--stdio\"]\n[nolang]\ncommand = \"x\"\n[cmake]\ncommand = \"cmake-ls\"\n",
        )
        .unwrap();
        let (servers, errors) = user_servers(&user);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("[lsp.nolang]"));
        b.user = servers;
        let name = |r: Resolved| match r {
            Resolved::Server(s) => (s.name, s.from_workspace),
            r => panic!("{r:?}"),
        };
        // No workspace entry: the user's beats the built-in ty, in every trust state.
        for trust in [Trust::Trusted, Trust::Restricted, Trust::NotNeeded, Trust::Undecided] {
            assert_eq!(
                name(resolve(Language::Python, &none, trust, &b)),
                ("pyright".into(), false)
            );
        }
        // A language without a built-in.
        assert_eq!(
            name(resolve(Language::CMake, &none, Trust::Trusted, &b)),
            ("cmake-ls".into(), false)
        );
        // A workspace entry: trusted wins, undecided waits, restricted falls to the user's.
        assert_eq!(
            name(resolve(Language::Python, &ws, Trust::Trusted, &b)),
            ("pyright-langserver".into(), true)
        );
        assert_eq!(
            resolve(Language::Python, &ws, Trust::Undecided, &b),
            Resolved::AwaitingTrust
        );
        assert_eq!(
            name(resolve(Language::Python, &ws, Trust::Restricted, &b)),
            ("pyright".into(), false)
        );
        // Others keep the built-ins.
        assert_eq!(
            name(resolve(Language::Rust, &none, Trust::Trusted, &b)).0,
            "rust-analyzer"
        );
    }

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
