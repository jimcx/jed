---
id: semantic_tokens
name: LSP semantic tokens
summary: C/C++ (and any server that offers them) get semantic highlighting from the language server on top of tree-sitter: constructors, macros, parameters, fields, readonly, deprecated, and compiled-out `#if` code dimmed
status: in-progress
priority: 0
dependencies: []
---

See [LSP](../PLAN.md#lsp), [Syntax highlighting](../PLAN.md#syntax-highlighting), [Theme](../PLAN.md#theme).

  - Goal: tree-sitter stays the instant, always-available base layer; the server's `textDocument/semanticTokens` (full + full/delta) and clangd's `textDocument/inactiveRegions` are laid over it. Measured against clangd 22.1.8: provider has `full.delta: true`, `range: false`; constructors and destructors arrive as type `class` with modifier `constructorOrDestructor`; compiled-out code arrives as the inactiveRegions notification (opt-in via `textDocument.inactiveRegionsCapabilities.inactiveRegions`), not as tokens
  - Client: advertise `textDocument.semanticTokens` (relative format, no multiline / overlapping tokens) and `workspace.semanticTokens.refreshSupport`; read the `semanticTokensProvider` legend at initialize (legend names can repeat: map by name); `workspace/semanticTokens/refresh` re-requests tokens for open buffers instead of answering Null (`src/lsp/server.rs`)
  - Requests: `Request::SemanticTokens` via `send_doc_request`, own debounce timer (~150 ms after the last edit, at once after didOpen), then `full/delta` with the last `resultId`. A result for an older revision is not dropped: positions are read in the text at the sent version (`Documents::since`, `pos::LspReader`) and mapped through the later change sets, as diagnostics are (no flicker while typing). Decoding runs in `spawn_blocking` on a rope clone, with `pos::LineWalker`
  - Storage: model module `semantic.rs`: sorted `Token { start, end, kind, mods }`, `replace` / `map(&ChangeSet)` (with `ChangeSet::mapper()`) / `overlapping`, and the inactive regions as a second sorted list; both fields on `Buffer`, mapped in `Buffer::apply` next to `diagnostics`; cleared when the server exits or the language changes
  - Drawing: one monotonic-index walker over the visible tokens in `ui/editor.rs`, patched right after the tree-sitter token and before bracket match / selection; a per-frame `(kind, mods)` -> style table from theme keys (`semantic.constructor`, `semantic.macro`, `semantic.parameter`, `semantic.property`, `semantic.enumMember`, `semantic.namespace`, `semantic.readonly`, `semantic.deprecated`, `semantic.inactive`, …, in `Theme::dark_plus`); a type or modifier with no key leaves the tree-sitter colour; inactive regions dim the foreground over everything
  - Settings: `semantic_highlighting` (`on` default / `off`), through `config/settings.rs` + `edit::Settings`
  - Needs: `Manager::supports`, `diagnostics::Marks` mapping as the pattern, `Documents::since`, `LineWalker`, the `editor.link` style work only as a reference
  - Tests: `semantic_tokens_*`: decode of a hand-built relative array (multi-line, UTF-8 and UTF-16), delta application, mapping through edits, a stale result still applies, 60k tokens overlap query within budget, fake-server end-to-end draw, and a real clangd test (skipped when absent) for constructor, macro, `#if 0` dim
  - Done when: in a C++ file open with clangd, constructors, macros, parameters, fields and `const` values are coloured apart from plain functions and variables, a `#if 0` block is dimmed, typing does not flicker colours, `semantic_highlighting = off` returns to pure tree-sitter, and `scripts/check` passes
  - Deferred: range requests for servers that offer them, inlay hints (needs virtual text in the row layout), document highlight, folding ranges, per-token italics / strikethrough beyond what the terminal supports
  - Subtasks (one commit each):
    - ✅ semantic_tokens-a: client capability, legend, requests with debounce and delta, off-thread decode, `semantic.rs` storage mapped through edits; unit tests; docs
      - Built: `src/semantic.rs` (`Semantic` on `Buffer::semantic`: `tokens()`, `overlapping`, `overlapping_inactive`, `replace_inactive`; `Kind::name`, `mods`), `src/app/semantic.rs` (debounce, requests, decode, refresh); `Documents::since` no longer drains old versions (so diagnostics and tokens both read stale versions); fake server `semantic_tokens` option and `test/refresh`. Drawing in b reads `buf.semantic.overlapping(..)`; the `#[allow(dead_code)]`s on the b-only items go when b uses them. inactiveRegions (capability + notification) is still to do
      - Also: FLAKE debugging added to `term_select_mouse_reporting_keeps_every_event` (failed once in a full run, unrelated, passed after)
    - semantic_tokens-b: drawing, theme keys, inactive regions, the setting, end-to-end and real-clangd tests; docs and hand checks
