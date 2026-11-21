# jmux — terminal multiplexer

A tmux-like program built from jed's terminal and window layout. It lives in jed's repository as a
second binary and shares code with jed through two crates. Tasks are in `../tasks/` (`crates_workspace`,
`jtui`, `jterm`, `jmux_*`), built with `/task` like jed's.

## Goals
- Named sessions that outlive the terminal window: detach, close the window, attach again later
- jed's window layout: tabs, groups, splits, drag and drop, a resizable side bar, a status bar
- jed's terminals unchanged: same scrollback (zstd-compressed), selection, search, links, scroll bar
- A side bar listing the commands that are running and that ran, each a click away
- One code base for the terminal engine and TUI building blocks, so a fix lands in jed and jmux at once

## Non-goals
- tmux compatibility (its commands, config or key table)
- Restoring a session after its server process ends (reboot, crash, the last terminal closing)
- Bringing back a closed terminal
- Several clients attached to one session at once (first version: a new attach takes the session over)
- A prefix key; jmux binds keys directly, as jed does

## Crates
One cargo workspace at the repository root; jed is the `jed/` package, the root a virtual workspace.

| Crate | Path | Holds |
|---|---|---|
| `jtui` | `crates/jtui` | Host terminal setup (raw mode, kitty keyboard, pointer, cursor shape), clipboard (OSC 52 / arboard), per-user dirs and sockets, the split tree and dividers, border junctions, scroll bar model and drawing, `TextField`, context menu, `Theme` and `Keymap` machinery, a ratatui backend that draws into a byte stream |
| `jterm` | `crates/jterm` | A terminal: PTY run, `vt100` screen and scrollback, command records, selection, search, key and mouse encoding, the shell tab model, shell integration snippets, drawing a screen into a ratatui buffer |
| `jed` | `.` | The editor; re-exports what moved so `crate::tasks::…` paths keep working |
| `jmux` | `jmux` | This program |

- `jterm` depends on `jtui`; neither depends on jed or jmux
- Shared code names no application: env variable names, directory names and window titles are passed in
- Not shared (each app has its own): `App`, the event loop, mouse dispatch, `HitMap`, tabs and groups (`Tab` is a closed enum in jed), palette, sessions, settings contents, default key bindings, theme colours
- The crossterm and vt100 forks stay in the root `[patch.crates-io]`, which covers every member

## Architecture

### Processes
- **Server**: one process per session, detached from the terminal that started it. Owns every PTY, screen and scrollback, the layout and the whole `App`. Listens on a Unix socket named after the session
- **Client**: what `jmux` runs in the user's terminal. Puts the terminal in raw mode, sends input events and its size to the server, writes the bytes the server sends back. Holds no session state
- The server draws with ratatui into a byte stream (`jtui`'s stream backend, sized by the client's last resize) instead of a tty. So a `jterm::Run` is used exactly as jed uses it and nothing about a screen is serialised
- One process per session: a crash loses one session, `jmux ls` is a listing of sockets, and the server exits when its last terminal closes

