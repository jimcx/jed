---
id: milestone_6
name: Multi-cursor and find
summary: Cmd+D, Cmd+Shift+L, add above/below, Alt+Click, box select, paste spread across cursors, find/replace in the file
status: done
priority: 0
dependencies: []
---

   - Needs: selection list (M1), clipboard (M4), key contexts (M5)
   - Needed by: workspace search (M7, the matcher), auto-close and auto-indent at every cursor (M8), completion at every cursor (M9)
   - `search` module first: plain / regex, case, whole word, matching on the rope without copying it into a string
     - Cmd+D, Cmd+Shift+L and the find widget all use it
   - Every command in the multi-cursor table; Cmd+D matching exactly as VS Code
   - Find/replace widget (Cmd+F / Cmd+Alt+F): case / whole word / regex toggles; replace all is one undo step; Alt+Enter selects every match
   - Line commands at every cursor: move line (Alt+Up/Down), duplicate line (Shift+Alt+Up/Down)
   - Paste spreads one line per cursor when the counts match ([Clipboard](../PLAN.md#clipboard))
   - Rendering: extra cursors as reversed cells, a selection background for each, "N selections" in the status bar
   - Same buffer in several groups (M3): every view's selections still map through multi-cursor edits
   - Done when: Cmd+Shift+L on a 1 MB file with tens of thousands of matches, then typing, stays smooth; one undo restores the selection list from before; replace-all on that file is one undo step
   - Tasks (one agent and one commit each):
     - ✅ 6a: `search` module — `search::Matcher` (plain / regex, case, whole word as VS Code), find next / previous with wrap, find all / in range, line by line on the rope; implements `grep_matcher::Matcher` for M7
     - ✅ 6b: Cmd+D, Cmd+Shift+L, Cmd+Alt+D, Cmd+U (cursor undo), Esc to one cursor; drawing extra cursors and selections, "N selections"; the 1 MB performance target
       - `multicursor::Session` (VS Code's MultiCursorSession) and `app/cursors.rs`; `Selections` tracks the newest selection; `View::CursorHistory` (50 states, cleared by any text change)
       - Session ends on any other selection change, Cmd+U, or focus leaving the editor; skips matches already selected (VS Code would merge)
       - Perf: `ChangeSet::mapper()` (binary search) for other views, one-pass rope rebuild from 256 changes, editor draws only visible selections; 1 MB / 63k matches: Cmd+Shift+L 3 ms, 5.5 ms a key, undo 15 ms
       - Not done: cursor redo (no VS Code default key); word characters are `is_word_char`, not VS Code's `wordSeparators`
       - Still to check by hand: Cmd+D / Cmd+Shift+L / Cmd+Alt+D / Cmd+U reach jed in each terminal (Ghostty and iTerm2 split on Cmd+D; iTerm2's Cmd+U menu item is unverified)
     - ✅ 6c: add cursor above / below, Shift+Alt+I, Alt+Click, box select (mouse and keyboard), paste spread across cursors, move / duplicate line at every cursor
       - `column.rs` (add cursor above / below, cursors at line ends, box select as VS Code's ColumnSelection) and `lines.rs` (move / copy lines as blocks, one undo step, no-op adds none); `clipboard::Copied { text, whole_line, pieces }`; `pos::Line` / `LineWalker` / `line_bounds` and an ASCII fast path for 60k-cursor speed
       - Paste spread as VS Code's `multiCursorPaste: spread`; copying from several empty cursors is no longer whole-line (VS Code marks only one cursor), changing M4's behaviour
       - Also (separate commit before 6c): typing at 63k cursors sped up for slower machines (one-string `ChangeSet`, `Arc` change sets, copy-on-write `Selections`)
       - Not done: move lines doesn't re-indent (M8); middle-button box select; a mix of empty and non-empty selections copies `""` for the empty ones (VS Code copies their lines) and cut leaves those lines
       - Still to check by hand: Shift+Alt+drag and Alt+Click reach jed (terminals keep these for their own selection); Cmd+Alt+Up/Down vs Ghostty / iTerm2 pane keys; Cmd+Shift+Alt+arrows / PageUp / PageDown arrive with Super; Shift+Alt+I needs Option-as-Alt; on Linux GNOME takes Ctrl+Alt+Up/Down and Ctrl+Shift+Alt+arrows
     - ✅ 6d: find / replace widget (Cmd+F / Cmd+Alt+F) and its key context; owns the find options that Cmd+D on selected text borrows (`App::find_options`); replace all as one undo step; Alt+Enter selects every match; marks M6 ✅
       - `find.rs` (model), `app/find.rs`, `ui/find.rs`; `find` and `replace` key contexts (replace → find → global) for `keybindings.toml`; match cache keyed on `Buffer::revision()`, drawn only in view; `search::Replacement` (`$1`, `$&`, `$$`, `\n` …)
       - Keys: Cmd+F / Cmd+Alt+F (Ctrl+F / Ctrl+H on Linux), Enter / Shift+Enter and F3 / Shift+F3, toggles Cmd+Alt+C/W/R (Alt+C/W/R on Linux) also from the editor, Cmd+Shift+1 replace one, Cmd+Alt+Enter replace all (one undo step), Alt+Enter selects every match; first Esc in the editor closes the widget
       - `KeyCombo` maps a shifted US-layout digit with Cmd/Ctrl/Alt back to digit + Shift (`cmd+shift+1`); on non-US layouts with `!` on its own key, Ctrl+`!` runs Replace One
       - 1 MB / 63k matches (Linux i7-2600K): find key + frame 10 ms, replace all + frame 11 ms, undo 4 ms
       - Not done: Cmd+G / Cmd+Shift+G (Ctrl+G is go to line on Linux); undo inside the widget's inputs; find/replace history, preserve case, find in selection; a match under the widget isn't scrolled clear; one widget for the app rather than per editor; the count doesn't refresh on background events (e.g. a file opening) until the next key
       - Still to check by hand: Cmd+F vs each terminal's own search (Ghostty, WezTerm, Alacritty, iTerm2's Find… menu); Cmd+Shift+1 arrives as Cmd+`!`; Cmd+Alt+C/W/R/F/Enter need Option-as-Alt; Alt+Enter and Shift+Enter keep their modifiers; on Linux Ctrl+H needs the kitty protocol (else Backspace)
