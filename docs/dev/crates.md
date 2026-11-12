# Crates

The repository is one cargo workspace (root `Cargo.toml`): jed is the root package (`src/`, the
default member, so plain `cargo build --release` builds `target/release/jed`); the libraries are
`crates/*`; `jmux/` (a tmux-like multiplexer, design in `jmux/PLAN.md`) joins as a member in its own task.

| Crate | Path | Holds |
| --- | --- | --- |
| `jtui` | `crates/jtui` | TUI building blocks: host terminal setup, clipboard, per-user dirs and sockets, split tree, border junctions, scroll bars, `TextField`, context menu, `Theme` / `Keymap` machinery, a byte-stream ratatui backend |
| `jterm` | `crates/jterm` | The terminal engine: PTY run, `vt100` screen and scrollback, command records, selection, search, key / mouse encoding, shell tab model, shell integration snippets, drawing a screen |
| `jed` | `.` | The editor |
| `jmux` | `jmux` | The multiplexer (later) |

`jterm` depends on `jtui`; neither depends on jed or jmux.

## Rules

- **Shared code names no application:** directory names, env variable names and window titles are
  passed in by the app, never written in a crate.
- **Not shared** (each app has its own): `App`, the event loop, mouse dispatch, `HitMap`, tabs and
  groups, the palette, sessions, settings contents, default key bindings, theme colours.
- **Moving a jed module into a crate:** move the file to `crates/<c>/src/`, `pub mod` it in that
  crate's `lib.rs`, and leave a re-export at the old path in jed (`pub use jtui::scrollbar;` in
  `main.rs`, or `pub use jtui::scrollbar::*;` in a stub module) so `crate::scrollbar::…` keeps working.
- **Dependencies:** versions shared by several members live in the root `[workspace.dependencies]`;
  members use `{ workspace = true }`. `edition.workspace = true`.
- **Root config covers every member:** `rustfmt.toml`, `rust-toolchain.toml`, `[patch.crates-io]`
  (crossterm and vt100 forks) and `[profile.*]`. `build.rs` and `JED_VERSION` are jed's alone.

## Tests

- A crate's unit tests sit at the bottom of the module in `crates/<c>/src/`; integration tests in
  `crates/<c>/tests/` only when a module's tests can't do.
- jed's end-to-end tests stay in `src/`; jmux's go in `jmux/src/<task id>_tests.rs`, named `<task id>_*`.
- `scripts/check` runs fmt, clippy and tests for the whole workspace (`--workspace`); `scripts/check test <filter>`
  finds a test in any member. CI does the same.
