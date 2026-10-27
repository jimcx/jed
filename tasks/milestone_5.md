---
id: milestone_5
name: Palette
summary: command palette, quick open, keybinding config with contexts
status: done
priority: 0
dependencies: []
---

   - Needs: named `Command`s (M1), workspace excludes and the explorer's `ignore` walk (M2)
   - Needed by: the find widget and Search view (M6, M7 key contexts), F2 meaning rename-file in the explorer and rename-symbol in the editor (M9), palette commands in M9–M11
   - Command palette over every `Command`, fuzzy via `nucleo`, showing each one's binding
   - Quick open: walk off the UI thread, fuzzy, `file:line[:col]`; skips `.jed/`
   - Go to line (Ctrl+G) in the same input widget
   - `keybindings.toml`: user bindings over the defaults, each limited to a context (editor, find widget, palette, explorer, search, …)
   - Keymaps stay data, chosen at startup (keeps [Basic terminals](../PLAN.md#basic-terminals) open)
   - Commands owed from earlier: "Manage Workspace Trust" (M2), "Open Log File", "Set Log Level" ([Logging](../PLAN.md#logging))
   - Done when: every command can be run from the palette; a binding in `keybindings.toml` replaces the default and shows in the palette; Esc closes the palette and does nothing else
   - Also: F1 opens the palette (VS Code default); the explorer hides `.jed/` too (shared walker with quick open)
   - Deferred: live preview for go to line; unbinding in `keybindings.toml`; live reload of `keybindings.toml` (TODO); find-widget and search contexts arrive with M6 / M7
   - Still to check by hand: Cmd+P / Cmd+Shift+P reach jed in each terminal (Ghostty and WezTerm palettes, iTerm2 Print menu); quick open speed on a large repo
