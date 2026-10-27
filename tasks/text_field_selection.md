---
id: text_field_selection
name: Text field selection and keys
summary: 'selection and keys in the find widget, Search view, palette and prompts (one `field::TextField`)'
status: done
priority: 0
dependencies: []
---

  - Model: `TextField` has `anchor: Option<usize>` (replacing `all_selected`) and `selection()`; `FieldCommand` (moves to Left / Right / WordLeft / WordRight / Start / End with or without select, the deletes, `SelectAll`) runs through `TextField::run` with the editor's rules (a move collapses a selection to its side, Shift extends, typing / paste / deletes replace it); `copy` / `cut` return only the selection. Offsets sit on grapheme boundaries. Word stops and graphemes come from `buffer::pos`'s new `word_left_in` / `word_right_in` / `prev_grapheme_in` / `next_grapheme_in` on a `&str`, which `word_left` / `word_right` now use too
  - Keys: `App::field_key` (`src/app/field.rs`) types plain chars, else runs the field context's own binding or the **editor** context's binding when it has a field meaning (`FieldCommand::from_command`), Copy / Cut / Paste through `app.clipboard` (`App::read_clipboard` shared with the editor's paste). Used by the find widget, the Search view, the palette and the `Input` prompt. While a field has focus, `handle_event` hands a command with a field meaning to the field instead of executing it, so a global `keybindings.toml` binding of an editor command doesn't act on the editor behind the field; other global commands (Cmd+Shift+P, Cmd+P, Cmd+S, Cmd+B) still run
  - Drawing: `ui::find::draw_field` and the prompt dialog style only the selected range
  - Deferred: undo inside a field; mouse selection (the next TODO item, Text field mouse)
  - Still to check by hand: plain Home / End in the find widget and the Search view in a real terminal and in tmux; on macOS Cmd+A / C / X, Cmd+Left / Right, Ctrl+A / E, Alt+Backspace and Cmd+Backspace in a field (the macOS branches of the tests haven't run on this machine)
