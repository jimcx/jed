---
id: first_reveal
name: First reveal in a never-drawn list
summary: a first selection reveal in a never-drawn list defers to the first draw
status: done
priority: 0
dependencies: []
---

  - Deferred to the first draw (not skipped, since the Explorer can reveal a start file deep in a long tree and Down / End can run before the first draw): a reveal while `height == 0` sets a private `reveal_pending`; drawing calls `set_height(…)` (never assigns `height`), which runs it once, scrolling the least that shows the row. In `grouped::ListState` (Search results, Problems, Locations), `ScmView`, `ToolsView`, `GitHistory` and `Explorer` (default height now 0; `scroll_by` / `set_top` / paging use `height.max(1)`); a resize doesn't reveal again
  - Locations' "not drawn yet" `set_scroll(0)` workaround removed; palette, completion, diff tabs and editor views already reveal with a known height
  - Tests: `todo_tests::first_reveal` (a header / first row kept in view and End before the first draw, per list; the Explorer's start file on the bottom row) and a unit test per list
  - Deferred: nothing
  - Still to check by hand: Ctrl+Shift+G at once in a repo with more changes than fit shows "Changes"; `jed <file far down a long folder>` shows the file on the Explorer's bottom row
