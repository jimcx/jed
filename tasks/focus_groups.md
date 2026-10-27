---
id: focus_groups
name: Editor group number label and more focus-group keys
summary: a number in each editor group's tab-bar corner, Cmd/Ctrl+1-8 focus keys, and focus next / previous editor group
status: done
priority: 0
dependencies: []
---

  - What it changed (for later work): `Command::FocusNextGroup` / `FocusPreviousGroup` (variants 123/124, `VARIANTS` 123→125) focus the group after / before the active one in `groups.ids()` (reading order), wrapping, via a new `App::focus_group_relative` (`app/files.rs`) that reuses `focus_group`; the focus-group keybindings grew from Cmd/Ctrl+1–3 to 1–8; `KeyCombo::from_event` now normalises the shifted `<` / `>` back to `,` / `.` (+Shift) only while a chord is held (so typed Shift+, still inserts a character) — the Settings task's `settings-b` reuses this for Cmd+Shift+,; `ui/tabs.rs`'s `render` gained a `number: Option<u8>` parameter that draws the group's 1-based reading-order number in the bar's last cells (a new `tab.groupNumber` theme key, dim foreground on the bar's background), shown only with two or more groups, the bar's layout shrunk left of it so the `›` scroll arrow sits one cell to the left of the number; clicking the number is the existing `Hit::TabBar` → `focus_group`, so no new hit variant or `layout.rs` change
  - Also: the label and the keybindings share `groups.ids()`'s reading order; the number is recomputed per frame, so splits / closes / reopens renumber and it disappears at one group
  - Deferred: none
  - Still to check by hand: a real terminal — with two groups the dim `1` / `2` show in the top-right corners and clicking either focuses it; a single group shows none; three splits renumber on close / reopen; a narrow window with overflowing tabs keeps the number with the `›` just to its left; Ctrl+Shift+, / Ctrl+Shift+. cycle groups on a kitty-protocol terminal (they arrive as `<` / `>`)
  - Needs: `Groups::ids()` reading order, `FocusGroup` (one past the last splits right), `PALETTE_GROUPS` = 8, the tab bar's `›` scroll arrow and `Hit::TabBar`
  - Done when: with a split, each group's corner shows its number in reading order and clicking it focuses the group; a single group shows nothing; the numbers follow reading order as groups are split, closed and reopened; Cmd/Ctrl+8 focuses the eighth group (and one past the last splits right, as VS Code); Focus Next / Previous Editor Group cycles through the groups in reading order, wrapping
  - Subtasks (one agent and one commit each):
    - ✅ focus_groups-a: the focus-group keys: Cmd/Ctrl+1–8 in `keymap.rs`, the cheat sheet's "Tabs and Groups" section and `docs/terminals/` (the Cmd table and every snippet gain `cmd+4`–`cmd+8`); `FocusNextGroup` / `FocusPreviousGroup` focusing the group after / before the active one in `groups.ids()`, wrapping, bound to Ctrl+Shift+, (previous) / Ctrl+Shift+. (next) on both platforms (the shifted `,`/`.` arrive as `<`/`>`, so `from_event` normalises them); tests for focus-8, one-past-last split right, next/previous cycle and wrap
    - ✅ focus_groups-b: the group number label: a small number in each group's tab-bar top-right corner (right-aligned, after the tabs; the `›` arrow takes precedence), only with two or more groups, a new `tab.groupNumber` theme key and the `Hit::TabBar` click to focus; the task's done-when label tests and hand checks; marks the task ✅
