//! The terminal engine shared by jed and jmux. Nothing is here yet; it will hold: PTY run, the
//! `vt100` screen and scrollback, command records, selection, search, key and mouse encoding,
//! the shell tab model, shell integration snippets, and drawing a screen into a ratatui buffer.
//!
//! Depends on `jtui`. Shared code names no application: directory names, env variable names and
//! window titles are passed in. See docs/dev/crates.md.
