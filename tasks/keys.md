---
id: keys
name: Keyboard Shortcuts view
summary: a full-screen, fuzzy-searchable list of every command with its key, context and source, plus record-keys mode and an Enter-to-rebind stub
status: done
priority: 0
dependencies: []
---

  - What it changed (for later work): `src/keys.rs` — the view's model: `Row { command, key (the combo's label), spec (its config spelling), context, source }` and `KeysView { query, record, all, rows, selected, top, height }`, with `build` (every `Keymap::bindings_with_source` row plus an empty-key row per command nothing is bound to, by title then key) and `filter` (a nucleo score over the title, the command name and the key text, best wins); `src/app/keys.rs` drives it (Alt+K toggles record-keys — the next combo becomes the query; Enter → `open_keybinding_stub`), `src/ui/keys.rs` draws it (search box, the `Press key combination…` hint, a `ScrollTarget::Keys` bar); `config/keymap.rs` gains `Source::{Default,User}` on every binding (`bindings_with_source`, `source_of`) and `KEYBINDINGS_TEMPLATE` / `create_if_missing` / `append_stub` (the "create like settings" write, off the UI thread); `Context::title()` moved out of the cheat sheet so both views share it; `Command::KeyboardShortcuts` and `AppEvent::KeybindingsReady`
  - Also: a `keybindings.toml` override shows as `User`; the stub for an unbound command writes `key = ""  # set the key` (an empty key is one load error until filled in, documented as expected)
  - Deferred: none
  - Still to check by hand: the record-keys flow and the search box in a real terminal (narrow widths, the hint), Enter's stub text and the opened file, and that Alt+K doesn't collide with anything
  - Needs: the `Keymap` (track where each binding came from), the palette's command list and `nucleo` fuzzy filter (`palette::command_rows`), the cheat sheet overlay's `Scrollable` / `ScrollTarget` pattern, settings' `create_if_missing` / `TEMPLATE` (the "create like settings" pattern)
  - Done when: "Preferences: Keyboard Shortcuts" (palette, unbound) opens the view; every command shows its title, key(s), context and source; the filter matches name or key text (`ctrl+shift`); record keys filters to the pressed combo's bindings; Enter writes a `[[binding]]` stub for that command, creating `keybindings.toml` if missing (like settings); a user `keybindings.toml` binding shows as an override over the default
  - Subtasks (one agent and one commit each):
    - ✅ keys-a: the `Keymap` records each binding's source (default / `keybindings.toml`) and lists every binding with source and context; the `Command::KeyboardShortcuts` command (`preferences.keyboardShortcuts`, palette, unbound) opens and closes a new (empty) view end-to-end
    - ✅ keys-b: the view: a full-screen scrollable list of every command (title, key, context, source), fuzzy search on name or key text (the `nucleo` path as `palette::command_rows`), selection, scrolling and a scroll bar (the cheat sheet's `Scrollable` / `ScrollTarget` pattern); Esc closes
    - ✅ keys-c: record-keys mode (the next combo filters to its bindings) and Enter on a row writes the `[[binding]]` stub (creating the file like settings); the done-when tests, gaps and hand checks; marks the task ✅
