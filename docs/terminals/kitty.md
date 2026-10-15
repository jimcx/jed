# Kitty

Checked against kitty **0.49.2** docs:
- https://sw.kovidgoyal.net/kitty/conf/ (default shortcuts, `macos_option_as_alt`)
- https://sw.kovidgoyal.net/kitty/mapping/ (unmapping, `--when-focus-on`)

Snippet below parsed by kitty 0.49.2's own config loader: no warnings, every line overrides the default, Option-as-Alt = both.

Config: `~/.config/kitty/kitty.conf`. Kitty keyboard protocol: always available, nothing to enable.

## How releasing works

`map <key>` with **no action** unmaps the shortcut and passes the key to the program ("mapping it to nothing"). Don't use `discard_event`: it drops the key without sending it to the program.

## Snippet

```conf
# --- red: send these Cmd keys to the program ---
map cmd+a
map cmd+z
map cmd+shift+z
map cmd+c
map cmd+x
map cmd+d
map cmd+shift+l
map cmd+alt+d
map cmd+u
map cmd+alt+up
map cmd+alt+down
map cmd+alt+shift+left
map cmd+alt+shift+right
map cmd+alt+shift+up
map cmd+alt+shift+down
map cmd+alt+shift+page_up
map cmd+alt+shift+page_down
map cmd+left
map cmd+shift+left
map cmd+right
map cmd+shift+right
map cmd+up
map cmd+shift+up
map cmd+down
map cmd+shift+down
map cmd+backspace
map cmd+f
map cmd+alt+f
map cmd+alt+c
map cmd+alt+w
map cmd+alt+r
map cmd+shift+1
map cmd+alt+enter
map cmd+alt+n
map cmd+alt+shift+n
map cmd+s
map cmd+q
map cmd+w
map cmd+b
map cmd+shift+e
map cmd+1
map cmd+2
map cmd+3
map cmd+backslash
map cmd+alt+backslash
map cmd+alt+right
map cmd+alt+left
map cmd+shift+p
map cmd+p

# --- red: Option acts as Alt ---
macos_option_as_alt both
```

Don't add `cmd+v`: kitty's paste is how text gets into red.

Only some of these are kitty defaults (`cmd+c` copy, `cmd+up`/`cmd+down` scroll, `cmd+1..3` switch window, `cmd+w` close tab, `cmd+q` quit); the rest are listed so the snippet stays a copy of red's table.

## What you lose

Kitty's own Cmd+C (copy screen text; use the mouse selection, which kitty can copy on select), Cmd+W (close tab), Cmd+Q (quit), Cmd+1..3 (switch window) and Cmd+Up/Down (scroll) in **every** kitty window. Close tabs with Cmd+Shift+W (close OS window) or the menu.

Scoping to red only: kitty supports conditional mappings (`map --when-focus-on var:<name> <key>`), but red doesn't set a kitty user variable yet, so there's nothing to match on. Until it does, the unmapping is global.

## Checklist

- [ ] Snippet added, kitty restarted
- [ ] Every key in the [table](README.md#cmd-bindings-red-uses) logs with `SUPER` (see [Checking a terminal](README.md#checking-a-terminal))
- [ ] Cmd+W closes red's tab, not kitty's
- [ ] Cmd+Q reaches red. **Check by hand:** kitty builds its macOS menu from mappings; confirm the Quit menu item no longer grabs Cmd+Q after unmapping
- [ ] Alt+Left logs `ALT` (Option-as-Alt works)
