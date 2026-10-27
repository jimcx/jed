---
id: history_view
name: File History section in the Explorer
summary: "the active file's saved snapshots as a third Explorer section, built like \"Git History\" (VS Code's Timeline shows both sources; jed keeps them apart): the tree, then \"FILE HISTORY\", then \"GIT HISTORY\", each section with its header row (click toggles collapse), a draggable divider above it, and the tree keeping its minimum height (`git_history::split` generalised to N sections: each section's height and collapsed state; a collapsed one is its header only)"
status: open
priority: 0
dependencies: []
---

  - Rows: the entries newest first, as the picker shows them — relative time (`history::ago`), size (`size_label`), the UTC time dimmed (theme keys, e.g. `fileHistory.date` / `.size`); "No history" for a file with none, a file outside the workspace, or a non-file tab (log, commit); a diff tab counts as its file (a history diff as its file too)
  - Follows the active file like Git History; entries read off-thread (`history::entries` in `spawn_blocking`, generation-checked); re-read after `HistoryDone` for that file (a save added an entry, a rename moved them) — no polling, and nothing read while the section is collapsed
  - Selection, scrolling and the scroll bar as the other side lists (`Scrollable`, a `ScrollTarget` of its own); keys in a new `Context::FileHistory` (Up / Down / PageUp / PageDown / Home / End, Left / Esc back to the tree); Enter or a click opens Compare (`Pair::history(snapshot)` diff tab, as the picker's Compare); "File History: Restore Entry" (palette, and a key in the section, e.g. Cmd+Enter / Ctrl+Enter — decide) restores the selected entry into the buffer (`restore` as today: one undo step, nothing written)
  - "File History: Show" focuses the section (expanding it) instead of opening the picker; the picker stays reachable for Restore and from the explorer's / tab bar's right-click until context menus exist — decide in the item whether the picker goes
  - Sessions keep its collapsed state and height (`SideState`, serde defaults, `VERSION` unchanged)
  - Tests: entries listed newest first after saves (and a new row after one more save), switching tabs switches the list, Enter / click opens the history diff, Restore through the section, collapse / divider drag / scroll bar, session round trip; its reveal defers to the first draw like the other lists' (a pending flag run by `set_height`)
  - Needs: the Explorer's Git History section (`git_history.rs`, `git_history::split`), `history.rs`, the diff view's `Pair::history`, `Scrollable`, sessions' `SideState`
  - Done when: the tests above pass
  - Subtasks (one agent and one commit each):
    - history_view-a: `git_history::split` generalised to N sections (heights, collapsed states, dividers, the tree's minimum) with an empty "FILE HISTORY" section drawn above "GIT HISTORY"; Git History unchanged (its tests still pass)
    - history_view-b: the rows: following the active file, entries read off-thread and re-read after `HistoryDone`, "No history" cases, drawing and theme keys, selection / scrolling / the scroll bar
    - history_view-c: `Context::FileHistory` keys, Enter / click opens Compare, Restore Entry, "File History: Show" and the picker decision, sessions; the remaining tests, gaps and hand checks; marks the task ✅
