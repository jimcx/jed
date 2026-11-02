---
id: more_line_commands
name: More line commands
summary: delete, select, join, and insert lines
status: done
priority: 0
dependencies: []
---

Delete line (Cmd+Shift+K), select line (Cmd+L, repeated: the next line too), join lines (Ctrl+J on macOS; unbound on Linux, where Ctrl+J is the panel), insert line below / above (Cmd+Enter / Cmd+Shift+Enter, with auto-indent); at every selection (`LineWalker`, one undo step each, the line-command budget); Cmd bindings update `docs/terminals/`

  - Built: `DeleteLine` (Cmd+Shift+K), `SelectLine` (Cmd+L; a selection that already is whole lines grows by the next line; seals undo), `JoinLines` (Ctrl+J, bound on macOS only), `InsertLineBelow` (Cmd+Enter) and `InsertLineAbove` (Cmd+Shift+Enter, indent copied). All in `lines.rs` through `LineWalker` (`finish_with` split from `finish` for commands that change the selection count), one `EditKind::Other` undo step, gated by `edit::is_edit` for read-only buffers. Names `editor.action.deleteLines`, `expandLineSelection`, `joinLines`, `insertLineAfter`, `insertLineBefore`
  - Semantics: delete merges touching lines and leaves one cursor at the same column on the next line (previous line for the last); join uses one space (none before `)`, beside an empty line or after trailing blanks) and a multi-line selection collapses to one cursor at the last join; insert below copies the line's indent even with `auto_indent` off
  - Docs: four rows in `docs/terminals/README.md` and the kitty / ghostty / wezterm / alacritty snippets; iterm2 and ghostty note that Cmd+Enter is the terminal's fullscreen toggle
  - Tests: `todo_tests::more_line_commands` (incl. the 31k-cursor `milestone_more_line_commands_scale_with_many_cursors`); the release 61k-selection line-command budget still passes
  - Deferred: a multi-line join selection does not stay selected
  - Still to check by hand: Cmd+Enter, Cmd+L and Cmd+Shift+K reaching jed in kitty / ghostty / iTerm2 / WezTerm / Alacritty (Cmd+Enter is fullscreen in some), and Ctrl+J on macOS
