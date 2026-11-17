# Kitty

Checked against kitty **0.49.2** docs:
- https://sw.kovidgoyal.net/kitty/conf/ (default shortcuts, `macos_option_as_alt`)
- https://sw.kovidgoyal.net/kitty/mapping/ (unmapping, `--when-focus-on`)

Snippet below parsed by kitty 0.49.2's own config loader: no warnings, every `map` a conditional mapping on `var:jed or var:jmux`, Option-as-Alt = both. The `"var:jed or var:jmux"` form was re-parsed with kitty 0.47.1 (no warnings); the unquoted form is an error ("or has unknown key").

Config: `~/.config/kitty/kitty.conf`. Kitty keyboard protocol: always available, nothing to enable.

## How releasing works

`map <key>` with **no action** unmaps the shortcut and passes the key to the program ("mapping it to nothing"). Don't use `discard_event`: it drops the key without sending it to the program.

Each line is scoped with `--when-focus-on "var:jed or var:jmux"` (quoted: it is one search expression): jed sets the user var `jed` (OSC 1337 `SetUserVar`) while it runs and jmux sets `jmux` the same way (but no `jmux_workspace`), each cleared on exit (and when it crashes), so the keys are released only while a jed or jmux window has focus; every other kitty window keeps kitty's own shortcuts. `--when-focus-on` takes a kitty search expression (spaces and parentheses are operators, so match on the var, not a title regex; `cmdline:` only sees the window's launch command, not jed started from its shell). A `kill -9`'d jed can't clear the var: run `printf '\e]1337;SetUserVar=jed\a'` in that window.

jed also sets the window title to `jed <workspace> (<branch>)` (the git branch when there is one; `jed <file>` in single file mode; kitty's title stack restores the shell's on exit), the user var `jed_workspace` (the workspace root, for `kitten @ ls`) and OSC 7 (the root as the window's directory, so new windows / tabs opened from jed's start there). Inside tmux the user vars need `set -g allow-passthrough on`.

## Snippet

```conf
# --- jed: send these Cmd keys to the program ---
map --when-focus-on "var:jed or var:jmux" cmd+a
map --when-focus-on "var:jed or var:jmux" cmd+z
map --when-focus-on "var:jed or var:jmux" cmd+shift+z
map --when-focus-on "var:jed or var:jmux" cmd+c
map --when-focus-on "var:jed or var:jmux" cmd+x
map --when-focus-on "var:jed or var:jmux" cmd+slash
map --when-focus-on "var:jed or var:jmux" cmd+shift+k
map --when-focus-on "var:jed or var:jmux" cmd+l
map --when-focus-on "var:jed or var:jmux" cmd+enter
map --when-focus-on "var:jed or var:jmux" cmd+shift+enter
map --when-focus-on "var:jed or var:jmux" cmd+]
map --when-focus-on "var:jed or var:jmux" cmd+[
map --when-focus-on "var:jed or var:jmux" cmd+alt+i
map --when-focus-on "var:jed or var:jmux" cmd+period
map --when-focus-on "var:jed or var:jmux" cmd+alt+v
map --when-focus-on "var:jed or var:jmux" cmd+d
map --when-focus-on "var:jed or var:jmux" cmd+shift+l
map --when-focus-on "var:jed or var:jmux" cmd+alt+d
map --when-focus-on "var:jed or var:jmux" cmd+u
map --when-focus-on "var:jed or var:jmux" cmd+alt+up
map --when-focus-on "var:jed or var:jmux" cmd+alt+down
map --when-focus-on "var:jed or var:jmux" cmd+alt+shift+left
map --when-focus-on "var:jed or var:jmux" cmd+alt+shift+right
map --when-focus-on "var:jed or var:jmux" cmd+alt+shift+up
map --when-focus-on "var:jed or var:jmux" cmd+alt+shift+down
map --when-focus-on "var:jed or var:jmux" cmd+alt+shift+page_up
map --when-focus-on "var:jed or var:jmux" cmd+alt+shift+page_down
map --when-focus-on "var:jed or var:jmux" cmd+left
map --when-focus-on "var:jed or var:jmux" cmd+shift+left
map --when-focus-on "var:jed or var:jmux" cmd+right
map --when-focus-on "var:jed or var:jmux" cmd+shift+right
map --when-focus-on "var:jed or var:jmux" cmd+up
map --when-focus-on "var:jed or var:jmux" cmd+shift+up
map --when-focus-on "var:jed or var:jmux" cmd+down
map --when-focus-on "var:jed or var:jmux" cmd+shift+down
map --when-focus-on "var:jed or var:jmux" cmd+backspace
map --when-focus-on "var:jed or var:jmux" cmd+f
map --when-focus-on "var:jed or var:jmux" cmd+alt+f
map --when-focus-on "var:jed or var:jmux" cmd+alt+c
map --when-focus-on "var:jed or var:jmux" cmd+alt+w
map --when-focus-on "var:jed or var:jmux" cmd+alt+r
map --when-focus-on "var:jed or var:jmux" cmd+alt+x
map --when-focus-on "var:jed or var:jmux" cmd+shift+1
map --when-focus-on "var:jed or var:jmux" cmd+alt+enter
map --when-focus-on "var:jed or var:jmux" cmd+alt+n
map --when-focus-on "var:jed or var:jmux" cmd+alt+shift+n
map --when-focus-on "var:jed or var:jmux" cmd+n
map --when-focus-on "var:jed or var:jmux" cmd+s
map --when-focus-on "var:jed or var:jmux" cmd+shift+s
map --when-focus-on "var:jed or var:jmux" cmd+alt+s
map --when-focus-on "var:jed or var:jmux" cmd+q
map --when-focus-on "var:jed or var:jmux" cmd+w
map --when-focus-on "var:jed or var:jmux" cmd+shift+t
map --when-focus-on "var:jed or var:jmux" cmd+k
map --when-focus-on "var:jed or var:jmux" cmd+b
map --when-focus-on "var:jed or var:jmux" cmd+shift+e
map --when-focus-on "var:jed or var:jmux" cmd+shift+f
map --when-focus-on "var:jed or var:jmux" cmd+shift+h
map --when-focus-on "var:jed or var:jmux" cmd+shift+j
map --when-focus-on "var:jed or var:jmux" cmd+1
map --when-focus-on "var:jed or var:jmux" cmd+2
map --when-focus-on "var:jed or var:jmux" cmd+3
map --when-focus-on "var:jed or var:jmux" cmd+4
map --when-focus-on "var:jed or var:jmux" cmd+5
map --when-focus-on "var:jed or var:jmux" cmd+6
map --when-focus-on "var:jed or var:jmux" cmd+7
map --when-focus-on "var:jed or var:jmux" cmd+8
map --when-focus-on "var:jed or var:jmux" cmd+t
map --when-focus-on "var:jed or var:jmux" cmd+y
map --when-focus-on "var:jed or var:jmux" cmd+backslash
map --when-focus-on "var:jed or var:jmux" cmd+alt+backslash
map --when-focus-on "var:jed or var:jmux" cmd+alt+right
map --when-focus-on "var:jed or var:jmux" cmd+alt+left
map --when-focus-on "var:jed or var:jmux" cmd+ctrl+right
map --when-focus-on "var:jed or var:jmux" cmd+ctrl+left
map --when-focus-on "var:jed or var:jmux" cmd+shift+page_up
map --when-focus-on "var:jed or var:jmux" cmd+shift+page_down
map --when-focus-on "var:jed or var:jmux" cmd+shift+p
map --when-focus-on "var:jed or var:jmux" cmd+,
map --when-focus-on "var:jed or var:jmux" cmd+alt+,
map --when-focus-on "var:jed or var:jmux" cmd+p
map --when-focus-on "var:jed or var:jmux" cmd+j
map --when-focus-on "var:jed or var:jmux" cmd+shift+m
map --when-focus-on "var:jed or var:jmux" cmd+shift+u
map --when-focus-on "var:jed or var:jmux" cmd+alt+z

# --- jed: Option acts as Alt ---
macos_option_as_alt both
```

