---
id: reopen_closed_tab
name: Reopen closed tab
summary: reopen the last closed tab (Cmd+Shift+T)
status: done
priority: 0
dependencies: []
---

Cmd+Shift+T; a stack of closed tabs per window with their view state, files reopened from disk; in the palette; Cmd bindings update `docs/terminals/`

- ✅ Reopen closed tab
  - `src/app/reopen.rs`: `remember_closed` (called first in `discard_tab`, so every close path records) keeps a stack `app.closed` of `navigation::Entry` (group, path, selection / scroll `Pos`), at most 50, one entry per file; only text tabs of file buffers (not untitled, log, commit, diff, hex, preview, shell, terminal). `Command::ReopenClosedTab` (`tab.reopenClosed`, "View: Reopen Closed Editor", Cmd+Shift+T / Ctrl+Shift+T) pops the newest, focuses its group (else the active one) and opens it with `Goto::Place`; a file gone from disk is skipped via `reopen_opened` (called from `opened`) and the next entry tried; an empty stack notifies.
  - Also: `docs/terminals/` README row and kitty, kitty-linux, ghostty, wezterm and alacritty snippets for `cmd+shift+t`.
  - Deferred: hex tabs aren't recorded; the 50-entry cap has no test.
  - Still to check by hand: Cmd+Shift+T in each terminal with its snippet applied (iTerm2 has no snippet); Ctrl+Shift+T on Linux isn't intercepted by the terminal.
