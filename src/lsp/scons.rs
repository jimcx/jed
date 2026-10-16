//! SCons support for ty: SCons runs `SConstruct` / `SConscript` with its globals (`Environment`,
//! `Import`, …) injected, which ty would flag as undefined. ty treats the symbols of a
//! `__builtins__.pyi` it finds on its search path as builtins, so in an SCons workspace (an
//! `SConstruct` at the root) red writes its own `__builtins__.pyi` (`BUILTINS`, embedded in the
//! binary) to `<support dir>/scons/` (`~/.local/state/red/lsp/scons/`) and adds that directory to
//! ty's `environment.extra-paths` through `initializationOptions.configuration` (ty's inline
//! settings; its arrays are merged with the project's own `ty.toml` / `pyproject.toml` ones).
//! Nothing is written into the workspace. A workspace with its own root `__builtins__.pyi` keeps
//! it (only one `__builtins__` module resolves, and ours would come first).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// SCons's globals as a builtins stub.
pub const BUILTINS: &str = include_str!("scons_builtins.pyi");

/// The names SCons looks for as the top-level build file.
const SCONSTRUCT: [&str; 6] = [
    "SConstruct",
    "Sconstruct",
    "sconstruct",
    "SConstruct.py",
    "Sconstruct.py",
    "sconstruct.py",
];

/// Whether `config` gets the SCons globals: ty (built in or named in `.workspace`).
pub fn applies(command_name: &str) -> bool {
    command_name == "ty"
}

/// The directory holding the stub for a workspace at `root`, written (or rewritten, when its text
/// differs) under `support_dir`; `None` when `root` isn't an SCons workspace, has its own
/// `__builtins__.pyi`, or the stub can't be written (logged).
///
/// Blocking: call from spawn_blocking.
pub fn prepare(root: &Path, support_dir: &Path) -> Option<PathBuf> {
    if !SCONSTRUCT.iter().any(|n| root.join(n).is_file()) || root.join("__builtins__.pyi").exists() {
        return None;
    }
    let dir = support_dir.join("scons");
    let file = dir.join("__builtins__.pyi");
    if std::fs::read_to_string(&file).is_ok_and(|t| t == BUILTINS) {
        return Some(dir);
    }
    // Written aside and renamed: two red instances (or tests) may write it at once.
    let tmp = dir.join(format!(".__builtins__.pyi.{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&tmp, BUILTINS))
        .and_then(|()| std::fs::rename(&tmp, &file));
    match written {
        Ok(()) => Some(dir),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!(path = %file.display(), "can't write the SCons builtins stub: {e}");
            None
        }
    }
}

/// `init` (a server's `initializationOptions`) with `dir` added to ty's
/// `configuration.environment.extra-paths`.
pub fn with_extra_path(init: Option<Value>, dir: &Path) -> Value {
    let mut init = match init {
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    };
    let path = Value::String(dir.to_string_lossy().into_owned());
    let mut at = &mut init;
    for key in ["configuration", "environment"] {
        if !at.get(key).is_some_and(Value::is_object) {
            at[key] = json!({});
        }
        at = &mut at[key];
    }
    match at.get_mut("extra-paths").and_then(Value::as_array_mut) {
        Some(paths) => paths.push(path),
        None => at["extra-paths"] = json!([path]),
    }
    init
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_scons_workspaces_get_the_stub() {
        let dir = tempfile::tempdir().unwrap();
        let (root, support) = (dir.path().join("w"), dir.path().join("support"));
        std::fs::create_dir(&root).unwrap();
        assert_eq!(prepare(&root, &support), None);
        assert!(!support.exists());
        std::fs::write(root.join("SConstruct"), "").unwrap();
        let stub_dir = prepare(&root, &support).unwrap();
        let stub = stub_dir.join("__builtins__.pyi");
        assert_eq!(std::fs::read_to_string(&stub).unwrap(), BUILTINS);
        // An old stub is replaced; nothing is written in the workspace.
        std::fs::write(&stub, "old").unwrap();
        assert_eq!(prepare(&root, &support), Some(stub_dir.clone()));
        assert_eq!(std::fs::read_to_string(&stub).unwrap(), BUILTINS);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&stub_dir).unwrap().count(), 1, "no temp file left");
        // The workspace's own builtins win.
        std::fs::write(root.join("__builtins__.pyi"), "").unwrap();
        assert_eq!(prepare(&root, &support), None);
        assert!(applies("ty"));
        assert!(!applies("clangd"));
    }

    #[test]
    fn extra_path_merges_into_init_options() {
        let dir = Path::new("/s/scons");
        assert_eq!(
            with_extra_path(None, dir),
            json!({ "configuration": { "environment": { "extra-paths": ["/s/scons"] } } })
        );
        let mine = json!({
            "logLevel": "debug",
            "configuration": { "environment": { "extra-paths": ["lib"], "python": ".venv" } },
        });
        assert_eq!(
            with_extra_path(Some(mine), dir),
            json!({
                "logLevel": "debug",
                "configuration": { "environment": { "extra-paths": ["lib", "/s/scons"], "python": ".venv" } },
            })
        );
    }

    #[test]
    fn stub_covers_the_usual_globals() {
        for name in [
            "Environment",
            "DefaultEnvironment",
            "Import",
            "Export",
            "Return",
            "SConscript",
            "Default",
            "Alias",
            "Glob",
            "Split",
            "ARGUMENTS",
            "COMMAND_LINE_TARGETS",
            "BUILD_TARGETS",
            "Help",
            "AddOption",
            "GetOption",
            "SetOption",
            "Dir",
            "File",
            "Builder",
            "Command",
            "Depends",
            "Variables",
            "Configure",
            "Execute",
            "Exit",
        ] {
            let def = BUILTINS.contains(&format!("def {name}("))
                || BUILTINS.contains(&format!("class {name}:"))
                || BUILTINS.contains(&format!("\n{name}: "));
            assert!(def, "{name} missing from the stub");
        }
    }
}
