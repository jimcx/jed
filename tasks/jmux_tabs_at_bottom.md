---
id: jmux_tabs_at_bottom
name: "jmux: tab bars at the bottom"
summary: "`tabs_at_bottom` in jmux's `settings.toml` docks each group's tab bar on its last row; Reload Settings moves the bars at once"
status: done
priority: 0
dependencies: [jmux_config]
---

See [jmux's plan](../jmux/PLAN.md): [Settings, keys and theme](../jmux/PLAN.md#settings-keys-and-theme). jed's model: `tabs_at_bottom` (`jed/src/config/settings.rs`, `edit::Settings`, the tab bar / editor split in `ui::render`, test `tabs_at_bottom` in `jed/src/todo_tests.rs`).

`tabs_at_bottom = true` in `~/.config/jmux/settings.toml` (bool, default `false`, same key, type and meaning as jed's) puts each group's tab bar on its last row and the terminal (shell and scroll bar track) above it.

  - Needs: `jmux_config` (the settings file and Reload Settings)
  - `jmux/src/settings.rs`: the `tabs_at_bottom` field, parsed leniently like the other keys (a non-bool is reported and skipped)
  - Layout in one place: `ui::term_area` (and the bar rect `ui::render` derives from it) takes the setting, so the frame and `App::term_size` (a new shell's starting size) agree; the height is the same either way, only `y` moves
  - Mouse needs no changes of its own: tab clicks, the scroll arrows, tab drag and drop (`tab_drag.rs`: the drop gap and the shaded terminal halves) all read the rects `ui::render` writes to the `HitMap`; check that each still hits the bar's new row
  - Reload Settings applies it at once (the next frame redraws the layout; shells keep their size since the height doesn't change)
  - Docs: the `settings.toml` table in `jmux/README.md`, the settings line in `jmux/PLAN.md`
  - Done when: with the setting on, each group's bar is on its last row and the shell starts on its first; clicking a tab, the bar's scroll arrows and dragging a tab to another bar or a terminal half work at the bottom; toggling it and running Reload Settings moves the bars without restarting the session; a bad value is reported and the default applies. Tests in `jmux/src/jmux_tabs_at_bottom_tests.rs`, named `jmux_tabs_at_bottom_*`

- ✅ Done. `tabs_at_bottom` (bool, default false) in jmux's `settings.toml` puts each group's tab bar on its last row.
  - Layout comes from one function, `ui::bar_and_term(group, tabs_at_bottom)` (`term_area` delegates to it), used by `ui::render` and `App::term_size`; the terminal's height is the same either way, so a reload moves the bars without resizing shells. Mouse, scroll arrows and tab drag needed no change: they read the `HitMap` rects.
  - Also: `apply_settings` needed no change (it already stores the new settings and redraws).
  - Deferred: nothing.
  - Still to check by hand: `tabs_at_bottom = true` + Ctrl+Shift+R in a real terminal with a split, the scroll bar track beside the shell, and a tab dragged onto a bottom bar.
