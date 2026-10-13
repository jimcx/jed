mod app;
mod buffer;
mod command;
mod config;
mod edit;
mod logging;
#[cfg(test)]
mod milestone_tests;
mod panic;
mod paths;
mod selection;
mod terminal;
mod theme;
mod ui;
mod view;

use std::path::PathBuf;
use std::process::ExitCode;

use buffer::Buffer;

const USAGE: &str = "usage: red [--log-level <filter>] <file>";

struct Args {
    file: PathBuf,
    log_level: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut file = None;
    let mut log_level = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "-V" | "--version" => return Err(format!("red {}", env!("CARGO_PKG_VERSION"))),
            "--log-level" => log_level = Some(args.next().ok_or("--log-level needs a value")?),
            a if a.starts_with('-') => return Err(format!("unknown option {a}\n{USAGE}")),
            a if file.is_none() => file = Some(PathBuf::from(a)),
            _ => return Err(format!("only one file can be opened for now\n{USAGE}")),
        }
    }
    Ok(Args {
        file: file.ok_or(USAGE)?,
        log_level,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = logging::init(args.log_level.as_deref()) {
        eprintln!("red: logging disabled: {e}");
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), file = %args.file.display(), "starting");

    // Load before taking over the terminal, so errors print normally.
    let buffer = match Buffer::open(&args.file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("red: {}: {e}", args.file.display());
            return ExitCode::FAILURE;
        }
    };

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
            let mut app = app::App::new(buffer, kitty);
            runtime.block_on(app.run(&mut term))
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
