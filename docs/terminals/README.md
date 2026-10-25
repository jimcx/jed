# Terminal setup for jed (macOS)

jed uses VS Code's Cmd shortcuts. Most terminals keep many Cmd keys for themselves, so out of the box:

- the terminal acts on the key and jed never sees it (Cmd+1 switches terminal tab, Cmd+Up scrolls, …)
- **Cmd+W can close the terminal tab or window, losing unsaved work in jed** (jed's crash backup only covers panics, not a killed terminal)
- Option types characters (∂, ˆ, …) instead of acting as Alt, so Alt bindings (word movement, Cmd+Alt+…) don't arrive

Each file below has a snippet that releases jed's Cmd keys and turns Option into Alt.

| Terminal | Kitty keyboard protocol | File |
|---|---|---|
| Kitty | always available | [kitty.md](kitty.md) |
| Ghostty | always available | [ghostty.md](ghostty.md) |
| WezTerm | **off by default** — snippet turns it on | [wezterm.md](wezterm.md) |
| iTerm2 | setting — snippet turns it on | [iterm2.md](iterm2.md) |
| Alacritty | always available (0.13+) | [alacritty.md](alacritty.md) |

Terminal.app doesn't support the kitty keyboard protocol and can't send Cmd keys; it isn't supported (stretch goal in PLAN.md). Ctrl+Q always quits jed, in any terminal.

## Cmd bindings jed uses

Every macOS binding with Cmd in jed's default keymap. **Keep this table in sync with `src/config/keymap.rs`**: a unit test (`terminal_docs_list_every_cmd_binding`) fails if a Cmd binding is missing here or listed but no longer bound. When the table changes, update every snippet too.

Canonical spelling (the test parses the first column): modifiers in the order `cmd`, `ctrl`, `alt`, `shift`; lowercase key names; `backslash` for `\`.

| Key | Action | Where |
|---|---|---|
| `cmd+a` | Select all | editor |
| `cmd+z` | Undo | editor |
| `cmd+shift+z` | Redo | editor |
| `cmd+c` | Copy (whole line if nothing selected) | editor |
| `cmd+x` | Cut (whole line if nothing selected) | editor |
| `cmd+/` | Toggle line comment | editor |
| `cmd+]` | Indent lines | editor |
| `cmd+[` | Outdent lines (also Shift+Tab) | editor |
| `cmd+alt+i` | Show hover (VS Code: Cmd+K Cmd+I; no chords) | editor |
| `cmd+.` | Quick fix (the code actions at the cursor) | editor |
| `cmd+d` | Add selection at next match | editor |
| `cmd+shift+l` | Select all matches | editor |
| `cmd+alt+d` | Move last selection to next match | editor |
| `cmd+u` | Undo last cursor change | editor |
| `cmd+alt+up` | Add cursor above | editor |
| `cmd+alt+down` | Add cursor below | editor |
| `cmd+alt+shift+left` | Column (box) select left | editor |
| `cmd+alt+shift+right` | Column (box) select right | editor |
| `cmd+alt+shift+up` | Column (box) select up | editor |
| `cmd+alt+shift+down` | Column (box) select down | editor |
| `cmd+alt+shift+pageup` | Column (box) select page up | editor |
| `cmd+alt+shift+pagedown` | Column (box) select page down | editor |
| `cmd+left` | Line start | editor |
| `cmd+shift+left` | Select to line start | editor |
| `cmd+right` | Line end | editor |
| `cmd+shift+right` | Select to line end | editor |
| `cmd+up` | File start | editor |
| `cmd+shift+up` | Select to file start | editor |
| `cmd+down` | File end (editor) / open (explorer) | editor, explorer |
| `cmd+shift+down` | Select to file end | editor |
| `cmd+backspace` | Delete to line start (editor) / delete (explorer) / dismiss a result (Search view's results) | editor, explorer, searchResults |
| `cmd+f` | Find (opens the find widget) | editor, find |
| `cmd+alt+f` | Find and replace | editor, find |
| `cmd+alt+c` | Toggle match case (find widget, Search view) | editor, find, search |
| `cmd+alt+w` | Toggle match whole word (find widget, Search view) | editor, find, search |
| `cmd+alt+r` | Toggle regular expression (find widget, Search view) | editor, find, search |
| `cmd+alt+x` | Toggle hex / ASCII bytes (find widget on a hex tab) | editor, find |
| `cmd+shift+1` | Replace one (find widget) / replace the selected result's line or file (Search view's results); terminals report it as Cmd+`!`, jed reads both | editor, find, searchResults |
| `cmd+alt+enter` | Replace all (find widget, Search view) | editor, find, search, searchResults |
| `cmd+alt+n` | New file | explorer |
| `cmd+alt+shift+n` | New folder | explorer |
| `cmd+n` | New untitled file | everywhere |
| `cmd+s` | Save | everywhere |
| `cmd+shift+s` | Save As (prompts for a path; not in single file mode) | everywhere |
| `cmd+alt+s` | Save All (untitled files ask for a path one at a time) | everywhere |
| `cmd+q` | Quit | everywhere |
| `cmd+w` | Close tab (a task tab in the panel: closes it, killing its task) | everywhere, task |
| `cmd+b` | Toggle sidebar | everywhere |
| `cmd+shift+e` | Focus explorer | everywhere |
| `cmd+shift+f` | Search in workspace (Search view, seeded with the selection) | everywhere |
| `cmd+shift+h` | Replace in workspace (Search view with its replace input) | everywhere |
| `cmd+shift+j` | Toggle search details (the Search view's files to include / exclude) | search |
| `cmd+1` | Focus group 1 | everywhere |
| `cmd+2` | Focus group 2 | everywhere |
| `cmd+3` | Focus group 3 | everywhere |
| `cmd+4` | Focus group 4 | everywhere |
| `cmd+5` | Focus group 5 | everywhere |
| `cmd+6` | Focus group 6 | everywhere |
| `cmd+7` | Focus group 7 | everywhere |
| `cmd+8` | Focus group 8 | everywhere |
| `cmd+backslash` | Split right | everywhere |
| `cmd+alt+backslash` | Split down | everywhere |
| `cmd+alt+right` | Next tab | everywhere |
| `cmd+alt+left` | Previous tab | everywhere |
| `cmd+ctrl+right` | Move the tab into the next group (a new split right if none) | everywhere |
| `cmd+ctrl+left` | Move the tab into the previous group | everywhere |
| `cmd+shift+pageup` | Move the tab left in its group | everywhere |
| `cmd+shift+pagedown` | Move the tab right in its group | everywhere |
| `cmd+shift+p` | Command palette | everywhere |
| `cmd+p` | Quick open (go to file) | everywhere |
| `cmd+j` | Toggle the bottom panel (Problems) | everywhere |
| `cmd+shift+m` | Toggle Problems (show and focus the list, or hide the panel) | everywhere |
| `cmd+shift+u` | Show the Tools view (VS Code's Output) | everywhere |
| `cmd+alt+z` | Toggle word wrap | editor |

**Mouse with modifiers.** Alt+Click adds or removes a cursor and Shift+Alt+drag makes a box selection. Most terminals keep Shift+drag for their own text selection while a program uses the mouse (kitty `terminal_select_modifiers`, WezTerm `bypass_mouse_reporting_modifiers`), and some use Alt or Cmd+Alt with the mouse for rectangle selection, so these may not reach jed. The keyboard box select (Cmd+Shift+Alt+arrows) always works.

Shift+Tab (outdent lines) needs no terminal change. The palette is also on F1, go to line is Ctrl+G and the Source Control view Ctrl+Shift+G (as VS Code on macOS); none needs a terminal change. Neither do the find widget's other keys: Enter / Shift+Enter and F3 / Shift+F3 (next / previous match), Alt+Enter (select all matches), Tab (find ↔ replace) and Esc. The Search view's Esc (back to the editor) needs nothing either, nor do F8 / Shift+F8 (next / previous problem), the Problems list's arrows, Enter and Esc, the Source Control list's arrows, Enter, Space and Esc, and the Tools view's arrows, Enter and Esc.

**Leave Cmd+V with the terminal.** jed doesn't bind it: the terminal's paste arrives in jed as a bracketed paste, which works the same locally and over SSH. Cmd+C and Cmd+X must reach jed, since the terminal's own copy takes text off the screen, not jed's selection.

## Checking a terminal

1. Apply the snippet and restart the terminal.
2. Run `JED_LOG=input=trace jed <some file>`.
3. Press every key in the table, then quit with Ctrl+Q.
4. In the newest file in `~/.local/state/jed/logs/`, each key should appear as a line like
   `TRACE input: key=KeyEvent { code: Char('w'), modifiers: KeyModifiers(SUPER), … }`.
   A key with no line was kept by the terminal; a key logged without `SUPER` lost its Cmd.
5. Check Option: Alt+Left should log `modifiers: KeyModifiers(ALT)`, not a typed character.

The status bar warns at start-up if the terminal doesn't speak the kitty keyboard protocol.
