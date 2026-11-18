# buffer

The only place that knows ropey's line model, and the only place text is mutated. Summary only: [docs/dev/buffer.md](../../../docs/dev/buffer.md) has every function and rule — read the entry for the file you touch, and add detail there, not here.

- `pos.rs` — `ByteIdx`, `DisplayCol` and every conversion (lines, columns, graphemes, words, LSP positions, tree-sitter points, chunk readers)
- `change.rs` — `ChangeSet`: sorted, non-overlapping changes in pre-edit coordinates; `apply` returns the inverse; `mapper()` maps positions
- `undo.rs` — `History` of `Step`s, the joining rule, `state_id()` (what dirtiness compares against)
- `mod.rs` — `Buffer` (`edit` / `undo` / `redo`, saving, the file on disk, indentation, read-only / log / commit / untitled buffers) and `Buffers`
- `file.rs` — `Stamp`, `load` / `decode` (binary sniff, BOM, lossy UTF-8 → read-only), `encode`, `atomic_write`

Rules:

- Line breaks are `LINE_TYPE` (LF, CRLF and lone CR): pass it to ropey's line APIs, or better call the `pos` helpers. `line_end` is before the break.
- `display_grapheme` is what both drawing and column maths use; keep them in step.
- Don't use ropey's `Lines::prev()` (2 beta repeats a line at a chunk edge): `each_line` steps backward by index.
- Many positions in order: `LineWalker` / `Line` (columns), `Peek` (chars), `LspWalker` / `LspReader` (LSP), `ChangeSet::mapper()` (mapping) — not the one-off functions in a loop.
- Build change sets with `with_capacity` + `push(start, end, &str)`; `Change` + `ChangeSet::new` is the owned form for tests. `map` is O(changes): tests only.
- Every mutation goes through `Buffer::edit` / `undo` / `redo` (or `append` for logs), which apply through the private `apply`: it feeds syntax, diagnostics, quick diff, blame, conflicts, `applied` (drained by `App::sync_views`) and the servers' tracked copies.
- Saving is `begin_save()` → `SaveJob::write()` (blocking) → `finish_save(state, written)`; `has_file()` is false for log, commit, version (`Buffer::version`: an earlier file version, read-only, language from its source file) and untitled buffers.
- Tests use `Buffer::from_str` (path `/test.txt`) and `tempfile` for the atomic write.
