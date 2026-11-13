//! The terminal engine shared by jed and jmux: `run` (a process in a PTY, its `vt100` screen and
//! scrollback, command records), `select` (drag selection), `keys` and `mouse` (encoding as a
//! terminal sends them) and `spec` (what a run starts from). Still to come: search, the shell tab
//! model, shell integration snippets, and drawing a screen into a ratatui buffer.
//!
//! Depends on `jtui`. Shared code names no application: directory names, env variable names
//! (`run::EnvNames`) and window titles are passed in. See docs/dev/crates.md.

pub mod keys;
pub mod mouse;
pub mod run;
pub mod select;
pub mod spec;
