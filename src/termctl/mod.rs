//! Talking to the terminal jed runs in, to find the other jed windows and raise or open one
//! (the "Open Recent Workspace" picker). No app state, nothing drawn: `app/recent.rs` drives it.
//!
//! The seam is [`Backend`]; [`detect`] tries each terminal in turn. Adding a terminal is a
//! `termctl/<name>.rs` with a `Backend` impl and one line in `detect`; `recent.rs` and
//! `app/recent.rs` don't change. Every `Backend` method blocks: call from `spawn_blocking`.

use std::path::{Path, PathBuf};

pub mod kitty;

/// The variables [`detect`] reads, as a plain struct so tests pass their own (no `set_var`).
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// `KITTY_LISTEN_ON`: kitty's remote control socket, exported to every child when kitty
    /// runs with `listen_on`.
    pub kitty_listen_on: Option<String>,
}

impl Env {
    /// The process's environment.
    pub fn from_process() -> Self {
        Self {
            kitty_listen_on: std::env::var("KITTY_LISTEN_ON").ok().filter(|s| !s.is_empty()),
        }
    }
}

/// A terminal window or pane, opaque to everything but the backend that made it (kitty's
/// numeric window id, another terminal's pane id).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowRef(pub String);

/// A terminal window running jed on `workspace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub workspace: PathBuf,
    pub id: WindowRef,
}

/// One terminal's way of listing, raising and opening windows. All methods block.
pub trait Backend: Send + Sync {
    /// For logs and messages ("kitty").
    fn name(&self) -> &str;
    /// Blocking: the windows running jed, with their workspaces.
    fn windows(&self) -> Result<Vec<Window>, String>;
    /// Blocking: raises the window (and its OS window / tab).
    fn focus(&self, window: &WindowRef) -> Result<(), String>;
    /// Blocking: a new terminal window running `jed <workspace>` in that folder.
    fn launch(&self, workspace: &Path) -> Result<(), String>;
}

/// The backend for the terminal described by `env`, if any.
pub fn detect(env: &Env) -> Option<Box<dyn Backend>> {
    kitty::detect(env)
}
