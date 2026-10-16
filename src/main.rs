mod app;
mod brackets;
mod buffer;
mod clipboard;
mod column;
mod command;
mod comment;
mod completion;
mod config;
mod diagnostics;
mod disk;
mod edit;
mod explorer;
mod field;
mod find;
mod format;
mod git;
mod group;
mod grouped;
mod hover;
mod indent;
mod layout;
mod lines;
mod locations;
mod logging;
mod lsp;
#[cfg(test)]
mod milestone10_tests;
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
mod pairs;
mod palette;
mod panic;
mod paths;
mod problems;
mod quick_diff;
mod rename;
mod search;
mod search_view;
mod selection;
mod syntax;
mod terminal;
mod theme;
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

const USAGE: &str = "usage: red [--log-level <filter>] [<folder> | <file>[:<line>[:<col>]]]";

struct Args {
    target: Option<String>,
    log_level: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut target = None;
    let mut log_level = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "-V" | "--version" => return Err(format!("red {}", env!("CARGO_PKG_VERSION"))),
            "--log-level" => log_level = Some(args.next().ok_or("--log-level needs a value")?),
            a if a.starts_with('-') => return Err(format!("unknown option {a}\n{USAGE}")),
            a if target.is_none() => target = Some(a.to_string()),
            _ => return Err(format!("only one file or folder can be opened\n{USAGE}")),
        }
    }
    Ok(Args { target, log_level })
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

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    let log = match logging::init(args.log_level.as_deref()) {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("red: logging disabled: {e}");
            None
        }
    };
    let (root, file) = match resolve_target(args.target.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("red: {e}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), root = %root.display(), "starting");

    // Load before taking over the terminal, so errors print normally.
    let initial = match file {
        Some((path, goto)) => match Buffer::open(&path) {
            Ok(buffer) => Some(app::InitialFile { buffer, goto }),
            Err(e) => {
                eprintln!("red: {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let user_settings = config::settings::load(&paths::config_dir().join(config::settings::FILE_NAME));
    let workspace = Workspace::load(root).with_user_settings(user_settings.overrides);
    let trust_store = TrustStore::load(TrustStore::default_path());
    let platform = Platform::current();
    let (keymap, keymap_errors) = Keymap::load(&paths::config_dir().join(keymap::FILE_NAME), platform);

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("red: can't start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    panic::install();
    let kitty = terminal::kitty_supported();
    tracing::info!(kitty, "keyboard protocol");
    let result = terminal::enter(kitty)
        .map_err(anyhow::Error::from)
        .and_then(|mut term| {
            runtime.block_on(async {
                let mut app = app::App::new(workspace, trust_store, initial, kitty);
                let mut errors = user_settings.errors;
                errors.extend(keymap_errors);
                app.set_keymap(keymap, errors);
                app.log = log;
                app.run(&mut term).await
            })
        });
    terminal::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            eprintln!("red: {e:#}");
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
}
