//! Task templates: `.workspace` `[tasks]` and the user's `tasks.toml`, merged, and resolved into
//! what a run needs (shell argv, cwd, env).
//!
//! `tasks.toml` (in `paths::config_dir()`) uses the same tables as `.workspace`:
//!
//! ```toml
//! [tasks.build]
//! command = "cargo build --color=always 2>&1 | tee build.log"   # run by `$SHELL -c`
//! cwd = "src"                  # optional: relative to the workspace root; `~/…` is the home folder
//! env = { RUST_LOG = "debug" } # optional: on top of jed's environment, values taken literally
//! ```
//!
//! Read once at startup like `settings.toml` (`main`), leniently: a TOML syntax error loses the
//! file (one error with its line), a bad task (unknown key, wrong type, empty command) is reported
//! and skipped, the others kept.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub use crate::workspace::config::Task;

pub const FILE_NAME: &str = "tasks.toml";

/// Where a template comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `.workspace` `[tasks]` (only while the workspace is trusted).
    Workspace,
    /// The user's `tasks.toml`.
    User,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    pub task: Task,
    pub source: Source,
}

/// `tasks.toml`'s tasks by name, and what was wrong with it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UserTasks {
    pub tasks: BTreeMap<String, Task>,
    pub errors: Vec<String>,
}

/// Blocking: call before the UI starts (as `main` does) or from `spawn_blocking`.
/// A missing file is no tasks and no error.
pub fn load(path: &Path) -> UserTasks {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UserTasks::default(),
        Err(e) => UserTasks {
            tasks: BTreeMap::new(),
            errors: vec![format!("{FILE_NAME}: {e}")],
        },
    }
}

/// Parses `tasks.toml`, skipping (and reporting) bad tasks and stray keys.
pub fn parse(text: &str) -> UserTasks {
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => {
            let error = match e.span() {
                Some(at) => {
                    let line = text[..at.start.min(text.len())].matches('\n').count() + 1;
                    format!("{FILE_NAME}: line {line}: {}", e.message().trim_end())
                }
                None => format!("{FILE_NAME}: {}", e.message().trim_end()),
            };
            return UserTasks {
                tasks: BTreeMap::new(),
                errors: vec![error],
            };
        }
    };
    let mut out = UserTasks::default();
    for (key, value) in table {
        match (key.as_str(), value) {
            ("tasks", toml::Value::Table(tasks)) => {
                for (name, value) in tasks {
                    match value.try_into::<Task>() {
                        Ok(task) if task.command.trim().is_empty() => {
                            out.errors.push(format!("{FILE_NAME}: [tasks.{name}]: empty `command`"))
                        }
                        Ok(task) => {
                            out.tasks.insert(name, task);
                        }
                        Err(e) => out
                            .errors
                            .push(format!("{FILE_NAME}: [tasks.{name}]: {}", e.message().trim_end())),
                    }
                }
            }
            ("tasks", _) => out.errors.push(format!("{FILE_NAME}: `tasks` must be a table")),
            (_, toml::Value::Table(_)) => out.errors.push(format!(
                "{FILE_NAME}: unknown table [{key}] (tasks go in [tasks.<name>])"
            )),
            _ => out
                .errors
                .push(format!("{FILE_NAME}: `{key}` belongs under [tasks.<name>]")),
        }
    }
    out
}

/// Every template, sorted by name: `workspace`'s (`None` when the workspace isn't trusted) and
/// `user`'s; the workspace's wins a name both have.
pub fn merge(workspace: Option<&BTreeMap<String, Task>>, user: &BTreeMap<String, Task>) -> Vec<Template> {
    let mut all: BTreeMap<&str, Template> = BTreeMap::new();
    for (name, task) in user {
        all.insert(
            name,
            Template {
                name: name.clone(),
                task: task.clone(),
                source: Source::User,
            },
        );
    }
    for (name, task) in workspace.into_iter().flatten() {
        all.insert(
            name,
            Template {
                name: name.clone(),
                task: task.clone(),
                source: Source::Workspace,
            },
        );
    }
    all.into_values().collect()
}

/// A template made ready to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub name: String,
    /// The template's command line, as typed.
    pub command: String,
    /// `[shell, "-c", command]`.
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
    /// Set on top of jed's own environment, in order (the template's last, so it wins).
    pub env: Vec<(String, String)>,
}

/// What every run gets unless its template sets them: tools see a colour terminal.
pub const DEFAULT_ENV: [(&str, &str); 2] = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];

