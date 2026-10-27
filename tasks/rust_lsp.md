---
id: rust_lsp
name: Rust language server
summary: `rust-analyzer` as a built-in server for Rust
status: done
priority: 0
dependencies: []
---

  - Root: the workspace root (rust-analyzer finds `Cargo.toml` / the cargo workspace under it itself); no `initializationOptions` by default (`check.command` stays rust-analyzer's `check`)
  - It's slow to start on big projects (indexing, `cargo check`): `$/progress` / `window/workDoneProgress` notifications are answered, and ideally shown (e.g. "rust-analyzer: indexing 40%" in the status bar); requests before it's ready may return nothing or `ContentModified` (-32801), which must be dropped quietly, not reported as errors
  - Diagnostics come from `cargo check` on save (pull vs push: rust-analyzer pushes `publishDiagnostics`; check it doesn't need `textDocument/diagnostic` pull support)
  - Tests: a fake-server test for the built-in being picked; a real-server test (like ty's / clangd's) on a tiny temp crate, skipped with a note when `rust-analyzer` isn't runnable. Here it's only the rustup proxy: `rustup component add rust-analyzer` for the pinned 1.99 toolchain would install it (ask first: it changes the toolchain setup); on macOS the same, or Homebrew's
  - Built-in `rust-analyzer` (`lsp::config` `Builtins`, no args or `initializationOptions`, the workspace root; `.workspace` `[lsp.rust]` wins only when trusted); `lsp/progress.rs` (per-server work-done progress by token — JSON values, `1` ≠ `"1"` — fed by `$/progress`, cleared when the server fails or crashes); the status bar shows the first running server's latest unfinished work after the problem counts ("rust-analyzer: Indexing 40% 3/25 (core)", any server); answers with -32800 / -32801 / -32802 (`RpcError::is_routine`) dropped without a message (a pending completion cleared); a server dying during startup quotes its last stderr line ("Unknown binary 'rust-analyzer'"); diagnostics are pushed (no pull support needed); the fake's `test/progress` and `fail(method, code, message)`; `REAL_SERVERS` / `children_named` shared from milestone9_tests
  - `rustup component add rust-analyzer` installed for 1.99 on the Linux box; `real_rust_analyzer_on_a_cargo_crate` runs hover, definition, references, completion, `cargo check` errors on save, rename and format on a temp crate (its own `cargo.targetDir` via the test's config), retrying requests while it loads
  - Deferred: nothing of the item
  - Still to check by hand: a large cargo project (indexing progress in the status bar, `cargo check` diagnostics after a save); the error message without the component installed; macOS (rustup or Homebrew's rust-analyzer)
