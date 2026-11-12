//! TUI building blocks shared by jed and jmux. Present so far:
//! - `paths`: per-user directories (named by the app), `~` display, `file:line:col` positions
//! - `url`: web links in text, opening one in the browser
//! - `socket`: where a process's unix socket lives, and creating its private directory
//! - `clipboard`: the system clipboard (arboard / OSC 52) and base64
//! - `terminal`: host terminal setup (raw mode, kitty keyboard, pointer and cursor shapes, window
//!   title and user vars)
//!
//! Still to move in: the split tree and dividers, border junctions, the scroll bar, `TextField`,
//! the context menu, `Theme` and `Keymap` machinery, and a ratatui backend that draws into a byte
//! stream.
//!
//! Shared code names no application: directory names, env variable names and window titles are
//! passed in. See docs/dev/crates.md.

pub mod clipboard;
pub mod paths;
pub mod socket;
pub mod terminal;
pub mod url;
