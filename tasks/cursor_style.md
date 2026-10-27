---
id: cursor_style
name: Cursor style setting
summary: `cursor_style` and `cursor_blinking` settings for the terminal cursor
status: done
priority: 0
dependencies: []
---

  - `edit::CursorStyle` (`line` / `block` / `underline`) + `cursor_blinking` in `edit::Settings` / `SettingsOverrides` / `TEMPLATE`; `terminal::CursorShape { style, blinking }` → crossterm `SetCursorStyle` (`set_cursor_shape`); `enter(kitty, cursor)` sends it last, `restore` (and so the panic hook) sends `CSI 0 q` before leaving the alternate screen; `start` / `finish` take any writer for byte tests. DECSCUSR isn't wrapped for tmux passthrough (tmux handles it via `Ss` / `Se`)
  - `App::cursor_shape` remembers the shape sent; `App::sync_cursor_shape(out)` runs each loop turn after the pointer sync and re-sends only on a change (a `.workspace` reload)
  - Secondary cursors: `line` / `underline` draw `editor.cursor.secondaryUnderline` (underlined, #AEAFAD underline colour, char readable), `block` the reversed `editor.cursor.secondary`; the default is now underlined (milestone 6 / render whitespace tests updated)
  - Task output only reaches its `vt100` parser, which drops DECSCUSR (`todo_tests::cursor_style::a_tasks_cursor_shape_request_is_not_forwarded`)
  - Also fixed: `milestone_9_open_save_close_and_sync_kinds` flaked under load (a quick-suggestion completion request during the save's settle); its assertion now keeps only `textDocument/did*`
  - Deferred: re-sending on a `settings.toml` change (waits for its live reload TODO)
  - Still to check by hand: shape and blinking on enter, the reset on quit and after a panic, a `.workspace` edit switching it live, in kitty, Ghostty, WezTerm, iTerm2, Alacritty and tmux (older tmux needs `terminal-overrides` `Ss` / `Se`); a `printf '\e[2 q'` task leaves jed's shape alone