### Socket and protocol
- Socket dir: `$XDG_RUNTIME_DIR/jmux`, else `$TMPDIR/jmux-<uid>`; mode `0700`, refused when owned by someone else (jed's `ipc::prepare_dir` rules, moved to `jtui`)
- Socket file: `<session name>.sock`. Names are `[A-Za-z0-9_.-]+`; an unnamed session gets the lowest free number
- Frames: a 4-byte length, then a JSON message, except output, whose payload is raw bytes
- Client to server: `Hello { version, rows, cols, truecolor, term, touch }` (`touch` is `#[serde(default)]`: protocol version 1 either way, an old server draws the normal layout), `Input(event)` (crossterm's event type through its `serde` feature), `Resize { rows, cols }`, `Detach`
- Server to client: `Welcome { session }`, `Output(bytes)`, `Bye { reason }` (detached, session ended, taken over, version mismatch)
- A stale socket (nothing listening) is removed by `ls` and by `new`

### Attach and detach
- `jmux` / `jmux attach [-t name]`: connect, send `Hello`; the server redraws everything for the new size
- Detach: the detach command (default Ctrl+Shift+D), the client's terminal closing (SIGHUP, read error), or another client attaching. The server keeps running
- The session ends when its last terminal closes or with `jmux kill-session`; the client then prints why and exits

### Clipboard
- The server may outlive the display, so copies go out as OSC 52 in the output stream; paste arrives as a bracketed paste event

### Server `App`
- Same shape as jed's: `App` owns all state, `execute(Command)`, `ui::render(f, &mut App)` writing a `HitMap`, keymaps as data, colours only through the theme, nothing blocking on the event loop
- Events: client messages, `jterm` run events (output throttled to one per frame), timers

## Layout
```
┌──────────────┬─────────────────────────────────────────┐
│ Commands     │ zsh ○ │ make ● │ zsh (2) ○              │
│ ● make       ├────────────────────────┬────────────────┤
│ ✓ git status │                        │                │
│ ✗ cargo test │   terminal             │   terminal     │
│              │                        │                │
├──────────────┴────────────────────────┴────────────────┤
│ session: work   3 terminals                 Ctrl+Shift+D │
└────────────────────────────────────────────────────────┘
```
- Side bar on the left (toggle, resize by dragging its border), terminal groups in a split tree, a status bar
- Every tab is a shell; a group with no tabs is removed, and the last terminal closing ends the session

## Features

### Sessions
- `jmux` — attach to the most recently used session, or start one when there are none
- `jmux new [-s name] [-c dir] [--touch]`, `jmux attach [-t name] [--touch]` (`--touch`: the touch layout, three-row padded tab bars ending in a `+` that opens a tab in that group, for as long as that client stays attached; bare `jmux --touch` too), `jmux ls`, `jmux kill-session -t name`, `jmux rename-session [-t name] new`
- `jmux ls` shows name, terminal count, running commands, created time, attached or not
- Starting `jmux` inside a jmux terminal (`$JMUX` set) is refused with a message

### Terminals, tabs and splits
- Touch gestures on a tab bar (`--touch`, rules in `jtui::touch`, driven by `app/touch.rs`): a press records a pending `Drag::Press` and starts a `HOLD` (450 ms) timer that sends `AppEvent::Hold(serial)`; motion past `SLOP` (1 cell) is a swipe that scrolls the bar column by column following the finger, so it can stop part way through a long tab (`Group::shift`; never focusing or closing); a release within `SLOP` is a tap, acted on at release; a hold on a tab lifts it (active, drawn `tab.lifted`) and it is then an ordinary tab drag. Wheel events still scroll; a plain client keeps press-and-drag. In a terminal (a press without modifiers) the same pending press: a swipe scrolls the scrollback by the rows moved, the text following the finger (a program that reports the mouse gets one wheel event per row instead), a tap replays the press and release (a click, double / triple tap a word / line), and a hold replays the press so a drag from there selects (or is reported to the program).
- New terminal, close terminal (confirm when a command is running), next / previous tab, go to the Nth tab
- Split right / down, focus the group left / right / up / down, drag a divider
- Drag a tab along its bar, to another group, or onto a group's edge to split there (jed's `drag_tabs` rules)
- Tab titles as jed's shell tabs: `zsh ○` at the prompt, `make ●` while it runs, a failed exit marked
- Inside a terminal everything jed's shell tabs do: mouse reporting, selection (drag, double / triple click, Shift+click), copy, paste, scrollback keys and wheel, scroll bar, search, link clicks
- Each shell gets `JMUX=<socket>` and `JMUX_TERMINAL=<id>`

### Commands side bar
- Needs the shell integration (OSC 133 / 633 / 7): "Install Shell Integration" writes the snippet and adds the line to the shell's init file, as jed does; a notice when a shell has none
- Rows, newest first: running commands, then finished ones. A row shows a state glyph (`●` running, `✓` exit 0, `✗` failed), the command line, the terminal's tab title, and how long it ran or how long ago it ended
- History length is a setting (`command_history`, default 500 per session); a closed terminal's commands stay in the list, dimmed
- Click or Enter on a row: focus that terminal's tab and scroll its scrollback so the command's first row is at the top (when the scrollback still holds it; otherwise only focus). A dimmed row does nothing
- Filter box at the top (a `TextField`): substring match on the command line
- Keys: Up / Down / PageUp / PageDown / Home / End, Enter, Esc back to the terminal

### Settings, keys and theme
- `~/.config/jmux/settings.toml`: `shell`, `scrollback`, `scrollback_compression`, `command_history`, `side_bar_width`, `cursor_style`, `tabs_at_bottom`
- `~/.config/jmux/keybindings.toml`, same format as jed's
- Default keys follow jed's terminal area (new terminal, split, focus group, move tab, scrollback, search, copy / paste) plus detach and "focus side bar"
- Built-in dark theme, jed's colours for the shared widgets

## Tests
- Unit tests at the bottom of each module
- End-to-end tests in `jmux/src/<task id>_tests.rs`, named `<task id>_*`: the server `App` driven headlessly (keys, mouse, `settle`, a `TestBackend`) as jed's are, with real shells where a test needs one
- Protocol tests run a server and a client in one process over a socket in a temp dir
- Never set `XDG_RUNTIME_DIR` or `XDG_CONFIG_HOME` in a test; pass dirs in

## Open questions
- Several clients on one session (smallest size wins, as tmux) — wanted later?
- Keep the command history after a session ends (a file per session), for a "past sessions" list?
- A command palette (jed's is tied to its pickers; jmux would need its own small one)
