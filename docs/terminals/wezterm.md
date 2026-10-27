# WezTerm

Checked against:
- https://wezterm.org/config/lua/config/enable_kitty_keyboard.html (since 20220624-141144-bd1b7c5d)
- https://wezterm.org/config/lua/keyassignment/DisableDefaultAssignment.html
- https://wezterm.org/config/default-keys.html
- https://wezterm.org/config/keyboard-concepts.html (macOS Option keys)

Config: `~/.wezterm.lua` (or `~/.config/wezterm/wezterm.lua`). **Kitty keyboard protocol is off by default** (`enable_kitty_keyboard = false`): jed can't see Cmd at all until it's on.

## How releasing works

`action = wezterm.action.DisableDefaultAssignment` undoes a default assignment and the key press is "propagated through to the tab for processing". On a key with no default it does nothing (harmless).

## Snippet

Merge into your existing `config` table:

```lua
local wezterm = require 'wezterm'
local config = wezterm.config_builder()

-- jed needs the kitty keyboard protocol to see Cmd
config.enable_kitty_keyboard = true

-- jed: Option acts as Alt (left Option already does by default; this makes both)
config.send_composed_key_when_left_alt_is_pressed = false
config.send_composed_key_when_right_alt_is_pressed = false

-- jed: send these Cmd keys to the program
local release = {
  { 'a', 'CMD' }, { 'z', 'CMD' }, { 'z', 'CMD|SHIFT' },
  { 'c', 'CMD' }, { 'x', 'CMD' }, { '/', 'CMD' }, { ']', 'CMD' }, { '[', 'CMD' },
  { 'i', 'CMD|ALT' }, { '.', 'CMD' },
  { 'd', 'CMD' }, { 'l', 'CMD|SHIFT' }, { 'd', 'CMD|ALT' }, { 'u', 'CMD' },
  { 'UpArrow', 'CMD|ALT' }, { 'DownArrow', 'CMD|ALT' },
  { 'LeftArrow', 'CMD|ALT|SHIFT' }, { 'RightArrow', 'CMD|ALT|SHIFT' },
  { 'UpArrow', 'CMD|ALT|SHIFT' }, { 'DownArrow', 'CMD|ALT|SHIFT' },
  { 'PageUp', 'CMD|ALT|SHIFT' }, { 'PageDown', 'CMD|ALT|SHIFT' },
  { 'LeftArrow', 'CMD' }, { 'LeftArrow', 'CMD|SHIFT' },
  { 'RightArrow', 'CMD' }, { 'RightArrow', 'CMD|SHIFT' },
  { 'UpArrow', 'CMD' }, { 'UpArrow', 'CMD|SHIFT' },
  { 'DownArrow', 'CMD' }, { 'DownArrow', 'CMD|SHIFT' },
  { 'Backspace', 'CMD' },
  { 'f', 'CMD' }, { 'f', 'CMD|ALT' },
  { 'c', 'CMD|ALT' }, { 'w', 'CMD|ALT' }, { 'r', 'CMD|ALT' }, { 'x', 'CMD|ALT' },
  { '1', 'CMD|SHIFT' }, { 'Enter', 'CMD|ALT' },
  { 'n', 'CMD|ALT' }, { 'n', 'CMD|ALT|SHIFT' },
  { 'n', 'CMD' }, { 's', 'CMD' }, { 's', 'CMD|SHIFT' }, { 's', 'CMD|ALT' }, { 'q', 'CMD' }, { 'w', 'CMD' }, { 'k', 'CMD' }, { 'b', 'CMD' },
  { 'e', 'CMD|SHIFT' },
  { 'f', 'CMD|SHIFT' },
  { 'h', 'CMD|SHIFT' },
  { 'j', 'CMD|SHIFT' },
  { '1', 'CMD' }, { '2', 'CMD' }, { '3', 'CMD' }, { '4', 'CMD' }, { '5', 'CMD' },
  { '6', 'CMD' }, { '7', 'CMD' }, { '8', 'CMD' },
  { '\\', 'CMD' }, { '\\', 'CMD|ALT' },
  { 'RightArrow', 'CMD|ALT' }, { 'LeftArrow', 'CMD|ALT' },
  { 'RightArrow', 'CMD|CTRL' }, { 'LeftArrow', 'CMD|CTRL' },
  { 'PageUp', 'CMD|SHIFT' }, { 'PageDown', 'CMD|SHIFT' },
  { 'p', 'CMD|SHIFT' }, { 'p', 'CMD' },
  { 'j', 'CMD' }, { 'm', 'CMD|SHIFT' },
  { 'u', 'CMD|SHIFT' },
  { 'z', 'CMD|ALT' },
}
config.keys = config.keys or {}
for _, k in ipairs(release) do
  table.insert(config.keys, { key = k[1], mods = k[2], action = wezterm.action.DisableDefaultAssignment })
end

return config
```

Don't release `cmd+v`: WezTerm's paste is how text gets into jed.

WezTerm defaults that clash with jed: `cmd+c` copy, `cmd+n` new window, `cmd+w` close tab (with confirmation), `cmd+k` clear scrollback, `cmd+1..8` activate tab, `cmd+f` search. The rest of jed's Cmd keys have no WezTerm default.

## What you lose

WezTerm's Cmd+C (copy screen text; Ctrl+Shift+C still copies), Cmd+N (new window), Cmd+W, Cmd+K (clear scrollback), Cmd+1..8 and Cmd+F (search; Ctrl+Shift+F still searches) in every window; use Cmd+Shift+[ / ] to switch tabs. Right Option no longer composes accented characters (drop the `right` line to keep that).

## Checklist

- [ ] Snippet merged, config reloaded (Cmd+R) or WezTerm restarted
- [ ] jed's start-up warning about the kitty protocol is gone
- [ ] Every key in the [table](README.md#cmd-bindings-jed-uses) logs with `SUPER`
- [ ] **Check by hand:** Cmd+Q isn't in WezTerm's default key table, so it may come from the macOS app menu; confirm it reaches jed (Ctrl+Q always quits jed)
- [ ] `{ 'e', 'CMD|SHIFT' }`, `{ 'f', 'CMD|SHIFT' }`, `{ 'h', 'CMD|SHIFT' }`, `{ 'j', 'CMD|SHIFT' }`, `{ 'm', 'CMD|SHIFT' }` and `{ 'u', 'CMD|SHIFT' }` match: if Cmd+Shift+E, Cmd+Shift+F, Cmd+Shift+H, Cmd+Shift+J, Cmd+Shift+M or Cmd+Shift+U doesn't arrive, try `key = 'E'` / `key = 'F'` / `key = 'H'` / `key = 'J'` / `key = 'M'` / `key = 'U'`
- [ ] **Check by hand:** Cmd+Shift+P opens jed's palette, not WezTerm's command palette (WezTerm's own is on Ctrl+Shift+P; if it still grabs Cmd+Shift+P, try `key = 'P'`)
- [ ] `{ '1', 'CMD|SHIFT' }` matches: if Cmd+Shift+1 doesn't arrive, try `key = '!'`
- [ ] **Check by hand:** Cmd+/ (comment line) logs as `SUPER` with `/`; on layouts where `/` is a shifted key (German: Shift+7) the terminal may report Cmd+Shift+7 instead: bind `edit.commentLine` to that in `keybindings.toml`
- [ ] Alt+Left logs `ALT`
