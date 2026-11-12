//! TUI building blocks shared by jed and jmux. Nothing is here yet; it will hold:
//! host terminal setup (raw mode, kitty keyboard, pointer, cursor shape), clipboard (OSC 52 /
//! arboard), per-user dirs and sockets, the split tree and dividers, border junctions, the scroll
//! bar model and drawing, `TextField`, the context menu, `Theme` and `Keymap` machinery, and a
//! ratatui backend that draws into a byte stream.
//!
//! Shared code names no application: directory names, env variable names and window titles are
//! passed in. See docs/dev/crates.md.
