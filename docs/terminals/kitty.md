# Kitty

Checked against kitty **0.49.2** docs:
- https://sw.kovidgoyal.net/kitty/conf/ (default shortcuts, `macos_option_as_alt`)
- https://sw.kovidgoyal.net/kitty/mapping/ (unmapping, `--when-focus-on`)

Snippet below parsed by kitty 0.49.2's own config loader: no warnings, every `map` a conditional mapping on `var:jed`, Option-as-Alt = both.

Config: `~/.config/kitty/kitty.conf`. Kitty keyboard protocol: always available, nothing to enable.

## How releasing works

`map <key>` with **no action** unmaps the shortcut and passes the key to the program ("mapping it to nothing"). Don't use `discard_event`: it drops the key without sending it to the program.

Each line is scoped with `--when-focus-on var:jed`: jed sets the user var `jed` (OSC 1337 `SetUserVar`) while it runs and clears it on exit (and when it crashes), so the keys are released only while a jed window has focus; every other kitty window keeps kitty's own shortcuts. `--when-focus-on` takes a kitty search expression (spaces and parentheses are operators, so match on the var, not a title regex; `cmdline:` only sees the window's launch command, not jed started from its shell). A `kill -9`'d jed can't clear the var: run `printf '\e]1337;SetUserVar=jed\a'` in that window.

jed also sets the window title to `jed <workspace>` (`jed <file>` in single file mode; kitty's title stack restores the shell's on exit), the user var `jed_workspace` (the workspace root, for `kitten @ ls`) and OSC 7 (the root as the window's directory, so new windows / tabs opened from jed's start there). Inside tmux the user vars need `set -g allow-passthrough on`.

## Snippet

```conf
# --- jed: send these Cmd keys to the program ---
map --when-focus-on var:jed cmd+a
map --when-focus-on var:jed cmd+z
map --when-focus-on var:jed cmd+shift+z
map --when-focus-on var:jed cmd+c
map --when-focus-on var:jed cmd+x
map --when-focus-on var:jed cmd+slash
map --when-focus-on var:jed cmd+]
map --when-focus-on var:jed cmd+[
map --when-focus-on var:jed cmd+alt+i
map --when-focus-on var:jed cmd+period
map --when-focus-on var:jed cmd+d
map --when-focus-on var:jed cmd+shift+l
map --when-focus-on var:jed cmd+alt+d
map --when-focus-on var:jed cmd+u
map --when-focus-on var:jed cmd+alt+up
map --when-focus-on var:jed cmd+alt+down
map --when-focus-on var:jed cmd+alt+shift+left
map --when-focus-on var:jed cmd+alt+shift+right
map --when-focus-on var:jed cmd+alt+shift+up
map --when-focus-on var:jed cmd+alt+shift+down
map --when-focus-on var:jed cmd+alt+shift+page_up
map --when-focus-on var:jed cmd+alt+shift+page_down
map --when-focus-on var:jed cmd+left
map --when-focus-on var:jed cmd+shift+left
map --when-focus-on var:jed cmd+right
map --when-focus-on var:jed cmd+shift+right
map --when-focus-on var:jed cmd+up
map --when-focus-on var:jed cmd+shift+up
map --when-focus-on var:jed cmd+down
map --when-focus-on var:jed cmd+shift+down
map --when-focus-on var:jed cmd+backspace
map --when-focus-on var:jed cmd+f
map --when-focus-on var:jed cmd+alt+f
map --when-focus-on var:jed cmd+alt+c
map --when-focus-on var:jed cmd+alt+w
map --when-focus-on var:jed cmd+alt+r
map --when-focus-on var:jed cmd+alt+x
map --when-focus-on var:jed cmd+shift+1
map --when-focus-on var:jed cmd+alt+enter
map --when-focus-on var:jed cmd+alt+n
map --when-focus-on var:jed cmd+alt+shift+n
map --when-focus-on var:jed cmd+n
map --when-focus-on var:jed cmd+s
map --when-focus-on var:jed cmd+q
map --when-focus-on var:jed cmd+w
map --when-focus-on var:jed cmd+b
map --when-focus-on var:jed cmd+shift+e
map --when-focus-on var:jed cmd+shift+f
map --when-focus-on var:jed cmd+shift+h
map --when-focus-on var:jed cmd+shift+j
map --when-focus-on var:jed cmd+1
map --when-focus-on var:jed cmd+2
map --when-focus-on var:jed cmd+3
map --when-focus-on var:jed cmd+backslash
map --when-focus-on var:jed cmd+alt+backslash
map --when-focus-on var:jed cmd+alt+right
map --when-focus-on var:jed cmd+alt+left
map --when-focus-on var:jed cmd+ctrl+right
map --when-focus-on var:jed cmd+ctrl+left
map --when-focus-on var:jed cmd+shift+page_up
map --when-focus-on var:jed cmd+shift+page_down
map --when-focus-on var:jed cmd+shift+p
map --when-focus-on var:jed cmd+p
map --when-focus-on var:jed cmd+j
map --when-focus-on var:jed cmd+shift+m
map --when-focus-on var:jed cmd+shift+u

# --- jed: Option acts as Alt ---
macos_option_as_alt both
```

Don't add `cmd+v`: kitty's paste is how text gets into jed.

Only some of these are kitty defaults (`cmd+c` copy, `cmd+n` new OS window, `cmd+up`/`cmd+down` scroll, `cmd+1..3` switch window, `cmd+w` close tab, `cmd+q` quit); the rest are listed so the snippet stays a copy of jed's table.

## What you lose

While a jed window has focus: kitty's own Cmd+C (copy screen text; use the mouse selection, which kitty can copy on select), Cmd+N (new OS window), Cmd+W (close tab), Cmd+Q (quit), Cmd+1..3 (switch window) and Cmd+Up/Down (scroll). Other kitty windows keep them. Close jed's tab with Cmd+Shift+W (close OS window) or the menu.

## Checklist

- [ ] Snippet added, kitty restarted
- [ ] Every key in the [table](README.md#cmd-bindings-jed-uses) logs with `SUPER` (see [Checking a terminal](README.md#checking-a-terminal))
- [ ] Cmd+W closes jed's tab, not kitty's
- [ ] In a shell window (no jed), Cmd+W / Cmd+C are kitty's again; after quitting jed too
- [ ] Cmd+Q reaches jed. **Check by hand:** kitty builds its macOS menu from mappings; confirm the Quit menu item no longer grabs Cmd+Q after unmapping
- [ ] **Check by hand:** Cmd+/ (comment line) logs as `SUPER` with `/`; on layouts where `/` is a shifted key (German: Shift+7) the terminal may report Cmd+Shift+7 instead: bind `edit.commentLine` to that in `keybindings.toml`
- [ ] Alt+Left logs `ALT` (Option-as-Alt works)
