---
id: term_select_word
name: Double-click selection in the terminal
summary: double-click in a shell or task tab selects the text under the pointer — a run of path characters (alphanumeric, /, :, -, _, .) — highlighted and copied by Cmd/Ctrl+C
status: open
priority: 0
dependencies: [term_select]
---

Extend `term_select`'s mouse selection with a double-click: over a shell or task tab whose program hasn't asked for the mouse, a double-click selects the text under the pointer — the longest run of characters that can make up a path (alphanumeric, `/`, `:`, `-`, `_`, `.`) — drawn highlighted and copied by Cmd/Ctrl+C, exactly as a drag's selection is. A single click still clears the selection, a drag still selects by dragging, and a program that has asked for the mouse keeps every event, unchanged.

  - Needs: `term_select` (`tasks/select.rs`, `Drag::Terminal` / `terminal_select` / `screen_cell` in `app/mouse.rs`, `Run::selection` / `set_selection`)
  - A path-run classifier in `tasks/select.rs`: given a `Screen` and a `(row, col)` cell, the `(row, col)` range of the contiguous run of path characters around that cell, on one screen row, in the visible-viewport cells the `Selection` already uses. A character is in the run when it's an ASCII alphanumeric or one of `/ : - _ .`; the run stops at any other character, a blank cell, or the row's left / right edge, and a double-click on a cell with no text selects nothing. Wide characters count by the cells they occupy, as `copy_text` already reads them.
  - `app/mouse.rs`: `terminal_select` gets the click count (the `ClickTracker` already counts clicks on one cell within `MULTI_CLICK`), so a double-click seeds the selection to that range instead of the empty `anchor == head` a click makes; a single click's empty selection — and its clearing on release — is unchanged, and a drag from a double-click still extends from the clicked cell.
  - Done when: over a shell and a task tab whose program hasn't asked for the mouse, a double-click selects exactly the run of alphanumeric, `/`, `:`, `-`, `_` and `.` characters under the pointer (e.g. `~/src/jed-0.1` in one selection), drawn highlighted, and Cmd/Ctrl+C copies that text with the drag's trailing-space trim; a double-click on a non-path character selects nothing (and clears any selection); a single click still clears and copies nothing; a drag still selects; a mouse-reporting program (vim / tmux / htop) keeps every event and is unchanged; the classifier's unit tests cover the character set and the run's bounds (a non-path neighbour, the row edge, an empty cell).
