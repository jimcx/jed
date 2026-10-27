---
id: cheat_sheet
name: Keyboard cheat sheet overlay
summary: a popup over the editor listing the navigation keys, built from the live Keymap
status: done
priority: 0
dependencies: []
---

  - `cheatsheet.rs` (model: `groups()` the fixed command lists, `sections(keymap, context)`, `layout(sections, width)` packing whole sections into balanced columns, `CheatSheet` scroll state as a `Scrollable`); `Keymap::context_bindings(context)` (a context's own keys plus its parents' short of Editor: Replace shows Find's); `app/cheatsheet.rs` (toggle, keys, mouse: intercepted right after a prompt, before the keymap); `ui/cheatsheet.rs` drawn after the palette, before the dialog; `ScrollTarget::CheatSheet`, `HitMap::cheat_sheet`; theme key `cheatSheet.heading` (keys `dialog.key`, titles `dialog`)
  - Also: Up / Down / PageUp / PageDown / Home / End scroll it; paste is swallowed too; the terminal cursor is hidden while it's open; row titles drop the palette's `Cursor: ` / `Selection: ` prefix (the heading says it); keys aligned per section, columns as wide as their rows; the context section is one key per command, sorted by title, left out when empty (hex)
  - Deferred: covers only the editor groups' area, not the bottom panel; with the side bar shown a 160-wide terminal still gets one column (Going Places is wide), ~200 gives two or more
  - Still to check by hand: Shift+F1 in a real kitty terminal from the editor, explorer, find widget and a diff tab; columns reflowing on resize; wheel and dragging the bar; click outside; a `keybindings.toml` rebind showing
