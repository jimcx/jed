---
id: side_panel_resize
name: Side panel resizing, finished
summary: make the side panel's resize sash discoverable and smooth
status: open
priority: 0
dependencies: []
---

The side panel already resizes by dragging its border (its last column: `Hit::SideBorder` → `Drag::SideBorder`, width in `ui.side_width`, kept in sessions), but it's undiscoverable and rough. Make it VS Code's sash:

  - The border column is drawn as a visible divider (a theme key, e.g. `sideBar.border`), lit on hover and while dragging (`sash.hoverBorder`); the hit area doesn't take the list's last column (scroll bars sit there since the side-bar scroll bars item) — the divider gets its own column, or the hit moves to the editor's first column, decide
  - A drag stores the clamped width (`layout::compute`'s max: the editor keeps `MIN_EDITOR_WIDTH`), so dragging back after overshooting the max moves the border at once (today the stored width can exceed the max and the border stays put until the pointer comes back); dragging below the minimum by a few more cells hides the panel (VS Code snaps it closed), dragging out from the activity bar edge reopens it
  - Double-click on the border resets the default width; the width survives toggling the panel off and on and a terminal resize (clamped when drawn, the stored width kept)
  - "View: Increase / Decrease Side Bar Width" (unbound, palette; a step of a few cells), sharing the step with Keyboard resize of splits
  - Tests: drag through `app.mouse` (hover colour, overshoot and back, snap closed, reopen, double-click reset), the commands, the session keeping the width
