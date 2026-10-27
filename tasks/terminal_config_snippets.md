---
id: terminal_config_snippets
name: Terminal config snippets
summary: `docs/terminals/<terminal>.md` snippets passing every Cmd binding through, with Option-as-Alt
status: done
priority: 0
dependencies: []
---

  - `docs/terminals/<terminal>.md` for Kitty, Ghostty, WezTerm, iTerm2, Alacritty
  - Each: pass through every Cmd binding in the default keymap; turn on Option-as-Alt
  - Update the snippets whenever a Cmd binding is added (keymap change → snippet change)
  - Per-terminal check: press every binding and confirm it arrives (`JED_LOG=input=trace`)
  - Done: `docs/terminals/` (README + kitty, ghostty, wezterm, iterm2, alacritty); unit test `terminal_docs_list_every_cmd_binding` fails if the README table and the macOS Cmd keymap differ
  - Still to do: hand-check each terminal with its checklist (iTerm2 least certain)
