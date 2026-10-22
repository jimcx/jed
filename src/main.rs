mod app;
mod backup;
mod blame;
mod brackets;
mod buffer;
mod cheatsheet;
mod clipboard;
mod code_action;
mod column;
mod command;
mod comment;
mod commit_view;
mod completion;
mod config;
mod diagnostics;
mod diff_view;
mod disk;
mod edit;
mod explorer;
mod field;
mod find;
#[cfg(test)]
mod fixes_tests;
mod format;
mod git;
mod git_history;
#[cfg(test)]
mod git_history_tests;
mod git_show;
mod group;
mod grouped;
mod hex;
#[cfg(test)]
mod hex_tests;
mod history;
mod hover;
#[cfg(test)]
mod hscroll_tests;
mod indent;
mod indentation;
#[cfg(test)]
mod indentation_tests;
mod layout;
mod lines;
mod locations;
mod logging;
mod logs;
mod lsp;
#[cfg(test)]
mod milestone10_tests;
#[cfg(test)]
mod milestone11_tests;
#[cfg(test)]
mod milestone12_tests;
#[cfg(test)]
mod milestone13_tests;
#[cfg(test)]
mod milestone2_tests;
#[cfg(test)]
mod milestone3_tests;
#[cfg(test)]
mod milestone4_tests;
#[cfg(test)]
mod milestone5_tests;
#[cfg(test)]
mod milestone6_tests;
#[cfg(test)]
mod milestone7_tests;
#[cfg(test)]
mod milestone8_tests;
#[cfg(test)]
mod milestone9_tests;
#[cfg(test)]
mod milestone_tests;
mod multicursor;
mod navigation;
mod pairs;
mod palette;
mod panic;
mod path_filter;
mod paths;
mod problems;
mod quick_diff;
mod recent;
mod rename;
#[cfg(test)]
mod replace_tests;
mod scm;
mod scrollbar;
mod search;
mod search_view;
mod selection;
mod session;
mod shift;
mod syntax;
#[cfg(test)]
mod syntax_plugins_tests;
mod tasks;
mod terminal;
mod theme;
#[cfg(test)]
mod todo_tests;
#[cfg(test)]
mod tools_tests;
mod tools_view;
mod ui;
mod view;
mod watcher;
mod workspace;
mod workspace_search;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use buffer::Buffer;
use config::keymap::{self, Keymap, Platform};
use workspace::Workspace;
use workspace::trust::TrustStore;

const USAGE: &str = "usage: jed [--log-level <filter>] [<folder> | <file>[:<line>[:<col>]]]\n       \
                     jed [--log-level <filter>] -s | --single <file>[:<line>[:<col>]]   (single file mode)";

#[derive(Debug, PartialEq)]
struct Args {
    target: Option<String>,
    log_level: Option<String>,
    /// `-s` / `--single`: single file mode (`app::Mode::SingleFile`).
    single: bool,
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut target = None;
    let mut log_level = None;
    let mut single = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "-V" | "--version" => return Err(format!("jed {}", env!("JED_VERSION"))),
            "--log-level" => log_level = Some(args.next().ok_or("--log-level needs a value")?),
            "-s" | "--single" => single = true,
            a if a.starts_with('-') => return Err(format!("unknown option {a}\n{USAGE}")),
            a if target.is_none() => target = Some(a.to_string()),
            _ => return Err(format!("only one file or folder can be opened\n{USAGE}")),
        }
    }
    if single && target.is_none() {
        return Err(format!("single file mode needs a file\n{USAGE}"));
    }
    Ok(Args {
        target,
        log_level,
        single,
    })
}

/// `file:12:5` → (`file`, Some((12, 5))), unless a file with the full name exists.
fn split_position(arg: &str) -> (PathBuf, Option<(usize, usize)>) {
    if Path::new(arg).exists() {
        return (arg.into(), None);
    }
    match paths::strip_position(arg) {
        Some((path, at)) => (path.into(), Some(at)),
        None => (arg.into(), None),
    }
}

/// A file to open and an optional 1-based line and column.
type FileArg = (PathBuf, Option<(usize, usize)>);

/// Workspace root, plus the file to open (if a file was given).
fn resolve_target(target: Option<&str>) -> Result<(PathBuf, Option<FileArg>), String> {
    let Some(target) = target else {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        return Ok((workspace::normalize(&cwd), None));
    };
    let (path, goto) = split_position(target);
    let path = workspace::normalize(&path);
    if path.is_dir() {
        return Ok((path, None));
    }
    Ok((workspace::root_for_file(&path), Some((path, goto))))
}

