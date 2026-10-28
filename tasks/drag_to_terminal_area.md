---
id: drag_to_terminal_area
name: Drag terminals back into the terminal area
summary: a shell tab in an editor group can be dragged onto the terminal area (its bar, screen or edge) to dock it there, as "Move Terminal into Terminal Area" does
status: open
priority: 0
dependencies: []
---

See [Layout](../PLAN.md#layout); `term_area` deferred this ("dragging an editor-group shell into the area stays refused").

  - Goal: a terminal moved out into an editor group (by a drag or "Move Terminal into Editor Area") can be dragged back with the mouse, as in VS Code; today only the command brings it back
  - Needs: the terminal area (`term_area`) and drag-and-drop tabs (`drag_tabs`): `can_dock(from, to)` in `app/tab_drag.rs` is the single place a cross-area drag is refused; `move_tab_across` already moves a tab across the two `Groups` (`take_tab` + `put_tab`, then `hide_empty_terminals`)
  - Accept: `can_dock` allows a shell tab from an editor group into a terminal group; only `Tab::Shell` docks (a text, diff or hex tab dragged over the area stays refused: no marker, no shade, a release does nothing). Drops are the same three as between terminal groups: a gap on a terminal group's tab bar (marker), its screen's centre (moved in, activated), and its edge band (splits that terminal group on that side, shaded)
  - While the area is hidden (no terminal left in it) there is nothing to drop on. Show a drop target for it: dragging a shell tab shades a strip along the bottom of the editor area (the region the area would take, at its remembered height) in `editor.dropBackground`; a release there shows the area and docks the terminal in it. The strip appears only for a shell drag and only while the area is hidden
  - A release seals undo, focuses the terminal (`Focus::Terminal`), reveals the tab, and closes the source group if that was its last tab; the shell keeps its run and output (a move, never a copy). A drag cancelled by Esc, a closed tab (`TabIdentity` check) or a release outside any target changes nothing
  - Tests: `todo_tests::drag_to_terminal_area`: drag an editor-group shell onto the terminal area's bar (at a gap), centre and each edge, checking placement, the shade / marker, focus and that the source group closes; a text tab over the area refused on every path; the hidden-area strip (shown only for a shell drag, release shows the area and docks); a mid-drag close cancelling; the `can_dock` unit test updated
  - Docs: update the Tabs paragraph in `docs/dev/conventions.md` ("dragging an editor tab into the area stays refused" no longer holds for shells) and `term_area`'s Deferred note stays as history
  - Done when: a terminal in an editor group can be dragged onto the terminal area's bar, screen or edge and lands there with its run intact, also when the area was hidden; a non-terminal tab is still refused; and `scripts/check` passes
  - Still to check by hand: dragging a live shell (e.g. running `top`) from an editor group into the area in a real terminal, each of the drop spots with the shade readable; the hidden-area strip and the area's height afterwards; Esc mid-drag
