---
id: live_diagnostics
name: Live diagnostics while editing
summary: pull diagnostics after a typing pause, per server
status: open
priority: 0
dependencies: []
---

See [LSP](../PLAN.md#lsp). Squiggles follow the unsaved text as the user types, for every server. Today each edit already goes out as an incremental `didChange` each loop turn and versioned publishes are mapped through later edits (`lsp/diagnostics.rs`), so clangd and ty re-check unsaved text by themselves; the gaps:

  - Pull diagnostics: advertise `textDocument.diagnostic` and, for a server with `diagnosticProvider`, ask `textDocument/diagnostic` (with `previousResultId`; an `unchanged` report keeps the marks) after a short rest in typing (~300 ms, one timer like the lightbulb's; older requests cancelled through `App::send_request`), on open, and on `workspace/diagnostic/refresh` (answered, then every open document of that server re-asked). `relatedDocuments` and `interFileDependencies` re-ask the other open files of that server. Pull results go into the same store as published ones (`Marks`, Problems, the raw JSON for code actions); a server that does both keeps whichever came for the newer version
  - Never block typing: requests and answers are async as now; a burst of edits sends one request after the rest, not one per key (checked at 61k selections: the timer, not the edit, does the work)
  - Tests: the fake server gains `diagnosticProvider` and scripted pull reports (`full` / `unchanged`, a `resultId`), and `workspace/diagnostic/refresh`; typing then resting asks once with the current version and draws the answer; an `unchanged` keeps the marks; a stale answer (older version) is dropped
