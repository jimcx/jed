//! `Spec`: what a run needs to start (argv, cwd, env), built by the app from its own templates.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A template made ready to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub name: String,
    /// The template's command line, with its `{{NAME}}`s filled in.
    pub command: String,
    /// `[shell, "-c", command]`.
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
    /// Set on top of the app's own environment, in order (the template's last, so it wins).
    pub env: Vec<(String, String)>,
    /// Test hook (always compiled: `cfg(test)` doesn't cross crates): when set, the off-thread spawn blocks on it, keeping the run `Starting` so a
    /// test can quit while the shell is still starting (rather than racing the spawn).
    pub start_gate: Option<StartGate>,
}

/// A test hook: a latch the spawn waits on (keeping a run `Starting`) until the test drops the
/// matching [`StartGateHandle`]. `Clone`/`Debug`/`PartialEq` so `Spec` keeps its derives.
#[derive(Clone, Debug)]
#[doc(hidden)]
pub struct StartGate {
    released: Arc<Mutex<bool>>,
}

/// The test's end of a [`StartGate`]: dropping it releases the waiting spawn.
#[doc(hidden)]
pub struct StartGateHandle {
    released: Arc<Mutex<bool>>,
}

impl StartGate {
    /// The gate (to put on a `Spec`), and the handle a test drops to release the spawn.
    pub fn new() -> (StartGate, StartGateHandle) {
        let released = Arc::new(Mutex::new(false));
        (
            StartGate {
                released: released.clone(),
            },
            StartGateHandle { released },
        )
    }

    /// Blocks (on the spawn's thread) until the test drops the handle.
    pub fn wait(&self) {
        while !*self.released.lock().unwrap() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl PartialEq for StartGate {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Drop for StartGateHandle {
    fn drop(&mut self) {
        *self.released.lock().unwrap() = true;
    }
}
