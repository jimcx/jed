//! TUI building blocks shared by jed and jmux. Present so far:
//! - `paths`: per-user directories (named by the app), `~` display, `file:line:col` positions
//! - `url`: web links in text, opening one in the browser
//! - `socket`: where a process's unix socket lives, and creating its private directory
//! - `clicks`: the single / double / triple click counter
//! - `clipboard`: the system clipboard (arboard / OSC 52) and base64
//! - `terminal`: host terminal setup (raw mode, kitty keyboard, pointer and cursor shapes, window
//!   title and user vars)
//! - `scrollbar`: the scroll bar model; `scrollbar_draw`: drawing bars and marks; `style`: the `Styles`
//!   trait the drawing reads colours through
//! - `field`: `TextField`; `field_draw`: drawing one; `text`: its grapheme and word steps on a `&str`
//! - `context_menu`: the menu model, generic over the command type
//! - `split`: the split tree (`SplitTree`, `Divider`, `SplitDir`, `DropSide`); `borders`: the border layer
//!   (junction glyphs where lines meet)
//! - `theme`: `Theme` (key -> `Style`, 256-colour fallback, terminal cell mapping) and `widget_entries`,
//!   the keys the shared widgets read
//! - `keymap`: `KeyCombo`, `Platform`, and `Keymap<Ctx, Cmd>` with the user keybindings file reader,
//!   generic over the app's `KeyContext` / `KeyCommand` types
//!
//! - `tabs`: `TabMetrics`, a tab bar's rows, padding and target widths (normal and touch)
//! - `stream_backend`: `StreamBackend`, a ratatui `Backend` writing into a byte buffer, sized by the caller
//!   (a server drawing for a remote client)
//!
//! Shared code names no application: directory names, env variable names and window titles are
//! passed in. See docs/dev/crates.md.

pub mod borders;
pub mod clicks;
pub mod clipboard;
pub mod context_menu;
pub mod field;
pub mod field_draw;
pub mod keymap;
pub mod paths;
pub mod scrollbar;
pub mod scrollbar_draw;
pub mod socket;
pub mod split;
pub mod stream_backend;
pub mod style;
pub mod tabs;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod url;
