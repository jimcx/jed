---
id: snippets
name: Snippets
summary: LSP snippet completions with tab stops, mirrors and choices
status: open
priority: 0
dependencies: []
---

  - Needs: completion (M9: `completion::insert`, the popup), multi-cursor selections, `ChangeSet` mapping (as `diagnostics::Marks`)
  - A snippet parser and a per-view snippet session (tab stops as mapped ranges, Tab / Shift+Tab, `$0`, ending rules), `snippetSupport` advertised, placeholders drawn
  - Done when: with rust-analyzer on a small cargo project, completing a function call inserts `name(arg1, arg2)` with `arg1` selected, Tab moves to `arg2`, Tab again leaves the cursor after `)`; a `match` / `for` postfix snippet with a mirrored name edits both places as one types; Esc ends the session and Tab indents again; the same completion at three cursors gives three sessions' worth of stops moving together; undo removes the insertion in one step; clangd's function-argument placeholders behave the same
  - Subtasks (one agent and one commit each):
    - snippets-a: the model, no UI: `snippet.rs` — the parser (grammar above, a reference test against VS Code's documented examples), resolving variables, laying the text out at each cursor (indentation, EOL) and the stops' ranges; `Session` (stops → ranges per cursor, `map(&ChangeSet)`, next / previous, `contains(selections)` for the ending rule); unit tests
    - snippets-b: in the app: `snippetSupport: true`, `completion::insert` keeps the snippet and starts the session, `Context::Snippet` keys (Tab / Shift+Tab), ending rules (Esc, outside edits / moves, click, undo)
    - snippets-c: choices' list, placeholders drawn and their theme keys
    - snippets-d: the done-when tests (real rust-analyzer and clangd, skipped with a note when absent; the fake server's scripted snippet completions for the rest), the 61k-selection budget unaffected when no session runs, gaps and hand checks; marks the task ✅
