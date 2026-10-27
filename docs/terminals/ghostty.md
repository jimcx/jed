# Ghostty

Checked on 2026-10-14 against:
- https://ghostty.org/docs/config/reference (`keybind`, `macos-option-as-alt`, config path)
- https://ghostty.org/docs/config/keybind/reference (`unbind`, `ignore`)
- default bindings from `src/config/Config.zig` on the `main` branch (https://github.com/ghostty-org/ghostty)

Config: `~/.config/ghostty/config.ghostty` per the current docs (older releases read `~/.config/ghostty/config`). Kitty keyboard protocol: always available, nothing to enable.

## How releasing works

`keybind = <trigger>=unbind` removes a binding; the docs say the key is then "sent through to the child command if it is printable". Don't use `ignore`: Ghostty then neither acts on the key nor forwards it.

`unbind` "cannot unbind bindings that were not bound by Ghostty or the user", so keys Ghostty doesn't bind are unaffected by their lines (harmless).

## Snippet

```conf
# --- jed: send these Cmd keys to the program ---
keybind = cmd+a=unbind
keybind = cmd+z=unbind
keybind = cmd+shift+z=unbind
keybind = cmd+c=unbind
keybind = cmd+x=unbind
keybind = cmd+slash=unbind
keybind = cmd+]=unbind
keybind = cmd+[=unbind
keybind = cmd+alt+i=unbind
keybind = cmd+period=unbind
keybind = cmd+d=unbind
keybind = cmd+shift+l=unbind
keybind = cmd+alt+d=unbind
keybind = cmd+u=unbind
keybind = cmd+alt+arrow_up=unbind
keybind = cmd+alt+arrow_down=unbind
keybind = cmd+alt+shift+arrow_left=unbind
keybind = cmd+alt+shift+arrow_right=unbind
keybind = cmd+alt+shift+arrow_up=unbind
keybind = cmd+alt+shift+arrow_down=unbind
keybind = cmd+alt+shift+page_up=unbind
keybind = cmd+alt+shift+page_down=unbind
keybind = cmd+arrow_left=unbind
keybind = cmd+shift+arrow_left=unbind
keybind = cmd+arrow_right=unbind
keybind = cmd+shift+arrow_right=unbind
keybind = cmd+arrow_up=unbind
keybind = cmd+shift+arrow_up=unbind
keybind = cmd+arrow_down=unbind
keybind = cmd+shift+arrow_down=unbind
keybind = cmd+backspace=unbind
keybind = cmd+f=unbind
keybind = cmd+alt+f=unbind
keybind = cmd+alt+c=unbind
keybind = cmd+alt+w=unbind
keybind = cmd+alt+r=unbind
keybind = cmd+alt+x=unbind
keybind = cmd+shift+1=unbind
keybind = cmd+alt+enter=unbind
keybind = cmd+alt+n=unbind
keybind = cmd+alt+shift+n=unbind
keybind = cmd+n=unbind
keybind = cmd+s=unbind
keybind = cmd+shift+s=unbind
keybind = cmd+alt+s=unbind
keybind = cmd+q=unbind
keybind = cmd+w=unbind
keybind = cmd+k=unbind
keybind = cmd+b=unbind
keybind = cmd+shift+e=unbind
keybind = cmd+shift+f=unbind
keybind = cmd+shift+h=unbind
keybind = cmd+shift+j=unbind
keybind = cmd+1=unbind
keybind = cmd+2=unbind
keybind = cmd+3=unbind
keybind = cmd+4=unbind
keybind = cmd+5=unbind
keybind = cmd+6=unbind
keybind = cmd+7=unbind
keybind = cmd+8=unbind
keybind = cmd+backslash=unbind
keybind = cmd+alt+backslash=unbind
keybind = cmd+alt+arrow_right=unbind
keybind = cmd+alt+arrow_left=unbind
keybind = cmd+ctrl+arrow_right=unbind
keybind = cmd+ctrl+arrow_left=unbind
keybind = cmd+shift+page_up=unbind
keybind = cmd+shift+page_down=unbind
keybind = cmd+shift+p=unbind
keybind = cmd+p=unbind
keybind = cmd+j=unbind
keybind = cmd+shift+m=unbind
keybind = cmd+shift+u=unbind
keybind = cmd+alt+z=unbind

# --- jed: Option acts as Alt ---
macos-option-as-alt = true
```

Don't unbind `cmd+v`: Ghostty's paste is how text gets into jed.

Ghostty defaults that clash with jed (macOS): `cmd+a` select all, `cmd+c` copy, `cmd+n` new window, `cmd+z`/`cmd+shift+z` undo/redo, `cmd+left`/`cmd+right`/`cmd+backspace` send line-editing control codes, `cmd+up`/`cmd+down` and `cmd+shift+up`/`cmd+shift+down` jump to prompt, `cmd+q` quit, `cmd+w` close split/tab, `cmd+k` clear screen (jed's Terminal: Clear in a shell or task tab), `cmd+1..8` go to tab, `cmd+alt+arrows` move between splits (jed: Cmd+Alt+Up/Down add a cursor above/below), `cmd+shift+p` Ghostty's own command palette (recent releases), `cmd+d` new split right (jed's Cmd+D, add next match), `cmd+f` Ghostty's search (recent releases; jed's find), `cmd+alt+i` the terminal inspector (jed's show hover), `cmd+[` / `cmd+]` previous / next split (jed's outdent / indent lines), `cmd+ctrl+arrows` resize the split (jed's Cmd+Ctrl+Left/Right move a tab between groups).

`macos-option-as-alt` already defaults to `true` on the U.S. and U.S. International layouts; setting it makes other layouts behave the same (`left`/`right` limit it to one Option key).

## What you lose

Ghostty's Cmd+C (copy screen text), Cmd+N (new window), Cmd+W, Cmd+Q, Cmd+K (clear screen), Cmd+1..8, Cmd+A, Cmd+Z, prompt jumping, Cmd+[ / Cmd+] (previous / next split), Cmd+D (new split right; Cmd+Shift+D still splits down), Cmd+Alt+arrow split navigation, Cmd+Ctrl+Left/Right split resizing and the terminal inspector (Cmd+Alt+I) in every Ghostty window. Close with Cmd+Alt+W (close tab) or Cmd+Shift+W (close window). Ghostty has no per-program config, so this can't be scoped to jed.

## Checklist

- [ ] Snippet added, Ghostty restarted (or config reloaded with Cmd+Shift+,)
- [ ] Every key in the [table](README.md#cmd-bindings-jed-uses) logs with `SUPER`. **Check by hand:** the docs only promise pass-through for *printable* keys; confirm Cmd+arrows and Cmd+Backspace arrive too
- [ ] **Check by hand:** "bindings in the main menu will trigger before any remapping"; confirm Cmd+Q and Cmd+W reach jed and don't still fire the menu items
- [ ] **Check by hand:** `cmd+[` / `cmd+]` free Ghostty's split navigation (if Cmd+[ still switches splits, try `cmd+bracket_left` / `cmd+bracket_right`)
- [ ] **Check by hand:** `cmd+period` (Cmd+., quick fix) is spelled right for your Ghostty (try `cmd+.` if Ghostty reports an invalid key)
- [ ] `cmd+backslash` is spelled right for your Ghostty (try `cmd+\` if Ghostty reports an invalid key)
- [ ] **Check by hand:** Cmd+Shift+P opens jed's palette, not Ghostty's
- [ ] **Check by hand:** Cmd+F opens jed's find widget, not Ghostty's search
- [ ] **Check by hand:** Cmd+/ (comment line) logs as `SUPER` with `/`; on layouts where `/` is a shifted key (German: Shift+7) the terminal may report Cmd+Shift+7 instead: bind `edit.commentLine` to that in `keybindings.toml`
- [ ] Alt+Left logs `ALT`
