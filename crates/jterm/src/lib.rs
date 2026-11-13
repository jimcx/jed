//! The terminal engine shared by jed and jmux: `run` (a process in a PTY, its `vt100` screen and
//! scrollback, command records), `select` (drag selection), `search`, `keys` and `mouse` (encoding
//! as a terminal sends them), `spec` (what a run starts from), `shell` (the shell tab model) and
//! `shell_integration` (the snippets, rendered from the app's `Names`) and `draw` (a screen, its
//! selection, search and cursor into a ratatui buffer, over `jtui::Theme`).
//!
//! Depends on `jtui`. Shared code names no application: directory names, env variable names
//! (`run::EnvNames`) and window titles are passed in. See docs/dev/crates.md.

pub mod draw;
pub mod keys;
pub mod mouse;
pub mod run;
pub mod search;
pub mod select;
pub mod shell;
pub mod shell_integration;
pub mod spec;
