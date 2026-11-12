//! TUI building blocks shared by jed and jmux. Present so far:
//! - `paths`: per-user directories (named by the app), `~` display, `file:line:col` positions
//! - `url`: web links in text, opening one in the browser
//! - `socket`: where a process's unix socket lives, and creating its private directory
//! - `clipboard`: the system clipboard (arboard / OSC 52) and base64
//! - `terminal`: host terminal setup (raw mode, kitty keyboard, pointer and cursor shapes, window
//!   title and user vars)
//! - `scrollbar`: the scroll bar model; `scrollbar_draw`: drawing bars and marks; `style`: the `Styles`
//!   trait the drawing reads colours through
//! - `field`: `TextField`; `text`: its grapheme and word steps on a `&str`
//! - `context_menu`: the menu model, generic over the command type
//! - `split`: the split tree (`SplitTree`, `Divider`, `SplitDir`, `DropSide`); `borders`: the border layer
//!   (junction glyphs where lines meet)
//! - `theme`: `Theme` (key -> `Style`, 256-colour fallback, terminal cell mapping) and `widget_entries`,
//!   the keys the shared widgets read
//! - `keymap`: `KeyCombo`, `Platform`, and `Keymap<Ctx, Cmd>` with the user keybindings file reader,
//!   generic over the app's `KeyContext` / `KeyCommand` types
//!
//! Still to come: a ratatui backend that draws into a byte stream.
//!
//! Shared code names no application: directory names, env variable names and window titles are
//! passed in. See docs/dev/crates.md.

pub mod borders;
pub mod clipboard;
pub mod context_menu;
pub mod field;
pub mod keymap;
pub mod paths;
pub mod scrollbar;
pub mod scrollbar_draw;
pub mod socket;
pub mod split;
pub mod style;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod url;
