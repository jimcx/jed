---
id: jmux_commands
name: "jmux: commands side bar"
summary: a side bar listing the session's running and finished commands; picking one focuses its terminal and scrolls to where it ran
status: done
priority: 0
dependencies: [jmux_layout]
---

See [jmux's plan](../jmux/PLAN.md): [Commands side bar](../jmux/PLAN.md#commands-side-bar).

  - Needs: `jmux_layout`; `jterm`'s `CommandRecord`s (start / finish events, absolute rows, wall-clock time, the ring size argument), `Run::set_scrollback_from_top`, `shell_integration` (`status`, `write_snippets`, `append_line`); `jtui`'s scroll bar and `TextField`; jed's `tools_view.rs` / `ui/tools.rs` as a list view to model on, `app/shell.rs`'s install prompt and `shell_integration_check`
  - A model module (no drawing): the session's history as one list fed from every terminal's records — running first, then finished newest first, capped at `command_history`; a closed terminal's rows kept and marked; the filter; selection and scrolling
  - The side bar: toggle command, width by dragging its border, focus with its key, the rows and keys the plan lists, a scroll bar, an empty-state line when no shell has the integration
  - Picking a row: focus the terminal's group and tab, then put the command's first row at the top of the view when the scrollback still holds it (`start_row` against the rows scrolled away); a notice when it no longer does
  - "Install Shell Integration" (command and `jmux install-shell-integration`), with `JMUX` as the snippets' guard; a one-time notice when the first shell has none or a stale one
  - Durations and ages redraw on a coarse timer (once a second while the side bar shows), not per frame
  - Done when: with the integration installed, `sleep 5` shows as a running row with its terminal's title, then as `✓` with its duration; `false` shows `✗`; rows from two terminals interleave by time; clicking an old row in the other group focuses that terminal and scrolls its command to the top; after closing that terminal its rows are dimmed and do nothing; the filter narrows the list; the list holds at most `command_history` rows; without the integration the side bar says how to install it
  - Subtasks (one agent and one commit each):
    - ✅ jmux_commands-a: the model and its feed from the terminals' events, with unit tests; the install command and the missing-integration notice
      - `jmux/src/history.rs`: `History` (rows keyed `(RunId, record id)`; running first, newest start first, then finished newest finish first; the cap drops the oldest finished rows, never a running one; `close(run)` marks rows closed and ends a running one with no exit, glyph `·`; case-insensitive filter on the command line; selection kept by identity) with `visible`, `selected`, `set_filter`, `move_selection` / `select_first` / `select_last` / `select_index` / `scroll_by` / `reveal_selected` (all take the view height), `scroll`; `Row::{glyph, label, duration_text, age_text}`. It carries `#![allow(dead_code)]` until b uses it
      - `jmux/src/app/commands.rs`: the feed (`Run::commands()` on `Foreground` / `Ended`, and in `close_tab` before `history.close` so an unseen finish isn't lost), the one-time check on the first shell, the install off-thread
      - `jmux/src/integration.rs`: jmux's `Names` (guard `JMUX`, prefix `_jmux`), `Dirs { home, config_home, state }` (in `Config::dirs`, real ones from main.rs), `check`, `notice`, `install` (appends the line only when it isn't there; serialised in-process)
      - `App.notice` drawn over the status bar's title (`statusBar.notice`), cleared by the next key; `Command::InstallShellIntegration` (Ctrl+Shift+I) and `jmux install-shell-integration`; `Config::command_history` (default 500)
      - Tests: `jmux/src/jmux_commands_tests.rs` (`jmux_commands_a_*`, OSC codes printed by `/bin/sh`; the notice tests symlink `/bin/sh` as `zsh` in a temp dir)
    - ✅ jmux_commands-b: the side bar — drawing, toggle, resize, focus, keys, mouse, scroll bar, filter, the jump to a command; the done-when tests (`jmux/src/jmux_commands_tests.rs`), hand checks and marks the task ✅
      - `jmux/src/side.rs` (`SideBar`: visible, width, focused, filter `TextField`; `clamp_width` min 12, groups keep ≥ 20 columns), `jmux/src/ui_side.rs` (drawing), `jmux/src/app/side.rs` (`side_command`, `open_row`, `tick`, `side_mouse`); `ui::body(rows, cols, &side)` is the groups' area right of the bar and its border
      - `Command::Side(SideCommand)` (`sideBar.*`), `Context::Side` (falls back to the Terminal keys; unbound keys never reach a shell); toggle Ctrl+Shift+B (Cmd+B on macOS), focus Ctrl+Shift+E (Cmd+Shift+E)
      - `jterm::run::Run::scroll_to_command(id)` puts a command's first row at the top when the scrollback still holds it
      - `AppEvent::Tick` once a second from `App::start`; it redraws only while the bar shows rows and a client is attached

✅ Done
  - What changed: jmux has a commands side bar on the left — the session's running and finished commands from every shell with the integration (running first, then newest finished; capped at `command_history`), a filter, a scroll bar and a draggable border; picking a row focuses its terminal's group and tab and scrolls the command's first row to the top, or says it's gone from the scrollback; a closed terminal's rows stay, dimmed, and do nothing; durations and ages redraw once a second; "Install Shell Integration" (Ctrl+Shift+I, `jmux install-shell-integration`) and a one-time notice when the first shell lacks it
  - Also: a click on a closed row selects it but does nothing else; the `if row.closed` guard in `open_row` is belt and braces (a closed run's tab is gone, so `groups.find` already fails); the jmux layout tests' prompt wait works with bash as `/bin/sh`
  - Deferred: double-click and drag-select in the filter; the side bar's keys in a keybindings file (jmux_config)
  - Still to check by hand: toggle the bar; `sleep 5` running then `✓` with a live time; `false` shows `✗`; split and see rows from both groups interleave; `seq 1 200` in group 1, click its row from group 2 → the command's first line at the top; close a terminal → its rows dim and do nothing; Ctrl+Shift+E, type a filter, Up/Down/Enter, Esc; drag the border (pointer, shells reflow, 12 / 20 limits); wheel and scroll bar on a long list; detach and attach keep the bar; the install hint with no integration; redraws about once a second over SSH, not more
