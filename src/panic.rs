//! Panic hook: back up unsaved work, restore the terminal, log, then exit.

use std::backtrace::Backtrace;
use std::path::PathBuf;
use std::sync::{Mutex, TryLockError};

use crate::backup::{self, Snapshot};
use crate::{logging, terminal};

/// Where the backups go and the dirty buffers' texts, refreshed by the app after each event
/// loop turn (rope clones are cheap).
struct Dirty {
    state_dir: PathBuf,
    root: PathBuf,
    snapshots: Vec<Snapshot>,
}

/// The task socket's path while jed listens on it; the hook removes the file.
static IPC_SOCKET: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Records (or with `None`, forgets) the socket file to remove if jed panics.
pub fn set_ipc_socket(path: Option<PathBuf>) {
    if let Ok(mut p) = IPC_SOCKET.lock() {
        *p = path;
    }
}

fn remove_ipc_socket() {
    if let Ok(p) = IPC_SOCKET.try_lock()
        && let Some(path) = p.as_ref()
    {
        crate::ipc::remove_socket(path);
    }
}

static DIRTY: Mutex<Option<Dirty>> = Mutex::new(None);

/// The dirty buffers to back up if jed panics, into `state_dir`'s backups of the workspace at
/// `root` (the same files `backup` writes periodically).
pub fn set_dirty(state_dir: PathBuf, root: PathBuf, snapshots: Vec<Snapshot>) {
    if let Ok(mut d) = DIRTY.lock() {
        *d = Some(Dirty {
            state_dir,
            root,
            snapshots,
        });
    }
}

/// Install before entering raw mode.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let backtrace = Backtrace::force_capture();
        let backups = back_up_dirty();
        remove_ipc_socket();
        terminal::restore();
        tracing::error!(target: "panic", "{info}\n{backtrace}");
        eprintln!("jed crashed: {info}\n\n{backtrace}");
        for path in backups {
            eprintln!("Unsaved changes backed up to {}", path.display());
        }
        eprintln!("Logs: {}", logging::log_dir().display());
        // Also covers panics in background threads, which would otherwise leave the UI running.
        std::process::exit(101);
    }));
}

/// Best effort: never blocks on the lock, never panics. `backup::write`, so the next start
/// offers them like the periodic backups.
pub(crate) fn back_up_dirty() -> Vec<PathBuf> {
    let guard = match DIRTY.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return Vec::new(),
    };
    let Some(dirty) = guard.as_ref() else {
        return Vec::new();
    };
    let time = crate::history::now_ms();
    dirty
        .snapshots
        .iter()
        .filter_map(|s| backup::write(&dirty.state_dir, &dirty.root, s, time).ok())
        .collect()
}
