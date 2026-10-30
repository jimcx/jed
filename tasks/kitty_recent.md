---
id: kitty_recent
name: Terminal integration for recent workspaces (kitty first)
summary: switch to a running workspace's terminal window / tab, or open a new one, through kitty's remote control protocol on a socket
status: open
priority: 0
dependencies: []
---

Deferred from M13 — see [Recent workspaces](../PLAN.md#workspace). Picking a recent workspace that is already open in jed switches to its OS window and tab; one that isn't opens in a new OS window. jed speaks kitty's remote control protocol itself over the socket kitty listens on: no `kitten` binary, no controlling-tty round trip. The code is split so another terminal (WezTerm's `wezterm cli`, tmux, …) is one more backend, not a rewrite.

**Reference**: the kitty source at `~/kitty` — `docs/rc_protocol.rst` (framing, version rule, response shape), `kitty/remote_control.py` (`encode_send`, `SocketIO`, `do_io`, `parse_address_spec` in `kitty/utils.py`), `kitty/rc/{ls,focus_window,launch}.py` (payloads), `kitty/window.py` `as_dict` (the `ls` window fields), `kitty/boss.py` `listen_on` / `kitty/child.py` (`KITTY_LISTEN_ON`, `KITTY_PUBLIC_KEY` set in children).

**Protocol** (`kitty` backend, blocking std I/O in `spawn_blocking`)
- Address: `$KITTY_LISTEN_ON` (kitty exports it to every child when `listen_on` / `--listen-on` is set). `unix:<path>` (relative paths are resolved by kitty, so in practice absolute) and, on Linux, `unix:@name` (abstract: `SocketAddrExt::from_abstract_name`, `cfg(target_os = "linux")`; elsewhere `unix:@…` means backend unavailable). `tcp:` / `fd:` / unset → backend unavailable (Deferred: `tcp:`)
- One request per connection: write `ESC P @kitty-cmd <json> ESC \`, `shutdown(Write)`, read to EOF, take the bytes between `ESC P @kitty-cmd` and `ESC \`. Request `{"cmd", "version": [0, 14, 2], "no_response": false, "payload": {…}}` (the version only has to be ≤ kitty's major.minor; an old fixed one works with every kitty that has the commands). Response `{"ok": true, "data": …}` or `{"ok": false, "error": …}`; `ls` puts a JSON *string* in `data` (parse twice)
- Read and write timeouts (2 s) on the socket; the whole call is one blocking function. Errors (no socket, refused, timeout, `ok: false`, a response that doesn't parse) are `Err(String)`; the caller logs and falls back to the message
- Commands used:
  - `ls` (payload `{}`): OS windows → `tabs` → `windows`; per window `id`, `user_vars` (`jed_workspace`, already set by `terminal::Identity`), plus `is_focused` and the OS window / tab ids around it. A window is a running jed for a workspace when `user_vars.jed_workspace` equals the path (never match on title, cwd or cmdline: a shell `cd`'d there isn't jed)
  - `focus-window` (payload `{"match": "id:<window id>"}`): kitty's `set_active_window(…, switch_os_window_if_needed=True)` raises the OS window and selects the tab in one go
  - `launch` (payload `{"type": "os-window", "cwd": "<path>", "args": ["jed", "<path>"]}`): new OS window; `data` is the new window id
- `allow_remote_control` must be `yes` or `socket` / `socket-only`, with `listen_on`; a password setup (`password`, encrypted requests: `rc_crypto`) is out of scope → a clear "remote control refused" message

**Structure** (new `src/termctl/`, no app state, nothing drawn; `app/recent.rs` drives it)
- `termctl/mod.rs`: the seam. `trait Backend: Send { fn name(&self) -> &str; fn windows(&self) -> Result<Vec<Window>, String>; fn focus(&self, &WindowRef) -> Result<(), String>; fn launch(&self, workspace: &Path) -> Result<(), String>; }` (all blocking), `Window { workspace: PathBuf, id: WindowRef }`, `WindowRef(String)` (opaque: kitty's numeric id, another terminal's pane id), and `detect(env: &Env) -> Option<Box<dyn Backend>>` that tries each backend in turn (`Env` is a plain struct of the variables read, so tests pass their own: no `set_var`). Add a terminal = a `termctl/<name>.rs` with a `Backend` impl and a line in `detect`
- `termctl/kitty.rs`: `Kitty { address: Address }`, `Address::parse(spec)` (the `parse_address_spec` port), the framing (`encode`, `decode` pure functions, unit-tested against kitty's own examples), the three commands, the `ls` JSON → `Vec<Window>` mapping (pure, tested with canned JSON)
- `recent.rs`: `Entry.window` becomes `Option<termctl::WindowRef>` (replaces the `WindowId = u64` placeholder); `how_to_open(entry, caps)` returns `Open::{Focus(WindowRef), Launch, Message}` — `Focus` when the entry has a window, `Launch` when a backend is present, else `Message` (the existing text). The picker row keeps "open" for an entry with a window
- `app/recent.rs`: `App` holds `termctl: Option<Arc<dyn Backend>>` from `detect` in `App::start` (not `App::new`: it must not spawn, and detect reads the environment once); `show_recent` lists the file and asks the backend for `windows()` in the same `spawn_blocking`, so the picker opens once with its marks (a backend error is logged and means no marks, not no picker); `open_recent` runs `focus` / `launch` in `spawn_blocking` and reports with an `AppEvent` (`TermCtlDone { what, result }`): `Ok` → status-bar "Switched to <path>" / "Opened <path> in a new window"; `Err` → logged at `warn`, then the `Message` fallback with the reason appended
- The current workspace's own window never needs focusing (it is left out of the list); jed never changes its own workspace

**Docs**: `docs/terminals/kitty.md` — a "Switching between workspaces" section: `allow_remote_control socket-only`, `listen_on unix:@kitty-{kitty_pid}` (Linux) or `unix:/tmp/kitty-{kitty_pid}` (macOS has no abstract sockets, and its `sun_path` is 104 bytes, so a socket under the long `$TMPDIR` can fail to bind: use `/tmp`), that `KITTY_LISTEN_ON` is set in new windows only (restart kitty after changing it), tmux needs `KITTY_LISTEN_ON` passed through, passwords unsupported. A short "other terminals" note in `docs/terminals/CLAUDE.md` pointing at `termctl::Backend`. `docs/dev/modules.md` gets the `termctl` entry; the `recent.rs` entry's "deferred kitty work" wording is updated; PLAN.md's Recent workspaces section is already updated with this task

**Tests** (no real kitty on the Linux box)
- Fake sockets are bound under a short path (macOS's 104-byte `sun_path` limit), not the default long tempdir
- Unit: address parsing (`unix:/p`, `unix:@abs`, `tcp:` and junk rejected), `encode` / `decode` (a response split across reads, trailing bytes, `ok: false`), the `ls` mapping (windows in several OS windows and tabs, a window without the var, a window whose title says `jed` but has no var, a non-UTF-8 var)
- A fake kitty: a `UnixListener` thread on a socket in a tempdir that answers `ls` with canned JSON (a string in `data`, like kitty), records every request it gets and answers `focus-window` / `launch` with `ok`; one variant answers `ok: false`, one never answers (the timeout), one closes at once. Driven through the real `Kitty` backend: the recorded `focus-window` payload is `id:<id>` of the window carrying the workspace's var; `launch` has the cwd and `["jed", "<path>"]`; every request carries a valid `version`
- End to end (`src/kitty_recent_tests.rs`, `kitty_recent_*`): `App` with `Env` pointing at the fake socket — Ctrl+R marks the open workspace's row, Enter on it sends `focus-window` and says so; Enter on an unopened one sends `launch`; with the socket gone / refused / silent, Enter shows the old "To open …, run: jed …" message and the failure is in the log; no `KITTY_LISTEN_ON` → no calls and no marks; the UI thread is never blocked (the silent fake: `settle` still processes keys while the call times out)

**Done when**
- With kitty running with `listen_on`, Ctrl+R in one jed marks the workspaces open in other jed windows (any OS window, any tab); Enter on one raises its OS window and selects its tab
- Enter on an unmarked one opens a new OS window running `jed <path>` in that folder
- Without kitty, without `listen_on`, with remote control off or kitty not answering within 2 s, Enter shows the message with the `jed <path>` command and nothing hangs
- A second backend would add one file under `src/termctl/` and one `detect` line, with no change to `recent.rs` or `app/recent.rs`
- Hand checks: real kitty on macOS and Linux (`unix:` path and `@abstract` socket, window in another OS window, window in a background tab, window in a minimised OS window, a stale `jed_workspace` after `kill -9`), then a jed inside tmux inside kitty

**Deferred**
- `tcp:` / `fd:` addresses, the password / encrypted protocol, the controlling-tty transport (`kitten @` without a socket)
- Other backends: WezTerm (`wezterm cli list --format json` / `activate-pane`), tmux (`list-panes -a -F` + `select-window`), iTerm2 (its Python API / AppleScript), Ghostty (no remote control yet)
- A stale mark after a `kill -9`'d jed (the var stays): the focus works but lands on a shell; a follow-up could check the window's `cmdline` / foreground process too
