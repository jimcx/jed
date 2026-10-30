mod app;
#[cfg(test)]
mod apple_langs_tests;
mod backup;
mod blame;
mod brackets;
mod buffer;
mod cheatsheet;
#[cfg(test)]
mod clear_scrollback_tests;
mod clipboard;
mod code_action;
mod column;
mod command;
mod comment;
mod commit_view;
#[cfg(test)]
mod compare_files_tests;
mod completion;
mod config;
mod conflict;
#[cfg(test)]
mod conflicts_tests;
mod diagnostics;
mod diff_caret;
#[cfg(test)]
mod diff_edit_tests;
mod diff_view;
mod disk;
#[cfg(test)]
mod drag_tabs_tests;
mod edit;
mod explorer;
mod field;
mod find;
#[cfg(test)]
mod fixes_tests;
#[cfg(test)]
mod focus_groups_tests;
mod format;
mod git;
mod git_editor;
#[cfg(test)]
mod git_editor_tests;
mod git_history;
#[cfg(test)]
mod git_history_tests;
#[cfg(test)]
mod git_marks_tests;
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
mod ipc;
mod keys;
#[cfg(test)]
mod keys_tests;
#[cfg(test)]
mod kitty_recent_tests;
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
mod mru;
mod multicursor;
mod navigation;
mod pairs;
mod palette;
mod panic;
mod path_filter;
mod paths;
mod perf;
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
mod shell;
mod shell_integration;
#[cfg(test)]
mod shell_tests;
mod shift;
#[cfg(test)]
mod soft_wrap_tests;
mod syntax;
#[cfg(test)]
mod syntax_plugins_tests;
mod tasks;
#[cfg(test)]
mod term_area_tests;
#[cfg(test)]
mod term_select_tests;
mod termctl;
mod terminal;
mod theme;
#[cfg(test)]
mod todo_tests;
#[cfg(test)]
mod tools_tests;
mod tools_view;
mod ui;
#[cfg(test)]
mod untitled_tests;
mod view;
mod watcher;
mod workspace;
mod workspace_search;
mod wrap;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use buffer::Buffer;
use config::keymap::{self, Keymap, Platform};
use workspace::Workspace;
use workspace::trust::TrustStore;

const USAGE: &str = "usage: jed [--log-level <filter>] [--perf] [<folder> | <file>[:<line>[:<col>]]]\n       \
                     jed [--log-level <filter>] [--perf] -s | --single | --wait <file>[:<line>[:<col>]]   (single file mode)\n       \
                     jed --new-language <id>   (start a syntax plugin in ~/.config/jed/languages/<id>)\n       \
                     jed --check-language <id> [--sample <file>]   (build, load and check a syntax plugin)";

/// A syntax plugin authoring command (`syntax::authoring`): prints and exits.
#[derive(Debug, PartialEq)]
enum Authoring {
    New(String),
    Check { id: String, sample: Option<PathBuf> },
}