Don't add `cmd+v`: kitty's paste is how text gets into jed.

Only some of these are kitty defaults (`cmd+c` copy, `cmd+n` new OS window, `cmd+up`/`cmd+down` scroll, `cmd+1..8` switch window, `cmd+t` new tab (jed: focus terminal area 1), `cmd+w` close tab, `cmd+q` quit, `cmd+k` clear the terminal, `cmd+,` edit kitty.conf, `cmd+alt+,` debug the config); the rest are listed so the snippet stays a copy of jed's table.

## Switching between workspaces

"Open Recent Workspace" (Ctrl+R) marks the workspaces open in other jed windows and, on Enter, raises that window (its OS window and tab) or opens a new OS window running `jed <path>`. jed talks to kitty over its remote control socket, so kitty needs:

```
allow_remote_control socket-only
listen_on unix:@kitty-{kitty_pid}
```

- On macOS there are no abstract sockets: use `listen_on unix:/tmp/kitty-{kitty_pid}`. Keep it under `/tmp`: a socket path is limited to 104 bytes there and the long `$TMPDIR` can fail to bind.
- kitty sets `KITTY_LISTEN_ON` (which jed reads) in new windows only: restart kitty after changing the config.
- Inside tmux, pass `KITTY_LISTEN_ON` through (`set -g update-environment KITTY_LISTEN_ON`, or `set -ga update-environment ' KITTY_LISTEN_ON'`).
- Remote control with a `password` is not supported, nor `tcp:` addresses. Without a working socket Enter shows the `jed <path>` command to run.

## What you lose

While a jed or jmux window has focus: kitty's own Cmd+C (copy screen text; use the mouse selection, which kitty can copy on select), Cmd+N (new OS window), Cmd+W (close tab), Cmd+Q (quit), Cmd+K (clear the terminal), Cmd+1..8 (switch window) and Cmd+Up/Down (scroll). Other kitty windows keep them. Close jed's tab with Cmd+Shift+W (close OS window) or the menu.

## Checklist

- [ ] Snippet added, kitty restarted
- [ ] Every key in the [table](README.md#cmd-bindings-jed-uses) logs with `SUPER` (see [Checking a terminal](README.md#checking-a-terminal))
- [ ] Cmd+W closes jed's tab, not kitty's
- [ ] In a shell window (no jed), Cmd+W / Cmd+C are kitty's again; after quitting jed too
- [ ] Cmd+Q reaches jed. **Check by hand:** kitty builds its macOS menu from mappings; confirm the Quit menu item no longer grabs Cmd+Q after unmapping
- [ ] **Check by hand:** Cmd+/ (comment line) logs as `SUPER` with `/`; on layouts where `/` is a shifted key (German: Shift+7) the terminal may report Cmd+Shift+7 instead: bind `edit.commentLine` to that in `keybindings.toml`
- [ ] Alt+Left logs `ALT` (Option-as-Alt works)
