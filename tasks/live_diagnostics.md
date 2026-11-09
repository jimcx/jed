---
id: live_diagnostics
name: Live diagnostics while editing
summary: pull diagnostics (`textDocument/diagnostic`) after a typing pause, on open and on refresh; pushed and pulled sets kept apart per server
status: done
priority: 0
dependencies: []
---

See [LSP](../PLAN.md#lsp). Squiggles follow the unsaved text as the user types, for every server. Today each edit already goes out as an incremental `didChange` each loop turn and versioned publishes are mapped through later edits (`lsp/diagnostics.rs`), so clangd and ty re-check unsaved text by themselves; the gaps:

  - Pull diagnostics: advertise `textDocument.diagnostic` and, for a server with `diagnosticProvider`, ask `textDocument/diagnostic` (with `previousResultId`; an `unchanged` report keeps the marks) after a short rest in typing (~300 ms, one timer like the lightbulb's; older requests cancelled through `App::send_request`), on open, and on `workspace/diagnostic/refresh` (answered, then every open document of that server re-asked). `relatedDocuments` and `interFileDependencies` re-ask the other open files of that server. Pull results go into the same store as published ones (`Marks`, Problems, the raw JSON for code actions); a server that does both keeps whichever came for the newer version
  - Never block typing: requests and answers are async as now; a burst of edits sends one request after the rest, not one per key (checked at 61k selections: the timer, not the edit, does the work)
  - Tests: the fake server gains `diagnosticProvider` and scripted pull reports (`full` / `unchanged`, a `resultId`), and `workspace/diagnostic/refresh`; typing then resting asks once with the current version and draws the answer; an `unchanged` keeps the marks; a stale answer (older version) is dropped
  - ✅ Done
    - Built: `src/app/diagnostics_pull.rs` (a `Doc` per buffer whose server has `diagnosticProvider`; `check_pull` after every event notes the revision, `Timer::PullDiagnostics` asks 300 ms after the last edit, at once on open; `previousResultId` and the provider's `identifier`; `Trigger::{Edit, Refresh}`: only an edit / open's full answer re-asks the server's other open documents under `interFileDependencies`, so they can't ping-pong; a routine error retries after the rest); `Request::Diagnostics { buffer, version, trigger }` in `app/lsp.rs`; `lsp::diagnostics::{parse_items, parse_pulled, Report, Pulled}`; client capabilities `textDocument.diagnostic` (`relatedDocumentSupport`) and `workspace.diagnostic.refreshSupport`; `workspace/diagnostic/refresh` answered and forwarded as an `Event::Notification`; fake server `pull_diagnostics` / `pull_inter_file` and `test/diag_refresh`
    - Pushed and pulled diagnostics are two sets per server (VS Code's separate collections; the task first said one set, newer version wins): rust-analyzer pushes only its `cargo check` results once the client advertises pull. `lsp::Source::{Push, Pull}`, `ServerId::source(Source)` (the pull key sets a high bit) / `base()`; marks, `Store` and the newest-applied version per (file, key) (`Manager::applied`) use the key; an older version within a set is dropped (`milestone_9_diagnostics_follow_edits_until_republished` now expects that); clearing a server drops both; code actions send the server both sets
    - Also: `check_pull` scans buffers without allocating per event
    - Deferred: workspace diagnostics (`workspace/diagnostic`); a typed `(ServerId, Source)` key instead of the high bit (touches `Marks`, `resolve`, `Store` and every reader); sharing the debounce / per-buffer scan with `app/semantic.rs`; a 61k-selection test (an edit only notes the revision)
    - Still to check by hand: a server with pull diagnostics (ty) — squiggles follow typing ~300 ms after it stops; rust-analyzer shows both its own and `cargo check` diagnostics after a save; clangd unchanged
