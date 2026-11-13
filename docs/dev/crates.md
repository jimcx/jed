# Crates

The repository is one cargo workspace (root `Cargo.toml`): jed is the root package (`src/`, the
default member, so plain `cargo build --release` builds `target/release/jed`); the libraries are
`crates/*`; `jmux/` (a tmux-like multiplexer, design in `jmux/PLAN.md`) is a member too.

| Crate | Path | Holds |
| --- | --- | --- |
| `jtui` | `crates/jtui` | TUI building blocks: host terminal setup, clipboard, per-user dirs and sockets, split tree, border junctions, scroll bars, `TextField`, context menu, `Theme` / `Keymap` machinery, a byte-stream ratatui backend |
| `jterm` | `crates/jterm` | The terminal engine: PTY run, `vt100` screen and scrollback, command records, selection, search, key / mouse encoding, shell tab model, shell integration snippets, drawing a screen |
| `jed` | `.` | The editor |
| `jmux` | `jmux` | The multiplexer (binary `jmux`): `protocol` (frames and messages), `session` (socket directory); server and client follow |

`jterm` depends on `jtui`; neither depends on jed or jmux.

`jtui` holds: `paths` (per-app XDG dirs, `~` display, `file:line:col`), `url`, `socket` (socket dir
for a pid), `clipboard` (arboard / OSC 52 / in-process `memory()`, `base64`), `terminal` (host terminal
setup, `Identity`, `CursorShape`), `scrollbar` (model) and `scrollbar_draw` (bars and marks, colours via the
`style::Styles` trait: `Theme` implements it), `field` (`TextField`) with `field_draw` (`draw_field`: one input row, scrolled, placeholder, selection) and `text` (its `&str` grapheme and word
steps), `context_menu` (generic over the command type; jed aliases it to `Command`), `split` (the split tree,
`Divider`, `SplitDir`, `DropSide`, `MIN_GROUP_*`; serde form is the session file's), `borders` (junction glyphs,
colours via `Styles`), `theme` (`Theme::new(entries, truecolor)`: `get` / `find` / `dim` / `ansi` /
`terminal_cell`, `rgb`, `squiggle`, 256-colour fallback; `widget_entries`: the keys the shared widgets read,
which an app's theme starts from; jed's `Theme` wraps it and adds the editor / syntax entries), `keymap`
(`KeyCombo`, `Platform`, `Source`, `Binding<Ctx, Cmd>`, `Keymap<Ctx, Cmd>` and the keybindings file reader,
generic over the app's `KeyContext` / `KeyCommand` types; defaults and the file name are passed in).
`stream_backend` (`StreamBackend::new(cols, rows)`: a ratatui `Backend` over `CrosstermBackend<Vec<u8>>` whose `size()` / `window_size()` come from `set_size`, never the tty, `get_cursor_position` the last one set; `take_output()` drains the bytes a server sends to its client). jed passes `crate::APP` where a name is needed.
Stayed in jed (tied to its tab kinds or tool logs): `logging.rs`, `palette.rs`, `group.rs`, `ui/tabs.rs`,
`HitMap` and the hit structs, `session.rs`; also jed's `Context`, `defaults`, command names and keybindings
file template.
Anything jed's tests rely on is compiled unconditionally in a crate: `cfg(test)` doesn't cross crates.

## jterm: the terminal engine

Modules: `spec` (`Spec`, `DEFAULT_ENV`, and the `StartGate` test latch), `run`, `keys`, `mouse`,
`select`, `search`, `shell`, `shell_integration`, `draw`. Log target of the moved modules is `jterm::run` etc. (was `jed::tasks::run`). The app passes what it names:
`run::EnvNames { ipc, terminal }` (jed: `JED_IPC`, `JED_TERMINAL`) and the command ring's size in `run::StartOptions`
(jed's `tasks::START_OPTIONS`). Test hooks jed uses are compiled always, `#[doc(hidden)]`: `Spec::start_gate`,
`run::NO_SPAWN`, `Run::test_hold_state` / `test_screen_free`, `select::test_parser`. Real-shell tests of `Run` are
`run/real_tests.rs` (a small `Tasks` stand-in builds the specs); jed's `tasks/mod.rs` tests keep what concerns `Tasks`.

`run` (`Run::start(id, spec, rows, cols, sink, scrollback, StartOptions)`: `Status::Starting`, the spawn (`openpty` + fork / exec, `portable-pty`; clears an inherited `EnvNames::ipc` variable before applying the spec's env, so a nested app's tasks never get the outer app's socket) in `spawn_blocking`, then per run a reader thread (PTY output → `vt100::Parser` with the `scrollback` setting's lines (`SCROLLBACK` 5000 by default), behind a mutex: `screen()` for drawing; `scrollbar_extent()` reads `scrollback_len()` for `TerminalScroll`, the run's scroll bar as a `Scrollable`; `vt100` is the jimcx fork of 0.18.1, path-patched in `Cargo.toml`, with `Screen::mark_prompt()` (the cursor row starts a prompt; the mark travels with the row into scrollback and is lost when it is cleared or trimmed) and `clear_before_prompt() -> bool` (drops everything above the last mark, false when none)), a waiter thread (`waitid(WNOWAIT)`, waits ≤ 250 ms for the reader's end of output, then reaps with the state locked: a kill never signals a reaped pid; `Exited` means the screen has all the output) and a tokio notifier task (at most one `Event::Output` per `FRAME`, 20 ms); `Event::{Started, Output, Ended}(RunId)` and `Event::Foreground { id, name, idle, error }` (the terminal's OSC codes through a `Sink` closure: `TitleCallback` on the run's parser — a program's OSC 0 / 2 window title, sanitized, and FinalTerm's `133;A` prompt start (`mark_prompt()` on the cursor row, no event), `133;B` preexec (a command runs; the snippet's OSC 0 names it) / `133;D;<code>` precmd (idle at the shell's base name; a non-zero `code` sets `error`); the same codes plus `633;E;<command line>` (VS Code escaping) and OSC 7 (`file://host/path`, the cwd) fill a ring of the last `StartOptions::command_ring` (jed: 50) `CommandRecord { id, command, cwd, started, started_at (wall clock), finished, exit, start_row, end_row }` (absolute rows: `scrolled_lines` + cursor row), read with `Run::commands()` and `Run::command_output(id, max_lines) -> CommandOutput { text, truncated, running }` (see conventions "Terminal command capture"); `Run::start` adds `EnvNames::terminal=<RunId>` next to a spec's `EnvNames::ipc` variable (and the spawn clears inherited ones); `status()` → `Status::{Starting, Running, Exited(Exit { code, signal, killed }), Failed(message)}` (missing cwd, spawn error); `pid()`; `resize(rows, cols)` (screen + PTY, SIGWINCH; a size change clears the mouse selection); `write(bytes)` (input, through a writer thread so a process that doesn't read never blocks; while `Starting` queued and sent first once it runs, dropped if it fails to start; false once ended); `write_mouse(bytes, motion)` (mouse reports, same thread: the writer takes everything queued at once, keeps only the newest of consecutive motion reports and drops all mouse reports if the screen's mouse mode is `None` by write time, so a backlog under load can't leak `35;12;4M` into the shell after the program exits); `scroll(lines)` / `scroll_to(top)` (the vt100 scrollback view, 0 = live; the mouse selection stays on its text); `clear()` (Terminal / Tasks: Clear, Cmd+K, parser only, never the PTY: `clear_before_prompt()` at a prompt (`TitleCallback` not between `133;B` and `133;D`) with a `133;A` mark — the last prompt, all its lines, stays at the top — else (no mark, or a command runs) `ESC[2J ESC[H` + `clear_scrollback()`; drops the selection); `append_line(text)` (the end note); `kill()` = SIGHUP (+ SIGCONT) to the process group — the child is a session leader, so its group is everything the shell started — and SIGKILL to the group after `KILL_GRACE` (1 s) if the leader still runs or processes it started still hold the PTY; `force_kill()`; a kill while starting applies once spawned; **dropping a `Run` kills it** (and leftovers of an exited one still holding the PTY)), `keys` (`encode(key, application_cursor)`: a key as xterm sends it — UTF-8 chars, Ctrl+letter C0, Alt = Esc prefix, `CSI` / `SS3` cursor keys with xterm modifier parameters, `CSI n ~`, F1–F12; Cmd never; `paste(text, bracketed)`), `mouse` (`encode(event, mode, encoding)`: a mouse event as xterm sends it — nothing while the mode is `None`; SGR `CSI < b ; x ; y M` / release `m`, X10 / UTF-8 `CSI M` and three bytes (press-only in mode `Press`, release code 3), motion while a button is held (`ButtonMotion`, +32) and any motion (`+35`), the wheel as buttons 64–67, Shift / Alt / Ctrl +4 / +8 / +16; the event's 0-based cells + 1, X10 clamped to 223), `select` (`Selection { anchor, head }`: a drag's scroll-independent `Cell`s `(line, col)`, line 0 the live screen's first row and scrollback negative (`cell_at` from a viewport cell, `viewport_row` back); `path_run(screen, (row, col))` is a double-click's seed: the (start, end) cells of the run of ASCII alphanumerics and `/ : - _ .` around a cell on its row (none on any other cell); `rows()` puts it in row order for `contents_between` (the end column is excluded, so a click selects nothing), `copy_text(screen, selection)` scrolls the screen window by window (restored) and is `contents_between` over it with trailing spaces trimmed per line and soft-wrapped rows joined, in view or not, and `top_row(screen, height)` — the first row an ended run's screen taller than its area is drawn from — moved here from jed's `ui/task.rs`; every `Run` carries one behind a mutex: `selection()`, `set_selection(sel)`, `clear_selection()`, `copy_selection() -> Option<String>` — the mouse (`terminal_select`), `ui::task`'s highlight and `app::terminal_copy` share it, and a resize / Clear / a click clear it; a scroll doesn't, and new output keeps it on its text: the reader (`process_chunk`) takes `select::snapshot` before each chunk and `select::follow` after (shift by `Screen::scrolled_lines`'s delta, clear when a covered live row's text changed where it scrolled to, rows the cap pushed out skipped; while `set_dragging(true)` only the anchor moves and `take_drag_lines` feeds `Drag::Terminal`'s anchor copy; the rule is `select.rs`'s `//!`))

`search` (`Match { line, start, end }` in `select::Cell` space; `find_all(&mut Screen, query, case)`, windowed like `select::copy_text` with the view restored; `find_visible` for the rows shown, per frame; `TermSearch`, the bar's state over a `jtui::field::TextField`, with `label()`; literal, per screen row, case-insensitive unless asked). `shell` (`ShellTab { run, title, base, number, idle, error }`, `ShellTab::new(id, spec, rows, cols, sink, scrollback, StartOptions, number)` starts the run; `title(base, number, state)`: `zsh ○` at the prompt, `make ●` while `make` runs, `zsh (2) ○` for the next shell, glyphs `IDLE_GLYPH` / `RUNNING_GLYPH`; `base(program)`; `spec(program, root, ipc: Option<(var name, socket)>)`: argv is the program alone, no `-c` / `--norc`, so the user's init runs, cwd = root, `DEFAULT_ENV` plus the IPC variable). `shell_integration` (opt-in zsh / bash / fish snippets, templates rendered by `snippets(&Names { app, guard, prefix })` from `@@APP@@` / `@@GUARD@@` / `@@PREFIX@@`, which never occur in shell syntax and are asserted gone; jed's `NAMES`: `jed`, `JED_IPC`, `_jed`, pinned by `src/testdata/shell-integration.*`. Each opens with a guard on the IPC variable, so sourcing it elsewhere does nothing; preexec sends `133;B`, `633;E;<escaped command line>` then `OSC 0 <first word>`, precmd `133;D;<exit code>` then OSC 7 `file://host/<cwd>`; `133;A` leads the whole prompt: zsh's precmd prepends it to `PS1` once, bash's `<prefix>_mark` (appended to `PROMPT_COMMAND`, after any user hook that rebuilds `PS1`) likewise inside `\[ \]`, fish wraps `fish_prompt`; bash's `<prefix>_precmd` is prepended to the user's `PROMPT_COMMAND` (string or array) and returns the real `$?`; the DEBUG trap skips both and each `;` / newline piece of `PROMPT_COMMAND`; the snippet FILE is written once, so users re-run the install command to refresh it. Blocking `write_snippets(names, state_dir)`; `init_for(home, config_home, state_dir, shell_base)` → `Init { snippet, init, source }` (`~/.zshrc`, `~/.bashrc`, `<config_home>/fish/config.fish`; `None` for `sh`, …), `source` single-quoted; blocking `append_line(init, line)` (creates the file and folder, one `O_APPEND` write, a newline first when the file has text and doesn't end in one); blocking `status(names, home, config_home, state_dir, shell_base)` reads the init file (a line exactly the `source` line) and the snippet file: `NotInstalled` / `Stale` / `UpToDate`, `None` for `sh`).

`draw` (`draw_cells(buf, area, screen, top)`, `cursor_in`, `draw_screen(f, area, run, &jtui::Theme, focused, search) -> scrollback` (cells, every one through `Theme::terminal_cell`, the selection in `editor.selection`'s background, the search's matches and bar, the cursor), `draw_search`, `render_shell(f, area, &ShellTab, theme, focused, search)`; reads the run's status before its screen: the spawn holds the state lock while it takes the screen's. `draw::KEYS` lists the theme keys it reads besides `terminal.*`: an app's theme must define them, jed's `theme.rs` test checks Dark+ does). The app draws its own frame (jed's `ui::task::render`: info line, scroll bar) around it.

## jmux

`APP = "jmux"` (`main.rs`) is what it passes to jtui. Modules: `protocol` (`ClientMsg` / `ServerMsg` / `ByeReason`;
a frame is a u32 big-endian length of the rest, a kind byte (0 JSON, 1 raw `Output`), the payload; `encode_*`, the
incremental `FrameReader`, async `read_msg` / `write_all_msg` and blocking `read_msg_blocking`; crossterm's `Event`
travels through its `serde` feature, which jmux enables), `session` (names `[A-Za-z0-9_.-]{1,64}`, `<dir>/<name>.sock`,
`list` / `remove_stale` / `next_free_name` / `most_recent`, blocking: they connect to the sockets; every function takes the
directory, tests pass a temp dir) and the server:

- `server`: `spawn_server(exe, socket_dir, name, cwd)` (blocking; what `jmux new` calls) re-executes the binary as the hidden
  `jmux server --session <name> --socket-dir <dir>` (`setsid`, stdio null, in `cwd` or the caller's) and polls until the socket accepts;
  `run(socket_dir, Config)` is that process: bind (a live socket is `AddrInUse`, a stale file replaced; a guard removes the
  socket on every exit), then one loop over an unbounded `AppEvent` channel (drain the burst, redraw once). Each connection is
  a reader task (`AppEvent::Client` / `Closed`) and a writer task fed by a channel, so the app never waits on a client; the
  session end drops the queues and awaits the writers (2 s) so `Bye` is delivered.
- `app`: `App` (shape of jed's: owns everything, `handle(AppEvent)`, `needs_redraw`, `redraw(&mut Terminal<StreamBackend>)`).
  `groups: Groups` of `ShellTab`s (`$SHELL`, `JMUX` / `JMUX_TERMINAL` set; the first started in `start` at 24x80, every shell
  resized to its group's area each frame, a hidden tab keeps its last size); `execute(Command)` runs new / close / next /
  previous / Nth tab (new = after the active tab, numbered per program `sh (2)`; a tab with a running command, `!idle`
  from the shell integration, asks first: `confirm`, drawn over the status bar, y / Enter close, n / Esc cancel, other
  keys and the mouse swallowed). A shell that exits closes its tab; the last tab closing ends the session.
  One attached client: a `Hello` replaces it (the old one gets `Bye TakenOver`), sets the stream backend's size, clears and
  redraws in full; `Detach` / the connection closing leave the server running and drawing nothing. `key` looks the key up in `keymap`
  (`Context::Terminal` / `Confirm`), unbound keys go to the active shell, as do paste and mouse (`HitMap`: tab bar clicks
  pick / close (x or middle click) / scroll, terminal events keep their offset below the bar); `Bye SessionEnded` and the
  process exits with the last tab.
  `Query` / `Kill` / `Rename { name }` as a connection's first message never attach: answered (`Info` / `Done` / `Error`)
  and closed. `Kill` ends the session; `Rename` moves the socket file (the guard follows; running shells keep the old
  `$JMUX`).
- `client`: `attach(stream, &Terminal, input stream, out, signal) -> Outcome` (`Bye(reason)` / `ServerGone` / `Hangup`;
  `Outcome::message` is the one line, `exit_code`), driven by tests with a channel of events; `run_attached(socket, name)`
  is the real one: raw mode (`jtui::terminal::enter`, kitty when supported: the detach key needs it), a panic hook that
  restores first, SIGHUP / SIGTERM, `terminal::restore()` on every outcome, then the line.
- `cli`: `parse` -> `Cmd`; blocking functions of a socket dir (`ls`, `find`, `start`, `kill`, `rename`, `request`) and
  `nested(jmux_env)` (the `$JMUX` refusal); `main::run_cli(cmd, dir, jmux_env)` ties them to `client::run_attached`.
- `command` (`Command`, `Context`; `KeyCommand` names like `terminal.new`; plain Ctrl+letter passes through to the shell),
  `keymap` (`defaults(Platform)` as data: Ctrl+` / Ctrl+Shift+T new, Ctrl+Shift+W close, Ctrl+PageDown / PageUp, Alt+1..9,
  Ctrl+Shift+D detach; the file loading is `jmux_config`), `group` (`Group` = tabs + active + bar scroll; `Groups` over
  `jtui::split::SplitTree`, one group so far, an emptied group leaves the tree), `hit` (`HitMap` / `Hit`, written by
  `ui::render`), `tabbar` (` title × ` tabs, state glyph colours, `‹ ›` overflow arrows),
  `ui` (`render(f, &mut App)`: each group's tab bar and terminal above a one-row status bar: session name / title / detach
  hint from the keymap), `theme` (the only file
  with colours: `jterm::draw::KEYS` + `statusBar*` over `widget_entries`), `logging` (`<state dir>/jmux/logs/<name>-<pid>.log`,
  `JMUX_LOG`, panics logged).
- Tests: `jmux/src/jmux_server_tests.rs` runs a real server (own thread and runtime, `/bin/sh`) in a temp dir (a subfolder:
  `prepare_dir` insists on `0700`) and drives it with a client over the socket that feeds `Output` into a `vt100::Parser`, or
  with `client::attach` (`Attached`: events in a channel, output into a parser). `jmux/tests/cli.rs` runs the binary
  (`CARGO_BIN_EXE_jmux`, env via `Command::env`): `kill -9` of the server, `ls`, kill / rename, the `$JMUX` refusal.
  `jmux/src/jmux_layout_tests.rs` drives an `App` in-process (`Harness`: `settle` runs the event loop's steps until a
  condition, keys via `command(Command)` from the keymap, a `vt100` screen fed by the connection's `Output`) with real shells.

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