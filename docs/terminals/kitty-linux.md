# Kitty (Linux)

On Linux jed uses VS Code's Linux keys: Ctrl instead of Cmd, no SUPER, and Option/`macos_option_as_alt` doesn't apply (Alt is Alt). Plain Ctrl and Alt combos already reach the program; what kitty takes are its own `ctrl+shift+…` defaults. Release only the ones jed binds.

Config: `~/.config/kitty/kitty.conf`. Kitty keyboard protocol: always available, nothing to enable. Scoping with `--when-focus-on var:jed` works as in [kitty.md](kitty.md#how-releasing-works): other kitty windows keep kitty's shortcuts.

**Skeleton, not yet checked against kitty.** The list is jed's `Platform::Linux` defaults filtered to `ctrl+shift+…` (kitty's namespace). Unmapping a key kitty doesn't bind is harmless. Check with [Checking a terminal](README.md#checking-a-terminal), using `ctrl+shift` where it says `SUPER`.

## Snippet

```conf
# --- jed (Linux): release kitty's ctrl+shift defaults while jed has focus ---
map --when-focus-on var:jed ctrl+shift+1
map --when-focus-on var:jed ctrl+shift+comma
map --when-focus-on var:jed ctrl+shift+minus
map --when-focus-on var:jed ctrl+shift+period
map --when-focus-on var:jed ctrl+shift+e
map --when-focus-on var:jed ctrl+shift+f
map --when-focus-on var:jed ctrl+shift+g
map --when-focus-on var:jed ctrl+shift+h
map --when-focus-on var:jed ctrl+shift+i
map --when-focus-on var:jed ctrl+shift+j
map --when-focus-on var:jed ctrl+shift+l
map --when-focus-on var:jed ctrl+shift+m
map --when-focus-on var:jed ctrl+shift+p
map --when-focus-on var:jed ctrl+shift+s
map --when-focus-on var:jed ctrl+shift+u
map --when-focus-on var:jed ctrl+shift+z
map --when-focus-on var:jed ctrl+shift+left
map --when-focus-on var:jed ctrl+shift+right
map --when-focus-on var:jed ctrl+shift+up
map --when-focus-on var:jed ctrl+shift+down
map --when-focus-on var:jed ctrl+shift+home
map --when-focus-on var:jed ctrl+shift+end
map --when-focus-on var:jed ctrl+shift+page_up
map --when-focus-on var:jed ctrl+shift+page_down
```

Don't add `ctrl+shift+v` / `ctrl+shift+c` unless you want them gone: kitty's paste is how text gets into jed (bracketed paste), and jed copies with `ctrl+c`.

## TODO

- [ ] Run the snippet through kitty's config loader (as kitty.md did) and fix any warnings
- [ ] Press every Linux binding with `JED_LOG=input=trace` and confirm it logs with the right modifiers
- [ ] Decide whether to list jed's Linux bindings in a README table (the Cmd-table test doesn't cover them)
- [ ] Kitty defaults that still clash on `ctrl+alt+…` / `alt+…` under some desktops (the window manager may grab `ctrl+alt+arrows`, `ctrl+alt+d`)
