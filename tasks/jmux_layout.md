---
id: jmux_layout
name: "jmux: tabs, splits and terminal features"
summary: many terminals per session in jed's layout — tab bars, split groups, dividers, tab drag and drop — with selection, copy / paste, scrollback, search and links in each
status: open
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
    - jmux_layout-a: groups and the tab bar — the model, `Command` / `Context` / default keys, new / close / next / previous / Nth tab, titles, the close confirm
    - jmux_layout-b: splits — split right / down, focus by key and click, divider drag, borders, per-group sizes, empty groups removed
    - jmux_layout-c: tab drag and drop — reorder, move between groups, drop on an edge to split, the drop zone shaded
    - jmux_layout-d: inside a terminal — mouse reporting passthrough, selection and copy, paste, scrollback keys / wheel / scroll bar, search, links; the done-when tests (`jmux/src/jmux_layout_tests.rs`), hand checks and marks the task ✅
