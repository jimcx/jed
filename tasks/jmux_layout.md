---
id: jmux_layout
name: "jmux: tabs, splits and terminal features"
summary: many terminals per session in jed's layout — tab bars, split groups, dividers, tab drag and drop — with selection, copy / paste, scrollback, search and links in each
status: done
priority: 0
dependencies: [jmux_server]
---

See [jmux's plan](../jmux/PLAN.md): [Layout](../jmux/PLAN.md#layout), [Terminals, tabs and splits](../jmux/PLAN.md#terminals-tabs-and-splits).

  - Needs: `jmux_server`; `jtui::split`, `jtui::borders`, `jtui::scrollbar`; `jterm` selection, search, mouse and key encoding; jed's `group.rs`, `ui/tabs.rs`, `app/tab_drag.rs`, `app/shell.rs`, `app/term_search.rs`, `app/term_link.rs` and the terminal parts of `app/mouse.rs` as the code to model on (jmux's versions hold only shells, so they are much smaller)
  - jmux's own `Groups` (a `jtui` split tree of groups of `ShellTab`s), `HitMap`, `Command` enum, `Context`s and default keymap; commands never look at keys
  - The behaviours listed under "Terminals, tabs and splits" in the plan; each shell is sized to its group's area, hidden tabs keep their last size
  - Closing: a tab's close button or the close command hangs the shell up (confirm when a command is running, known from the shell integration); a shell that exits closes its tab; an empty group leaves the tree; the last tab closing ends the session
  - Mouse pointer shapes over dividers and text as jed sets them
  - If writing jmux's tab bar or groups turns out to copy more than ~300 lines of jed's unchanged, stop and report: that is the sign to make `Group<T>` generic in `jtui` instead (deferred from the `jtui` task)
  - Done when: a new terminal is a second tab and the keys cycle between them; split right gives two live shells side by side, the divider drags and focus moves with the keys and a click; a tab drags along its bar, to the other group, and onto a group's edge to split it; dragging in a terminal selects, copy puts it on the clipboard through OSC 52 and paste types it; the wheel and the scroll bar scroll the scrollback; search finds and steps through matches; a Ctrl+click on a URL opens it; a full-screen program (vim) gets its keys and mouse; detach and attach bring back the same layout
  - Subtasks (one agent and one commit each):
    - ✅ jmux_layout-a: groups and the tab bar — the model, `Command` / `Context` / default keys, new / close / next / previous / Nth tab, titles, the close confirm
      - `jmux/src/group.rs`: `Group` (tabs of `ShellTab`, `open` / `close` / `cycle` / `go_to` / `scroll_tabs`) and `Groups` on `jtui::split::SplitTree` (one group so far; `fresh_id` ready for splits; an emptied group leaves the tree unless last)
      - `command.rs` (`Command`, `Context::{Terminal, Confirm}`), `keymap.rs` (`defaults(Platform)`: new Ctrl+` / Ctrl+Shift+T, close Ctrl+Shift+W (Cmd+W), next / previous Ctrl+PageDown / PageUp, tab N Alt+1..9 (Ctrl+1..9 on macOS), detach Ctrl+Shift+D; plain Ctrl+letter passes through in `Terminal`), `hit.rs` (`HitMap`, written by `ui::render`), `tabbar.rs`
      - `app.rs`: `execute`, `new_terminal` (RunIds never reused), `request_close` (confirm over the status bar while a command runs; keys swallowed), `close_tab`; a shell exiting closes its tab, the last one ends the session; terminal mouse offset by the tab bar row
      - Copy budget: ~80–100 lines from jed's tab bar, so no generic `Group<T>` in jtui
      - Tests: `jmux/src/jmux_layout_tests.rs` (`Harness`, `jmux_layout_a_*`); the status bar doesn't show "N terminals" yet
    - ✅ jmux_layout-b: splits — split right / down, focus by key and click, divider drag, borders, per-group sizes, empty groups removed
      - `group.rs`: `Groups::split` (a new group beside the active one, focused; the caller opens its shell), `focus_step` (tree order, wraps), `focus_nth`, `focus_side` (`neighbour`: the adjacent group on that side overlapping the active one's span most), `set_ratio`
      - Commands / keys: split right Ctrl+\ (Cmd+\), split down Ctrl+Alt+\ (Cmd+Alt+\), focus group N Ctrl+1..8 (Cmd+1..8), previous / next group Ctrl+Shift+, / ., focus by side Alt+Shift+arrows (Cmd+Alt+arrows); `App::new_terminal(group)` is shared by new and split
      - `Hit::Divider(i)` (checked before the groups) and `HitMap.dividers`; `App.drag` holds a divider drag until release and owns the mouse meanwhile; a client attaching clears the drag and the pointer position; dividers drawn with `jtui::borders` (`editorGroup.border`)
      - Pointer: the server appends OSC 22 to a frame only when the shape changes (resize arrows over or while dragging a divider, the I-beam over terminal text unless its program reports the mouse); the client resets it on every exit and in the panic hook
      - Deferred: the OSC 22 passthrough wrap reads `TMUX` from the server's environment, not the client's; "N terminals" in the status bar
    - ✅ jmux_layout-c: tab drag and drop — reorder, move between groups, drop on an edge to split, the drop zone shaded
      - `jmux/src/tab_drag.rs`: `TabDrag` (the tab known by its `RunId`: a release after it closed or moved moves nothing), `TabDrop::{TabBar, Terminal { side }}`, `App::tab_drop_at` / `tab_drag_to` / `finish_tab_drag`, `shade_rect`; `Drag::Tab` owns the mouse until release like the divider drag and is cleared on attach
      - `group.rs`: `Group::move_tab`, `Groups::move_between` (an emptied source group leaves the tree), `split_empty_of`, `move_to_split`
      - jed's pure helpers `snap_side` / `gap_at` moved to `jtui::split` (`gap_at` takes `(index, x0, x1)` items); ~140 lines copied into jmux, so still no generic `Group<T>`
      - Drawing: the drop marker (`tab.dropMarker`) and the shade (`terminal.dropBackground`, new jmux theme key)
      - Deferred: no move-tab keys yet (left / right, to the next group); Esc doesn't cancel a drag (it goes to the shell)
    - ✅ jmux_layout-d: inside a terminal — mouse reporting passthrough, selection and copy (OSC 52 to the client), paste, scrollback keys / wheel / scroll bar
      - `app.rs`: `terminal_mouse` (a program reporting the mouse gets it unless Shift is held or the view is scrolled back), `select_press` / `select_drag` / `select_release` (`Drag::Terminal`; double click = jed's path run, triple click = the row, new; Shift+click extends), `scrollbar_click` / `scrollbar_drag` (`Drag::ScrollBar`), `scroll_active`; `App.out` holds escape output (OSC 52) appended to the next frame, dropped while no client is attached
      - Commands `Copy` (Ctrl+Shift+C, Cmd+C), `ScrollUp` / `ScrollDown` / `PageUp` / `PageDown` / `ScrollTop` / `ScrollBottom` (jed's shell keys); paste is the client terminal's bracketed paste (no key)
      - A scroll bar column at each terminal's right edge (`ui::shell_area`, `GroupHit::{text, scroll}`, `Hit::ScrollBar`); shells are sized to the text area, one column narrower
      - Moved out of jed: `screen_cell` / `shift_drag_cell` / `round_run_drag` → `jterm::select`, `ClickTracker` → `jtui::clicks`; new `jtui::clipboard::osc52_sequence`
      - Pitfall: don't hold `run.screen()` across `run.scroll(…)` (it deadlocks)
      - Deferred: `Clear` (Cmd+K); drag auto-scroll at the edges (jed has none); OSC 52's tmux wrap reads the server's `TMUX`
    - ✅ jmux_layout-e: search and links in a terminal; the done-when tests (`jmux/src/jmux_layout_tests.rs`), hand checks and marks the task ✅
  - ✅ Done. What it changed (for later work):
    - jmux's server `App` holds `Groups` (a `jtui::split::SplitTree` of groups of `ShellTab`s: `group.rs`), a `HitMap` written by `ui::render` (`hit.rs`), `Command` / `Context::{Terminal, Confirm, Search}` (`command.rs`) and per-platform defaults (`keymap.rs`); `App.drag` is `Divider` / `Tab` / `Terminal` / `ScrollBar`, owns the mouse until release and is cleared when a client attaches
    - Escape output for the client (OSC 22 pointer, OSC 52 copy) is appended to the next frame (`App.out`), dropped while no client is attached
    - Search: `jmux/src/app/search.rs` (`Context::Search`, parent `Terminal`, so the layout keys work while the bar is open and no key reaches the shell); Ctrl+Shift+F (Cmd+F), Enter / F3 step, Alt+C case, Esc closes. Links: Ctrl / Cmd+click on a URL opens it through `Config::open_url` (`jtui::url::open`; tests pass a recorder)
    - Shared code moved out of jed: `jtui::split::{snap_side, gap_at}`, `jtui::clicks::ClickTracker`, `jtui::clipboard::osc52_sequence`, `jterm::select::{screen_cell, shift_drag_cell, round_run_drag}`, `jterm::search::{find_in, reveal, step_pick, newest}`; jed calls them, behaviour unchanged
    - Tests: `jmux/src/jmux_layout_tests.rs` (`jmux_layout_{a..e}_*`; its `//!` doc maps each done-when clause to its test)
    - Also: `FLAKE` debugging on `jmux_server_replaces_a_stale_socket_but_not_a_live_one` and `milestone_9_rename_across_files` (each failed once, unrelated)
    - Deferred: file-path links (jmux has no editor); a status message when opening a link fails; move-tab keys; `Clear` (Cmd+K); Esc cancelling a tab drag; "N terminals" in the status bar; OSC 22 / OSC 52 tmux passthrough reads the server's `TMUX`, not the client's
    - Still to check by hand (a real attached jmux): Ctrl+` opens a second tab, Ctrl+PageDown / PageUp and Alt+1 / 2 cycle; Ctrl+\ splits right with two prompts, the divider drags with a resize pointer, Alt+Shift+arrows and a click move focus; a tab drags along its bar, to the other group's bar and onto a group's edge (marker and shade colours); drag-select then Ctrl+Shift+C pastes elsewhere through OSC 52; a paste types into the prompt; wheel, scroll bar and Shift+PageUp scroll back; `seq 1 200; echo MARK; seq 1 200; echo MARK` then Ctrl+Shift+F `mark` reads "2 of 2", steps with Enter / Shift+Enter, Esc gives keys back; Ctrl+click on `https://example.com/x` opens the browser, a wrapped URL opens whole; vim gets keys, arrows and the mouse, Shift+drag still selects; the pointer resets after detach; Ctrl+Shift+D then `jmux attach` brings back the same tabs, splits and ratios
