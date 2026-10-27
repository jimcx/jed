---
id: themes
name: Themes
summary: a built-in Light+, a `theme` setting, a picker with live preview and theme files
status: open
priority: 0
dependencies: []
---

See [Themes](../PLAN.md#themes).

  - Needs: `Theme` and its keys (M8), settings layering (M8), the palette's pickers (M5)
  - Light+ with the same keys as Dark+; `theme` setting; "Preferences: Color Theme" with live preview, saved to `settings.toml`; theme files from `~/.config/jed/themes/`
  - Done when: picking Light+ recolours everything — editor, syntax, side bar, panel, popups, diff view, task terminals — with no Dark+ colour left (a test draws every view under both and finds no colour of the other theme); Esc in the picker restores the old theme; the choice survives a restart; a theme file dropped into the themes folder is listed and applies, and a broken one is reported and falls back
  - Subtasks (one agent and one commit each):
    - themes-a: Light+ (`Theme::light_plus`, the same-keys test, 256-colour fallback) and the `theme` setting (`dark` / `light`), live on a `.workspace` reload
    - themes-b: "Preferences: Color Theme": the picker with live preview, Esc restores, Enter writes `theme` to the user's `settings.toml` keeping comments
    - themes-c: theme files: decide the format first (see [Themes](../PLAN.md#themes)), load from `~/.config/jed/themes/`, list in the picker, fallbacks and error reports
    - themes-d: the done-when tests (every view drawn under both themes, no colour of the other left), gaps and hand checks; marks the task ✅
