# Kitty (Linux)

On Linux jed uses VS Code's Linux keys: Ctrl instead of Cmd, no SUPER, and Option/`macos_option_as_alt` doesn't apply (Alt is Alt). Plain Ctrl and Alt combos already reach the program; what kitty takes are its own `ctrl+shift+…` defaults. Release only the ones jed and jmux bind (jmux adds `ctrl+shift+b`, `d`, `r` and `w`).

Config: `~/.config/kitty/kitty.conf`. Kitty keyboard protocol: always available, nothing to enable. Scoping with `--when-focus-on "var:jed or var:jmux"` (jmux sets the user var `jmux` as jed sets `jed`) works as in [kitty.md](kitty.md#how-releasing-works): other kitty windows keep kitty's shortcuts.

**Skeleton, not yet checked against kitty.** The list is jed's `Platform::Linux` defaults filtered to `ctrl+shift+…` (kitty's namespace). Unmapping a key kitty doesn't bind is harmless. Check with [Checking a terminal](README.md#checking-a-terminal), using `ctrl+shift` where it says `SUPER`.

## Snippet

```conf
# --- jed and jmux (Linux): release kitty's ctrl+shift defaults while they have focus ---
map --when-focus-on "var:jed or var:jmux" ctrl+shift+1
map --when-focus-on "var:jed or var:jmux" ctrl+shift+c
map --when-focus-on "var:jed or var:jmux" ctrl+shift+comma
map --when-focus-on "var:jed or var:jmux" ctrl+shift+minus
map --when-focus-on "var:jed or var:jmux" ctrl+shift+period
map --when-focus-on "var:jed or var:jmux" ctrl+shift+b
map --when-focus-on "var:jed or var:jmux" ctrl+shift+d
map --when-focus-on "var:jed or var:jmux" ctrl+shift+e
map --when-focus-on "var:jed or var:jmux" ctrl+shift+f
map --when-focus-on "var:jed or var:jmux" ctrl+shift+g
map --when-focus-on "var:jed or var:jmux" ctrl+shift+h
map --when-focus-on "var:jed or var:jmux" ctrl+shift+i
map --when-focus-on "var:jed or var:jmux" ctrl+shift+j
map --when-focus-on "var:jed or var:jmux" ctrl+shift+l
map --when-focus-on "var:jed or var:jmux" ctrl+shift+m
map --when-focus-on "var:jed or var:jmux" ctrl+shift+p
map --when-focus-on "var:jed or var:jmux" ctrl+shift+r
map --when-focus-on "var:jed or var:jmux" ctrl+shift+s
map --when-focus-on "var:jed or var:jmux" ctrl+shift+t
map --when-focus-on "var:jed or var:jmux" ctrl+shift+u
map --when-focus-on "var:jed or var:jmux" ctrl+shift+w
map --when-focus-on "var:jed or var:jmux" ctrl+shift+z
map --when-focus-on "var:jed or var:jmux" ctrl+shift+left
map --when-focus-on "var:jed or var:jmux" ctrl+shift+right
map --when-focus-on "var:jed or var:jmux" ctrl+shift+up
map --when-focus-on "var:jed or var:jmux" ctrl+shift+down
map --when-focus-on "var:jed or var:jmux" ctrl+shift+home
map --when-focus-on "var:jed or var:jmux" ctrl+shift+end
map --when-focus-on "var:jed or var:jmux" ctrl+shift+page_up
map --when-focus-on "var:jed or var:jmux" ctrl+shift+page_down
```

`ctrl+shift+c` is released because jed holds the mouse: a drag or double-click in a jed terminal is jed's selection, not kitty's, so kitty's copy would copy nothing and leave the clipboard as it was. jed copies it with `ctrl+shift+c`; `ctrl+c` is always the program's (SIGINT). Don't add `ctrl+shift+v`: kitty's paste is how text gets into jed (bracketed paste). Shift+drag still makes a kitty selection.

## TODO

- [ ] Run the snippet through kitty's config loader (as kitty.md did) and fix any warnings
- [ ] Press every Linux binding with `JED_LOG=input=trace` and confirm it logs with the right modifiers
- [ ] Decide whether to list jed's Linux bindings in a README table (the Cmd-table test doesn't cover them)
- [ ] Kitty defaults that still clash on `ctrl+alt+…` / `alt+…` under some desktops (the window manager may grab `ctrl+alt+arrows`, `ctrl+alt+d`)
