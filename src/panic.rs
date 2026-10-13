//! Panic hook: back up unsaved work, restore the terminal, log, then exit.

use std::backtrace::Backtrace;
use std::path::PathBuf;
use std::sync::{Mutex, TryLockError};

use ropey::Rope;

use crate::buffer::file;
use crate::{logging, paths, terminal};

/// A dirty buffer's text, refreshed by the app after each event (rope clones are cheap).
pub struct Snapshot {
    pub path: PathBuf,
    pub rope: Rope,
    pub bom: bool,
}

static DIRTY: Mutex<Vec<Snapshot>> = Mutex::new(Vec::new());

pub fn set_dirty(snapshots: Vec<Snapshot>) {
    if let Ok(mut d) = DIRTY.lock() {
        *d = snapshots;
    }
}

/// Install before entering raw mode.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let backtrace = Backtrace::force_capture();
        let backups = back_up_dirty();
        terminal::restore();
        tracing::error!(target: "panic", "{info}\n{backtrace}");
        eprintln!("red crashed: {info}\n\n{backtrace}");
        for path in backups {
            eprintln!("Unsaved changes backed up to {}", path.display());
        }
        eprintln!("Logs: {}", logging::log_dir().display());
        // Also covers panics in background threads, which would otherwise leave the UI running.
        std::process::exit(101);
    }));
}

/// Best effort: never blocks, never panics.
fn back_up_dirty() -> Vec<PathBuf> {
    let guard = match DIRTY.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return Vec::new(),
    };
    let dir = paths::state_dir().join("backup");
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
    guard
        .iter()
        .filter_map(|s| {
            let name = s.path.to_string_lossy().replace('/', "%");
            let dest = dir.join(format!("{name}.{}.bak", std::process::id()));
            std::fs::write(&dest, file::encode(&s.rope, s.bom)).ok().map(|_| dest)
        })
        .collect()
}
