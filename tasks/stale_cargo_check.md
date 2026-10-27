---
id: stale_cargo_check
name: Stale cargo check diagnostics
summary: "rust-analyzer's own diagnostics are live already; rustc's (`cargo check`, type and borrow errors) only run on save, because cargo reads the files on disk. Keep save as the trigger (VS Code's default), but make it visible: after an edit, the check's marks of that file are drawn dimmed (a theme key, e.g. `editorDiagnostic.stale`) and the status bar says \"cargo check: on save\" until the next check publishes; a `.workspace` example in the docs for `rust-analyzer.diagnostics.experimental.enable` (more live diagnostics from rust-analyzer itself). No auto-save. Tests: rust-analyzer's real check marks dimmed after an edit and back to normal after save (real-server test, skipped when absent)"
status: open
priority: 0
dependencies: []
---