impl Template {
    /// The command through `shell -c` (so templates can use pipes, `&&`, globs), the cwd resolved
    /// against `root` (none = the root; `~` / `~/…` = the home folder), `DEFAULT_ENV` then the
    /// template's env.
    pub fn resolve(&self, root: &Path, shell: &Path) -> Spec {
        let mut env: Vec<(String, String)> = DEFAULT_ENV
            .iter()
            .filter(|(k, _)| !self.task.env.contains_key(*k))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        env.extend(self.task.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        Spec {
            name: self.name.clone(),
            command: self.task.command.clone(),
            argv: vec![shell.into(), "-c".into(), self.task.command.clone().into()],
            cwd: resolve_cwd(root, self.task.cwd.as_deref()),
            env,
        }
    }
}

/// `None` → `root`; `~` / `~/x` → the home folder (/ x); relative → under `root`; absolute as is.
pub fn resolve_cwd(root: &Path, cwd: Option<&str>) -> PathBuf {
    match cwd {
        None | Some("") | Some(".") => root.to_path_buf(),
        Some("~") => crate::paths::home(),
        Some(c) if c.starts_with("~/") => crate::paths::home().join(&c[2..]),
        Some(c) => root.join(c),
    }
}

/// `$SHELL` when it's an absolute path, else `/bin/sh`.
pub fn default_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(command: &str) -> Task {
        Task {
            command: command.into(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn parses_tasks_toml() {
        let t = parse(
            r#"
[tasks.build]
command = "make -j8"
cwd = "src"
env = { CC = "clang" }

[tasks.test]
command = "make test"
"#,
        );
        assert_eq!(t.errors, Vec::<String>::new());
        assert_eq!(t.tasks.len(), 2);
        assert_eq!(t.tasks["build"].cwd.as_deref(), Some("src"));
        assert_eq!(t.tasks["build"].env["CC"], "clang");
        assert_eq!(t.tasks["test"].command, "make test");
    }

    #[test]
    fn bad_tasks_are_reported_and_skipped() {
        let t = parse(
            r#"
top = 1

[other]
x = 1

[tasks.good]
command = "true"

[tasks.typo]
comand = "x"

[tasks.empty]
command = "  "

[tasks.wrong]
command = 3
"#,
        );
        assert_eq!(t.tasks.keys().collect::<Vec<_>>(), vec!["good"]);
        assert_eq!(t.errors.len(), 5, "{:?}", t.errors);
        assert!(t.errors.iter().any(|e| e.contains("`top` belongs under")));
        assert!(t.errors.iter().any(|e| e.contains("unknown table [other]")));
        assert!(
            t.errors
                .iter()
                .any(|e| e.contains("[tasks.typo]") && e.contains("comand"))
        );
        assert!(t.errors.iter().any(|e| e.contains("[tasks.empty]: empty `command`")));
        assert!(t.errors.iter().any(|e| e.contains("[tasks.wrong]")));
    }

    #[test]
    fn syntax_error_names_its_line() {
        let t = parse("[tasks.a]\ncommand = \"x\"\n[tasks.b\n");
        assert!(t.tasks.is_empty());
        assert_eq!(t.errors.len(), 1);
        assert!(t.errors[0].starts_with("tasks.toml: line 3:"), "{}", t.errors[0]);
    }

    #[test]
    fn missing_file_is_no_tasks() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join(FILE_NAME)), UserTasks::default());
    }

    #[test]
    fn workspace_wins_a_clash() {
        let ws = BTreeMap::from([("build".to_string(), task("scons")), ("lint".to_string(), task("ruff"))]);
        let user = BTreeMap::from([("build".to_string(), task("make")), ("clean".to_string(), task("rm"))]);
        let all = merge(Some(&ws), &user);
        let names: Vec<_> = all
            .iter()
            .map(|t| (t.name.as_str(), t.task.command.as_str(), t.source))
            .collect();
        assert_eq!(
            names,
            vec![
                ("build", "scons", Source::Workspace),
                ("clean", "rm", Source::User),
                ("lint", "ruff", Source::Workspace),
            ]
        );
        let user_only: Vec<_> = merge(None, &user).into_iter().map(|t| t.source).collect();
        assert_eq!(user_only, vec![Source::User, Source::User]);
    }

    #[test]
    fn resolves_cwd_env_and_shell() {
        let root = Path::new("/w");
        assert_eq!(resolve_cwd(root, None), root);
        assert_eq!(resolve_cwd(root, Some("src/a")), Path::new("/w/src/a"));
        assert_eq!(resolve_cwd(root, Some("/abs")), Path::new("/abs"));
        assert_eq!(resolve_cwd(root, Some("~/x")), crate::paths::home().join("x"));

        let mut t = Template {
            name: "b".into(),
            task: task("make | tee log"),
            source: Source::User,
        };
        t.task.env.insert("TERM".into(), "dumb".into());
        t.task.env.insert("A".into(), "1".into());
        let spec = t.resolve(root, Path::new("/bin/zsh"));
        assert_eq!(
            spec.argv,
            vec![OsString::from("/bin/zsh"), "-c".into(), "make | tee log".into()]
        );
        assert_eq!(spec.cwd, root);
        assert_eq!(
            spec.env,
            vec![
                ("COLORTERM".to_string(), "truecolor".to_string()),
                ("A".to_string(), "1".to_string()),
                ("TERM".to_string(), "dumb".to_string()),
            ],
            "the template's TERM replaces the default"
        );
    }
}
