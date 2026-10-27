---
id: move_tabs
name: Move tabs
summary: move the active tab into the next / previous group, or left / right within its group
status: done
priority: 0
dependencies: []
---

  - Done: `view.moveEditorToNextGroup` / `view.moveEditorToPreviousGroup` (global) move the active tab, view and all (a diff tab keeps its id, so a load in flight still lands), into the neighbouring group in `groups.ids()` order (Cmd+1/2/3's), right of its active tab, and focus the editor there; an emptied group closes; no next group splits right. `view.moveEditorLeft` / `view.moveEditorRight` swap with a neighbour, no wrap, the tab staying active. `group.rs`: `Tab::same_as`, `Group::take`, `Group::move_active`, `Groups::move_active_tab`; `app/files.rs`: `move_tab_to_group`, `move_tab`
  - Choices: the only tab of the only group "to next" does nothing (VS Code would leave an empty group, which jed closes); no previous group does nothing; a target already showing the same buffer (or the same diff) has that tab replaced by the moved one (the moved view wins, as VS Code)
  - Also fixed: the tab bar's furthest scroll left out the last tab's own width, so an active last tab in an overflowing bar was drawn cut off under `›`
  - The "every command" tests on diff and log tabs skip the four commands (as `NextTab` / `CloseTab`)
  - Deferred: a save-then-close in flight (`AfterSave::Close(group, buffer)`) doesn't close if the tab moves before the save ends (tab cycling has the same gap)
  - Still to check by hand: Ctrl+Cmd+Left / Right and Cmd+Shift+PageUp / PageDown reaching jed in kitty, Ghostty (its default `cmd+ctrl+arrows` resizes splits; noted in its doc), WezTerm, Alacritty and iTerm2; on Linux, GNOME's Ctrl+Alt+Left / Right workspace switch, and WezTerm / kitty / GNOME Terminal's Ctrl+Shift+PageUp / PageDown defaults
