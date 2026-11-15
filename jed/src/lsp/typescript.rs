//! `typescript-language-server` launch preparation: a project that has the server and TypeScript in its own
//! `node_modules` runs those (as VS Code runs the workspace's TypeScript), so the language service matches
//! the project's compiler version. The root stays the workspace root: tsserver finds each file's nearest
//! `tsconfig.json` / `jsconfig.json` itself (a file under no config gets an inferred project).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The built-in's command and name.
pub(crate) const SERVER: &str = "typescript-language-server";

/// What to launch differently for the workspace at `root`.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Prepared {
    /// The workspace's `node_modules/.bin/typescript-language-server`, when the command is that bare name.
    pub command: Option<PathBuf>,
    /// The workspace's `node_modules/typescript/lib`, for `initializationOptions.tsserver.path`.
    pub tsserver: Option<PathBuf>,
}

/// Decide from file checks (`is_file`, `is_dir`): only for a bare `typescript-language-server` command (the
/// built-in's, or a configured one of that name); never overrides an
/// `initializationOptions.tsserver.path` the user set.
fn decide(
    command: &str,
    init: Option<&Value>,
    root: &Path,
    is_file: impl Fn(&Path) -> bool,
    is_dir: impl Fn(&Path) -> bool,
) -> Prepared {
    if command != SERVER {
        return Prepared::default();
    }
    let bin = root.join("node_modules/.bin").join(SERVER);
    let lib = root.join("node_modules/typescript/lib");
    let own_path = init.is_some_and(|i| i.pointer("/tsserver/path").is_some());
    Prepared {
        command: is_file(&bin).then_some(bin),
        tsserver: (!own_path && is_dir(&lib) && is_file(&lib.join("tsserver.js"))).then_some(lib),
    }
}

/// Blocking: call from spawn_blocking
pub(crate) fn prepare(command: &str, init: Option<&Value>, root: &Path) -> Prepared {
    decide(command, init, root, Path::is_file, Path::is_dir)
}

/// `init` with `tsserver.path` set to `lib`, other options kept.
pub(crate) fn with_tsserver_path(init: Option<Value>, lib: &Path) -> Value {
    let mut init = match init {
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    };
    if !init.get("tsserver").is_some_and(Value::is_object) {
        init["tsserver"] = json!({});
    }
    init["tsserver"]["path"] = Value::String(lib.to_string_lossy().into_owned());
    init
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: &Path) -> bool {
        true
    }
    fn none(_: &Path) -> bool {
        false
    }

    #[test]
    fn workspace_copies_are_preferred_for_the_bare_built_in_only() {
        let root = Path::new("/w");
        let p = decide(SERVER, None, root, all, all);
        assert_eq!(
            p.command.as_deref(),
            Some(Path::new("/w/node_modules/.bin/typescript-language-server"))
        );
        assert_eq!(p.tsserver.as_deref(), Some(Path::new("/w/node_modules/typescript/lib")));
        assert_eq!(decide(SERVER, None, root, none, none), Prepared::default());
        assert_eq!(decide("/opt/tsls", None, root, all, all), Prepared::default());
        // tsserver.js missing: no path.
        let no_js = |p: &Path| !p.ends_with("tsserver.js");
        assert_eq!(decide(SERVER, None, root, no_js, all).tsserver, None);
    }

    #[test]
    fn a_configured_tsserver_path_is_kept() {
        let own = json!({ "tsserver": { "path": "/x" }, "hostInfo": "jed" });
        assert_eq!(decide(SERVER, Some(&own), Path::new("/w"), all, all).tsserver, None);
        let merged = with_tsserver_path(Some(json!({ "hostInfo": "jed" })), Path::new("/w/lib"));
        assert_eq!(merged, json!({ "hostInfo": "jed", "tsserver": { "path": "/w/lib" } }));
        assert_eq!(
            with_tsserver_path(None, Path::new("/l")),
            json!({ "tsserver": { "path": "/l" } })
        );
    }
}
