---
id: indent_outdent
name: Indent / outdent lines
summary: Tab with a multi-line selection indents, Shift+Tab / Cmd+] / Cmd+[ outdent, one undo step
status: done
priority: 0
dependencies: []
---

  - `src/shift.rs` (VS Code's `ShiftCommand` / `TypeOperations.tab`): `Command::IndentLines` (`edit.indentLines`, Cmd+] / Ctrl+]) and `OutdentLines` (`edit.outdentLines`, Cmd+[ / Ctrl+[ and Shift+Tab), editor context only; indentation goes to the next / previous tab-width multiple, written as the language's `insert_spaces`; empty lines skipped unless a selection is alone on one (whitespace-only lines are indented, as VS Code); Tab decides per selection (over lines or a whole line: indent; else a tab there, one step), falling back to `insert_tab`; release budget `milestone_indent_lines_budget_at_61k_selections` (~18 ms at 61k)
  - Deferred: indentation detection (tab width = indent size); keeping `onEnterRules` alignment; Tab on a whitespace-only line jumping to the expected indentation; a cursor before a multi-line selection on the same line leaves that line unindented (VS Code indents it)
  - Still to check by hand: Linux terminals without the kitty keyboard protocol send Ctrl+[ as Esc and Ctrl+] as Ctrl+5 (only Shift+Tab works there); the terminal snippets' `cmd+[` / `cmd+]` spellings in Ghostty (maybe `bracket_left`), Alacritty and iTerm2's freed pane menu items
