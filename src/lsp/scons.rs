//! SCons support for ty: SCons runs `SConstruct` / `SConscript` with its globals (`Environment`,
//! `Import`, …) injected, which ty would flag as undefined. ty treats the symbols of a
//! `__builtins__.pyi` it finds on its search path as builtins, so in an SCons workspace (an
//! `SConstruct` at the root) jed writes its own `__builtins__.pyi` (`BUILTINS`, embedded in the
//! binary) to `<support dir>/scons/` (`~/.local/state/jed/lsp/scons/`) and adds that directory to
//! ty's `environment.extra-paths` through `initializationOptions.configuration` (ty's inline
//! settings; its arrays are merged with the project's own `ty.toml` / `pyproject.toml` ones).
//! `.workspace`'s `[scons]` (`SconsConfig`) names a build file in a subfolder, a folder with the
//! `SCons` package and can turn the stub off; `site_scons/` beside the build file is added too.
//! Nothing is written into the workspace. A workspace with its own root `__builtins__.pyi` keeps
//! it (only one `__builtins__` module resolves, and ours would come first).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::workspace::config::SconsConfig;

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

/// The folders to add to ty's `extra-paths` for a workspace at `root`, in order: the stub's
/// directory (written, or rewritten when its text differs, under `support_dir`), `site_scons/`
/// beside the build file, then `config.scons_path`. Empty when `config.enabled` is `Some(false)`
/// or `root` isn't an SCons workspace (no build file: `config.sconstruct`, else a root
/// `SConstruct`). A root `__builtins__.pyi` or a stub that can't be written (logged) skips only
/// the stub.
///
/// Blocking: call from spawn_blocking.
pub fn prepare(root: &Path, support_dir: &Path, config: &SconsConfig) -> Vec<PathBuf> {
    if config.enabled == Some(false) {
        return Vec::new();
    }
    let build_dir = match &config.sconstruct {
        Some(path) => {
            let file = root.join(expand_home(path));
            if !file.is_file() {
                tracing::warn!(path = %file.display(), "[scons] sconstruct isn't a file");
                return Vec::new();
            }
            file.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf())
        }
        None if SCONSTRUCT.iter().any(|n| root.join(n).is_file()) => root.to_path_buf(),
        None => return Vec::new(),
    };
    let mut paths = Vec::new();
    if !root.join("__builtins__.pyi").exists()
        && let Some(dir) = write_stub(support_dir)
    {
        paths.push(dir);
    }
    let site = build_dir.join("site_scons");
    if site.is_dir() {
        paths.push(site);
    }
    if let Some(path) = &config.scons_path {
        let dir = root.join(expand_home(path));
        if !dir.join("SCons").join("__init__.py").is_file() {
            tracing::warn!(path = %dir.display(), "[scons] scons_path has no SCons/__init__.py");
        }
        paths.push(dir);
    }
    paths
}

