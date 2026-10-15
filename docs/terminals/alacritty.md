# Alacritty

Checked against:
- https://alacritty.org/config-alacritty.html (`keyboard.bindings`, `window.option_as_alt`, config path)
- https://alacritty.org/config-alacritty-bindings.html (macOS defaults)
- https://github.com/alacritty/alacritty/blob/master/CHANGELOG.md (kitty keyboard protocol fixes from 0.13.x; latest release listed: 0.17.0)

Needs **Alacritty 0.13 or later** (kitty keyboard protocol). Config: `~/.config/alacritty/alacritty.toml`. No setting needed for the protocol.

## How releasing works

The docs: "To unset a default binding, you can use the action "ReceiveChar" to remove it or "None" to inhibit any action." Use `ReceiveChar`: it passes the key on. `None` swallows it.

## Snippet

```toml
[window]
# red: Option acts as Alt
option_as_alt = "Both"

# red: send these Cmd keys to the program
[keyboard]
bindings = [
  { key = "A", mods = "Command", action = "ReceiveChar" },
  { key = "Z", mods = "Command", action = "ReceiveChar" },
  { key = "Z", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "C", mods = "Command", action = "ReceiveChar" },
  { key = "X", mods = "Command", action = "ReceiveChar" },
  { key = "Slash", mods = "Command", action = "ReceiveChar" },
  { key = "D", mods = "Command", action = "ReceiveChar" },
  { key = "L", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "D", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "U", mods = "Command", action = "ReceiveChar" },
  { key = "Up", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "Down", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "Left", mods = "Command|Alt|Shift", action = "ReceiveChar" },
  { key = "Right", mods = "Command|Alt|Shift", action = "ReceiveChar" },
  { key = "Up", mods = "Command|Alt|Shift", action = "ReceiveChar" },
  { key = "Down", mods = "Command|Alt|Shift", action = "ReceiveChar" },
  { key = "PageUp", mods = "Command|Alt|Shift", action = "ReceiveChar" },
  { key = "PageDown", mods = "Command|Alt|Shift", action = "ReceiveChar" },
  { key = "Left", mods = "Command", action = "ReceiveChar" },
  { key = "Left", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "Right", mods = "Command", action = "ReceiveChar" },
  { key = "Right", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "Up", mods = "Command", action = "ReceiveChar" },
  { key = "Up", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "Down", mods = "Command", action = "ReceiveChar" },
  { key = "Down", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "Back", mods = "Command", action = "ReceiveChar" },
  { key = "F", mods = "Command", action = "ReceiveChar" },
  { key = "F", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "C", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "W", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "R", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "1", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "Enter", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "N", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "N", mods = "Command|Alt|Shift", action = "ReceiveChar" },
  { key = "S", mods = "Command", action = "ReceiveChar" },
  { key = "Q", mods = "Command", action = "ReceiveChar" },
  { key = "W", mods = "Command", action = "ReceiveChar" },
  { key = "B", mods = "Command", action = "ReceiveChar" },
  { key = "E", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "F", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "1", mods = "Command", action = "ReceiveChar" },
  { key = "2", mods = "Command", action = "ReceiveChar" },
  { key = "3", mods = "Command", action = "ReceiveChar" },
  { key = "Backslash", mods = "Command", action = "ReceiveChar" },
  { key = "Backslash", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "Right", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "Left", mods = "Command|Alt", action = "ReceiveChar" },
  { key = "P", mods = "Command|Shift", action = "ReceiveChar" },
  { key = "P", mods = "Command", action = "ReceiveChar" },
]
```

If you already have a `[keyboard]` section, merge the entries into its `bindings`.

Don't add `cmd+v`: Alacritty's paste is how text gets into red.

Alacritty defaults that clash with red: `cmd+c` copy, `cmd+w` and `cmd+q` **both quit Alacritty** (Cmd+W isn't "close tab"), `cmd+f` / `cmd+b` search forward / backward, `cmd+1..3` select tab.

## What you lose

Alacritty's Cmd+C copy, Cmd+W/Cmd+Q quit, Cmd+F/Cmd+B search and Cmd+1..3 tab switching in every window. Alacritty has no per-program config.

## Checklist

- [ ] Snippet added; Alacritty reloads config live (restart if not)
- [ ] `alacritty --version` is 0.13 or later
- [ ] **Check by hand:** the docs don't say whether Alacritty forwards Cmd combos under the kitty protocol once released; confirm every key in the [table](README.md#cmd-bindings-red-uses) logs with `SUPER`
- [ ] **Check by hand:** key names `Back` and `Backslash` are accepted (Alacritty warns about unknown keys in its log; try `"\\"` for backslash if needed)
- [ ] Cmd+W closes red's tab and Alacritty stays open
- [ ] **Check by hand:** Cmd+/ (comment line) logs as `SUPER` with `/`; on layouts where `/` is a shifted key (German: Shift+7) the terminal may report Cmd+Shift+7 instead: bind `edit.commentLine` to that in `keybindings.toml`
- [ ] Alt+Left logs `ALT`
