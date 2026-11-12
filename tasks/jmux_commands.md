---
id: jmux_commands
name: "jmux: commands side bar"
summary: a side bar listing the session's running and finished commands; picking one focuses its terminal and scrolls to where it ran
status: open
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
    - jmux_commands-a: the model and its feed from the terminals' events, with unit tests; the install command and the missing-integration notice
    - jmux_commands-b: the side bar — drawing, toggle, resize, focus, keys, mouse, scroll bar, filter, the jump to a command; the done-when tests (`jmux/src/jmux_commands_tests.rs`), hand checks and marks the task ✅