/// `path` with a leading `~` / `~/` replaced by the home folder.
fn expand_home(path: &str) -> PathBuf {
    let rest = if path == "~" { Some("") } else { path.strip_prefix("~/") };
    match (rest, std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Writes the stub under `support_dir/scons` (when its text differs); the directory, or `None`.
fn write_stub(support_dir: &Path) -> Option<PathBuf> {
    let dir = support_dir.join("scons");
    let file = dir.join("__builtins__.pyi");
    if std::fs::read_to_string(&file).is_ok_and(|t| t == BUILTINS) {
        return Some(dir);
    }
    // Written aside and renamed: two jed instances (or tests) may write it at once.
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

/// `init` (a server's `initializationOptions`) with `dirs` appended, in order, to ty's
/// `configuration.environment.extra-paths`.
pub fn with_extra_path(init: Option<Value>, dirs: &[PathBuf]) -> Value {
    let mut init = match init {
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    };
    let mut at = &mut init;
    for key in ["configuration", "environment"] {
        if !at.get(key).is_some_and(Value::is_object) {
            at[key] = json!({});
        }
        at = &mut at[key];
    }
    if !at.get("extra-paths").is_some_and(Value::is_array) {
        at["extra-paths"] = json!([]);
    }
    if let Some(paths) = at.get_mut("extra-paths").and_then(Value::as_array_mut) {
        paths.extend(dirs.iter().map(|d| Value::String(d.to_string_lossy().into_owned())));
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
        let none = SconsConfig::default();
        assert!(prepare(&root, &support, &none).is_empty());
        assert!(!support.exists());
        std::fs::write(root.join("SConstruct"), "").unwrap();
        let stub_dir = prepare(&root, &support, &none).pop().unwrap();
        let stub = stub_dir.join("__builtins__.pyi");
        assert_eq!(std::fs::read_to_string(&stub).unwrap(), BUILTINS);
        // An old stub is replaced; nothing is written in the workspace.
        std::fs::write(&stub, "old").unwrap();
        assert_eq!(prepare(&root, &support, &none), vec![stub_dir.clone()]);
        assert_eq!(std::fs::read_to_string(&stub).unwrap(), BUILTINS);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(&stub_dir).unwrap().count(), 1, "no temp file left");
        // The workspace's own builtins win.
        std::fs::write(root.join("__builtins__.pyi"), "").unwrap();
        assert!(prepare(&root, &support, &none).is_empty());
        assert!(applies("ty"));
        assert!(!applies("clangd"));
    }

    #[test]
    fn extra_path_merges_into_init_options() {
        let dir = Path::new("/s/scons");
        assert_eq!(
            with_extra_path(None, &[dir.to_path_buf()]),
            json!({ "configuration": { "environment": { "extra-paths": ["/s/scons"] } } })
        );
        let mine = json!({
            "logLevel": "debug",
            "configuration": { "environment": { "extra-paths": ["lib"], "python": ".venv" } },
        });
        assert_eq!(
            with_extra_path(Some(mine), &[dir.to_path_buf(), PathBuf::from("/x")]),
            json!({
                "logLevel": "debug",
                "configuration": { "environment": { "extra-paths": ["lib", "/s/scons", "/x"], "python": ".venv" } },
            })
        );
    }

    fn cfg(sconstruct: Option<&str>, scons_path: Option<&str>) -> SconsConfig {
        SconsConfig {
            sconstruct: sconstruct.map(Into::into),
            scons_path: scons_path.map(Into::into),
            enabled: None,
        }
    }

    #[test]
    fn subfolder_sconstruct_brings_its_site_scons() {
        let dir = tempfile::tempdir().unwrap();
        let (root, support) = (dir.path().join("w"), dir.path().join("support"));
        std::fs::create_dir_all(root.join("build/site_scons")).unwrap();
        // No build file anywhere: nothing.
        assert!(prepare(&root, &support, &cfg(Some("build/SConstruct"), None)).is_empty());
        std::fs::write(root.join("build/SConstruct"), "").unwrap();
        let stub = support.join("scons");
        let site = root.join("build/site_scons");
        assert_eq!(
            prepare(&root, &support, &cfg(Some("build/SConstruct"), None)),
            vec![stub.clone(), site.clone()]
        );
        let abs = root.join("build/SConstruct");
        assert_eq!(
            prepare(&root, &support, &cfg(abs.to_str(), None)),
            vec![stub.clone(), site.clone()]
        );
        // Without the setting the subfolder isn't looked at.
        assert!(prepare(&root, &support, &cfg(None, None)).is_empty());
    }

    #[test]
    fn root_site_scons_goes_on_the_path_without_the_setting() {
        let dir = tempfile::tempdir().unwrap();
        let (root, support) = (dir.path().join("w"), dir.path().join("support"));
        std::fs::create_dir_all(root.join("site_scons")).unwrap();
        std::fs::write(root.join("SConstruct"), "").unwrap();
        assert_eq!(
            prepare(&root, &support, &cfg(None, None)),
            vec![support.join("scons"), root.join("site_scons")]
        );
        // The workspace's own builtins skip only the stub.
        std::fs::write(root.join("__builtins__.pyi"), "").unwrap();
        assert_eq!(
            prepare(&root, &support, &cfg(None, None)),
            vec![root.join("site_scons")]
        );
    }

    #[test]
    fn scons_path_comes_last_and_is_passed_even_when_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let (root, support) = (dir.path().join("w"), dir.path().join("support"));
        std::fs::create_dir_all(root.join("site_scons")).unwrap();
        std::fs::write(root.join("SConstruct"), "").unwrap();
        let paths = prepare(&root, &support, &cfg(None, Some("/no/such/scons")));
        assert_eq!(
            paths,
            vec![
                support.join("scons"),
                root.join("site_scons"),
                PathBuf::from("/no/such/scons")
            ]
        );
        // After the project's own extra-paths, through the merge.
        let init = json!({ "configuration": { "environment": { "extra-paths": ["lib"] } } });
        let merged = with_extra_path(Some(init), &paths);
        assert_eq!(merged["configuration"]["environment"]["extra-paths"][0], "lib");
        assert_eq!(
            merged["configuration"]["environment"]["extra-paths"][3],
            "/no/such/scons"
        );
        // `~` is the home folder, `~/x` below it.
        if let Some(home) = std::env::home_dir() {
            assert_eq!(expand_home("~"), home);
            assert_eq!(expand_home("~/x/y"), home.join("x/y"));
        }
        assert_eq!(expand_home("rel/p"), PathBuf::from("rel/p"));
    }

    #[test]
    fn disabled_writes_and_returns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (root, support) = (dir.path().join("w"), dir.path().join("support"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("SConstruct"), "").unwrap();
        let off = SconsConfig {
            enabled: Some(false),
            scons_path: Some("/x".into()),
            ..SconsConfig::default()
        };
        assert!(prepare(&root, &support, &off).is_empty());
        assert!(!support.exists());
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