/// Single file mode's root and file: the file must not be a folder; a missing one (its folder
/// existing) opens empty and is created on save.
fn resolve_single(target: &str) -> Result<(PathBuf, FileArg), String> {
    let (path, goto) = split_position(target);
    let path = if path.exists() {
        workspace::normalize(&path)
    } else {
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let Some(name) = path.file_name() else {
            return Err(format!("{}: not a file", path.display()));
        };
        if !dir.is_dir() {
            return Err(format!("{}: no such folder", dir.display()));
        }
        workspace::normalize(dir).join(name)
    };
    if path.is_dir() {
        return Err(format!(
            "{}: single file mode opens a file, not a folder",
            path.display()
        ));
    }
    Ok((workspace::root_for_file(&path), (path, goto)))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    let tool_logs = logs::Logs::default();
    let log = match logging::init(args.log_level.as_deref(), tool_logs.clone()) {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("jed: logging disabled: {e}");
            None
        }
    };
    let mode = if args.single {
        app::Mode::SingleFile
    } else {
        app::Mode::Workspace
    };
    let resolved = match args.target.as_deref() {
        Some(target) if args.single => resolve_single(target).map(|(root, file)| (root, Some(file))),
        target => resolve_target(target),
    };
    let (root, file) = match resolved {
        Ok(r) => r,
        Err(e) => {
            eprintln!("jed: {e}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(version = env!("JED_VERSION"), root = %root.display(), single = args.single, "starting");

    let user_settings = config::settings::load(&paths::config_dir().join(config::settings::FILE_NAME));
    // Syntax plugins: only the user's own directory, never the workspace's.
    let plugin_errors = syntax::plugin::load_and_register(&paths::config_dir().join("languages"));
    // Needs the plugins: `[lsp.<id>]` may name one.
    let (user_servers, server_errors) = lsp::config::user_servers(&user_settings.lsp);
    let user_tasks = tasks::templates::load(&paths::config_dir().join(tasks::templates::FILE_NAME));
    let workspace = Workspace::load(root).with_user_settings(user_settings.overrides);
    // Load before taking over the terminal, so errors print normally. A binary file opens in a
    // hex tab.
    let mut initial_hex = None;
    let single_file = file
        .as_ref()
        .filter(|_| mode.is_single())
        .map(|(path, _)| path.as_path());
    let identity = terminal::Identity::new(&workspace.root, single_file, &paths::home());
    let initial = match file {
        Some((path, goto)) => match Buffer::open_with(&path, workspace.settings().indentation()) {
            Ok(buffer) => Some(app::InitialFile { buffer, goto }),
            // Single file mode: a new file, created on save.
            Err(buffer::file::LoadError::Io(e)) if mode.is_single() && e.kind() == std::io::ErrorKind::NotFound => {
                Some(app::InitialFile {
                    buffer: Buffer::new_file(&path),
                    goto,
                })
            }
            Err(buffer::file::LoadError::Binary(bytes)) => {
                if let Err(e) = hex::check_size(bytes.len() as u64) {
                    eprintln!("jed: {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
                initial_hex = Some((path, hex::HexDoc::new(bytes)));
                None
            }
            Err(e) => {
                eprintln!("jed: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let trust_store = TrustStore::load(TrustStore::default_path());
    let platform = Platform::current();
    let (keymap, keymap_errors) = Keymap::load(&paths::config_dir().join(keymap::FILE_NAME), platform);

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("jed: can't start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    panic::install();
    let kitty = terminal::kitty_supported();
    tracing::info!(kitty, "keyboard protocol");
    let cursor = terminal::CursorShape::of(&workspace.settings());
    let result = terminal::enter(kitty, cursor, &identity)
        .map_err(anyhow::Error::from)
        .and_then(|mut term| {
            runtime.block_on(async {
                let mut app = app::App::new_in(mode, workspace, trust_store, initial, kitty);
                if let Some((path, doc)) = initial_hex {
                    app.open_initial_hex(path, doc);
                }
                app.tasks.user = user_tasks.tasks;
                app.lsp.builtins.user = user_servers;
                let mut errors = user_settings.errors;
                errors.extend(server_errors);
                errors.extend(user_tasks.errors);
                errors.extend(plugin_errors);
                errors.extend(keymap_errors);
                app.set_keymap(keymap, errors);
                app.log = log;
                app.logs = tool_logs;
                app.run(&mut term).await
            })
        });
    terminal::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            eprintln!("jed: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_in_arguments() {
        assert_eq!(split_position("a.rs:12:5"), ("a.rs".into(), Some((12, 5))));
        assert_eq!(split_position("a.rs:12"), ("a.rs".into(), Some((12, 1))));
        assert_eq!(split_position("a.rs"), ("a.rs".into(), None));
        assert_eq!(split_position("a:b.rs:3"), ("a:b.rs".into(), Some((3, 1))));
        assert_eq!(split_position("a.rs:x"), ("a.rs:x".into(), None));
        assert_eq!(split_position(":3"), (":3".into(), None));
    }

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args_from(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn single_file_flag() {
        for flag in ["-s", "--single"] {
            let args = parse(&[flag, "msg.txt:3"]).unwrap();
            assert!(args.single);
            assert_eq!(args.target.as_deref(), Some("msg.txt:3"));
        }
        assert!(!parse(&["a.rs"]).unwrap().single);
        assert!(parse(&["-s"]).is_err());
    }

    #[test]
    fn single_file_targets() {
        let dir = tempfile::tempdir().unwrap();
        let root = workspace::normalize(dir.path());
        std::fs::write(root.join(workspace::FILE_NAME), "").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        // A new file: its root is still the nearest `.workspace`.
        let new = root.join("sub/.new");
        let (r, (path, goto)) = resolve_single(&format!("{}:2", new.display())).unwrap();
        assert_eq!((r, path, goto), (root.clone(), new, Some((2, 1))));
        assert!(resolve_single(root.join("sub").to_str().unwrap()).is_err());
        assert!(resolve_single(root.join("nowhere/x").to_str().unwrap()).is_err());
    }
}
