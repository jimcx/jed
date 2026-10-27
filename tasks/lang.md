---
id: lang
name: Language features
summary: go to symbol, signature help and breadcrumbs / sticky scroll (code actions are in Diagnostics and quick fixes)
status: open
priority: 0
dependencies: []
---

  - Needs: language servers (M9), completion's popup, kind icons and request handling (`App::send_request`), quick open (M5), back / forward navigation (see [Tasks](../PLAN.md#tasks); jumps recorded when it's there)
  - Go to symbol in file (Cmd+Shift+O, `@`) and workspace (Cmd+T, `#`); signature help (trigger characters, Cmd+Shift+Space); breadcrumbs and sticky scroll (settings, off by default); Cmd bindings update `docs/terminals/`
  - Done when: with rust-analyzer on a small cargo project: Cmd+Shift+O lists the file's items with their containers and Enter jumps (Esc after moving the selection goes back); Cmd+T finds a struct in another file; typing `foo(` shows its signature with the active parameter moving on `,` and closing on `)`; with breadcrumbs on, the row shows path › impl › fn following the cursor; clangd does the same on a C file
  - Subtasks (one agent and one commit each):
    - lang-a: go to symbol in file: `documentSymbol` (both shapes: `DocumentSymbol[]` tree, `SymbolInformation[]`) → a symbols model, the `@` quick open mode and Cmd+Shift+O, preview while moving, Esc restores
    - lang-b: go to symbol in workspace: `workspace/symbol`, `#` mode and Cmd+T, debounced re-requests with older ones cancelled, rows with the file
    - lang-c: signature help: the popup, trigger / retrigger characters, the active parameter, overloads, closing rules, next to the completion list
    - lang-d: breadcrumbs (setting, off by default): the row above the editor from `documentSymbol` (the tree-sitter tree without a server), following the cursor
    - lang-e: sticky scroll (setting, off by default): the enclosing scopes' first lines pinned at the top while scrolling, from the same model
    - lang-f: the done-when tests (real rust-analyzer and clangd, skipped with a note when absent), gaps and hand checks; marks the task ✅
