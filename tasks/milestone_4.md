---
id: milestone_4
name: Clipboard
summary: copy, cut, paste
status: done
priority: 0
dependencies: []
---

   - Needs: selection list and undo grouping (M1), bracketed paste (M1, `edit::paste`)
   - Needed by: multi-cursor copy/paste (M6), line commands (M6)
   - `clipboard` module: `arboard` locally; OSC 52 when `SSH_TTY` is set or `arboard` fails
   - Copy / cut commands (Cmd+C / Cmd+X); cut is one undo step, like paste
   - Empty selection copies / cuts the whole line; pasting that line inserts above the cursor's line
   - Copy works on the whole selection list from the start (joined with newlines), so M6 only adds the paste spreading
   - Paste: Cmd+V stays with the terminal on macOS (bracketed paste); Ctrl+V reads `arboard` on Linux
   - Terminal snippets: pass Cmd+C / Cmd+X through
   - Done when: copy in jed → paste in another app → copy there → paste back into jed, both locally and over SSH; empty-selection copy/cut/paste matches VS Code
   - Implemented; automated tests cover the editing rules with an in-memory clipboard. Still to check by hand: the round trip with another app locally (`arboard`) and over SSH (OSC 52), incl. inside tmux