#[derive(Debug, PartialEq)]
struct Args {
    target: Option<String>,
    log_level: Option<String>,
    /// `-s` / `--single`: single file mode (`app::Mode::SingleFile`).
    single: bool,
    /// `--wait`: editor mode on any file (git's `core.editor = "jed --wait"`); IPC will make it
    /// wait for a running jed.
    wait: bool,
    /// `--new-language` / `--check-language`: run instead of the editor.
    authoring: Option<Authoring>,
    /// `--perf`: log timings around file opens and git reads (target `perf`).
    perf: bool,
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut target = None;
    let mut log_level = None;
    let mut single = false;
    let mut wait = false;
    let mut authoring = None;
    let mut sample = None;
    let mut perf = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "-V" | "--version" => return Err(format!("jed {}", env!("JED_VERSION"))),
            "--log-level" => log_level = Some(args.next().ok_or("--log-level needs a value")?),
            "-s" | "--single" => single = true,
            "--wait" => wait = true,
            "--perf" => perf = true,
            "--new-language" => authoring = Some(Authoring::New(args.next().ok_or("--new-language needs an id")?)),
            "--check-language" => {
                let id = args.next().ok_or("--check-language needs an id")?;
                authoring = Some(Authoring::Check { id, sample: None });
            }
            "--sample" => sample = Some(PathBuf::from(args.next().ok_or("--sample needs a file")?)),
            a if a.starts_with('-') => return Err(format!("unknown option {a}\n{USAGE}")),
            a if target.is_none() => target = Some(a.to_string()),
            _ => return Err(format!("only one file or folder can be opened\n{USAGE}")),
        }
    }
    if let Some(Authoring::Check { sample: slot, .. }) = &mut authoring {
        *slot = sample.take();
    }
    if sample.is_some() {
        return Err(format!("--sample only goes with --check-language\n{USAGE}"));
    }
    if authoring.is_some() && (target.is_some() || single || wait) {
        return Err(format!(
            "--new-language and --check-language take no file or folder\n{USAGE}"
        ));
    }
    if (single || wait) && target.is_none() {
        return Err(format!("single file mode needs a file\n{USAGE}"));
    }
    Ok(Args {
        target,
        log_level,
        single,
        wait,
        authoring,
        perf,
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

/// Runs a plugin authoring command before the UI (or logging) exists: prints its text, then the
/// exit status says whether it worked.
fn run_authoring(command: Authoring) -> ExitCode {
    let dir = paths::config_dir().join("languages");
    match command {
        Authoring::New(id) => match syntax::authoring::new_language(&dir, &id) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("jed: {e}");
                ExitCode::FAILURE
            }
        },
        Authoring::Check { id, sample } => {
            let tools = syntax::build::Toolchain::from_env();
            let (text, ok) = syntax::authoring::check_language(&dir, &id, sample.as_deref(), &tools);
            print!("{text}");
            if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
    }
}

/// How jed starts: from the arguments, before anything is loaded.
struct Start {
    mode: app::Mode,
    root: PathBuf,
    file: Option<FileArg>,
    /// Editor mode on a git message file: the status bar text (`App::editor_hint`).
    hint: Option<&'static str>,
}

/// The mode, root and file for `args`. Editor mode (`--wait`, or a git message file without
/// any flag) is single file mode with the repository's work tree as the root.
///
/// Blocking (`git2`): call before the runtime and raw mode.
fn start_of(args: &Args) -> Result<Start, String> {
    let message = args
        .target
        .as_deref()
        .and_then(|t| git_editor::kind(&workspace::normalize(&split_position(t).0)));
    let single = args.single || args.wait || message.is_some();
    let mode = if single {
        app::Mode::SingleFile
    } else {
        app::Mode::Workspace
    };
    let (root, file) = match args.target.as_deref() {
        Some(target) if single => {
            let (root, file) = resolve_single(target)?;
            (git_editor::root(&file.0).unwrap_or(root), Some(file))
        }
        target => resolve_target(target)?,
    };
    Ok(Start {
        mode,
        root,
        file,
        hint: message.map(git_editor::Kind::hint),
    })
}

/// The socket in `jed_ipc` (`$JED_IPC`) and the request for `args`' file target (not a folder; a
/// missing file is sent too, single file mode makes it), if both exist.
fn ipc_request(args: &Args, jed_ipc: Option<std::ffi::OsString>) -> Option<(PathBuf, ipc::Request)> {
    let socket = PathBuf::from(jed_ipc.filter(|v| !v.is_empty())?);
    let (path, at) = split_position(args.target.as_deref()?);
    let path = workspace::normalize(&path);
    if path.is_dir() {
        return None;
    }
    let request = ipc::Request {
        open: path,
        line: at.map(|(l, _)| l),
        col: at.map(|(_, c)| c),
        wait: args.wait,
    };
    Some((socket, request))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    if let Some(command) = args.authoring {
        return run_authoring(command);
    }
    if args.perf {
        perf::enable();
    }
    // Started from a task terminal of a running jed: open the file there instead.
    let ipc_fallback = match ipc_request(&args, std::env::var_os(ipc::ENV)) {
        Some((socket, request)) => match ipc::client::open(&socket, &request) {
            ipc::client::Outcome::Done(code) => return ExitCode::from(code),
            ipc::client::Outcome::Fallback(reason) => Some(reason),
        },
        None => None,
    };
    let tool_logs = logs::Logs::default();
    let log = match logging::init(args.log_level.as_deref(), tool_logs.clone()) {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("jed: logging disabled: {e}");
            None
        }
    };
    let Start { mode, root, file, hint } = match start_of(&args) {
        Ok(start) => start,
        Err(e) => {
            eprintln!("jed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(reason) = ipc_fallback {
        tracing::info!(%reason, "JED_IPC unusable, starting normally");
    }
    let single = mode.is_single();
    tracing::info!(version = env!("JED_VERSION"), root = %root.display(), single, "starting");

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
    let identity = terminal::Identity::new(&workspace.root, single_file, &paths::home())
        .with_title(workspace.config.window.title.as_deref());
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
                let mut app = app::App::new_in(mode, workspace, trust_store, initial, identity, kitty);
                app.editor_hint = hint;
                app.startup_terminal = true;
                if let Some((path, doc)) = initial_hex {
                    app.open_initial_hex(path, doc);
                }
                app.tasks.user = user_tasks.tasks;
                app.lsp.builtins.user = user_servers;
                app.syntax_plugins = syntax::plugin::PluginId::all();
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
    fn perf_flag() {
        assert!(parse(&["--perf", "a.rs"]).unwrap().perf);
        assert!(parse(&["--perf"]).unwrap().perf);
        assert!(!parse(&["a.rs"]).unwrap().perf);
    }

    #[test]
    fn wait_flag() {
        let args = parse(&["--wait", "msg.txt"]).unwrap();
        assert!(args.wait && !args.single);
        assert_eq!(args.target.as_deref(), Some("msg.txt"));
        assert!(!parse(&["a.rs"]).unwrap().wait);
        assert!(parse(&["--wait"]).is_err());
    }

    #[test]
    fn ipc_request_for_file_targets_only() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("msg.txt");
        std::fs::write(&file, "").unwrap();
        let file = workspace::normalize(&file);
        let env = || Some(std::ffi::OsString::from("/run/jed/1.sock"));
        let target = |t: &Path| t.to_str().unwrap().to_string();

        let (socket, req) = ipc_request(&parse(&["--wait", &target(&file)]).unwrap(), env()).unwrap();
        assert_eq!(socket, PathBuf::from("/run/jed/1.sock"));
        let want = ipc::Request {
            open: file.clone(),
            line: None,
            col: None,
            wait: true,
        };
        assert_eq!(req, want);
        let with_pos = format!("{}:4:2", target(&file));
        let (_, req) = ipc_request(&parse(&["-s", &with_pos]).unwrap(), env()).unwrap();
        assert_eq!((req.line, req.col, req.wait), (Some(4), Some(2), false));
        assert_eq!(req.open, file);
        // A file that doesn't exist yet is sent too (absolute), for jed to create.
        let missing = tmp.path().join("new.txt");
        let (_, req) = ipc_request(&parse(&[&target(&missing)]).unwrap(), env()).unwrap();
        assert!(req.open.is_absolute() && req.open.ends_with("new.txt"));

        assert!(ipc_request(&parse(&[&target(&file)]).unwrap(), None).is_none());
        assert!(ipc_request(&parse(&[&target(&file)]).unwrap(), Some("".into())).is_none());
        assert!(ipc_request(&parse(&[]).unwrap(), env()).is_none(), "no target");
        assert!(
            ipc_request(&parse(&[&target(tmp.path())]).unwrap(), env()).is_none(),
            "a folder"
        );
    }

    #[test]
    fn authoring_flags() {
        assert_eq!(
            parse(&["--new-language", "lua"]).unwrap().authoring,
            Some(Authoring::New("lua".into()))
        );
        let check = |sample: Option<&str>| {
            Some(Authoring::Check {
                id: "lua".into(),
                sample: sample.map(PathBuf::from),
            })
        };
        assert_eq!(parse(&["--check-language", "lua"]).unwrap().authoring, check(None));
        assert_eq!(
            parse(&["--check-language", "lua", "--sample", "a.lua"])
                .unwrap()
                .authoring,
            check(Some("a.lua"))
        );
        assert_eq!(
            parse(&["--sample", "a.lua", "--check-language", "lua"])
                .unwrap()
                .authoring,
            check(Some("a.lua"))
        );
        assert!(parse(&["a.rs"]).unwrap().authoring.is_none());
    }

    #[test]
    fn authoring_flags_are_refused_when_misused() {
        assert!(parse(&["--new-language"]).is_err());
        assert!(parse(&["--check-language"]).is_err());
        assert!(parse(&["--check-language", "lua", "--sample"]).is_err());
        assert!(
            parse(&["--sample", "a.lua"])
                .unwrap_err()
                .contains("only goes with --check-language")
        );
        assert!(parse(&["--new-language", "lua", "--sample", "a"]).is_err());
        assert!(parse(&["--new-language", "lua", "file.rs"]).is_err());
        assert!(parse(&["--check-language", "lua", "-s"]).is_err());
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
