# buffer

The only place that knows ropey's line model, and the only place text is mutated.

- `pos.rs` — `ByteIdx`, `DisplayCol` and every conversion. `LINE_TYPE = LineType::LF_CR`: LF, CRLF *and lone CR* are line breaks; always pass it to ropey's line APIs (or better, call the helpers here). `line_end` is before the line break; `line_text` excludes it. `display_grapheme` decides how a grapheme is drawn (tab → spaces to the next stop, controls as `^X` width 2, zero-width as `·`); rendering and column maths both use it, so keep them in step. Word movement/`word_range` use three classes (space, word = alphanumeric or `_`, punctuation) as VS Code does.
- `change.rs` — `Change { start, end, text }` and `ChangeSet`: sorted, non-overlapping, **all in pre-edit coordinates** (`debug_assert`ed; empty changes are dropped). `apply` returns the inverse set. `map(pos, Assoc)` moves a pre-edit position through the set; other views use `Assoc::Before` (`After` is test-only so far).
- `undo.rs` — `History` of `Step`s, each a list of `(forward, inverse)` change sets plus `sel_before`/`sel_after`. `record` joins a step when it isn't sealed, the kind matches and isn't `Other`, it's within `GROUP_PAUSE` (1 s) and `sel_before` equals the step's `sel_after`. `undo`/`redo` return the selections to restore and the change sets they applied. `state_id()` (0 = as loaded) is what dirtiness compares against.
- `mod.rs` — `Buffer`:
  - `edit(changes, before, after, kind)` applies, records undo, and pushes onto `applied`; `undo`/`redo` push too. `take_applied()` is drained by `App::sync_views` — every mutation must go through these three methods.
  - Dirty = `history.state_id() != saved_state || deleted`. Saving: `begin_save()` seals undo and returns a `SaveJob` (rope clone is cheap); `SaveJob::run()` blocks (atomic write); `finish_save(state, ok)` records the state that was written, so edits made during the save stay dirty.
  - `read_only: Option<String>` holds the reason (invalid UTF-8); `deleted` means the file is gone and a save recreates it.
  - `Buffers`: `HashMap<BufferId(u64), Buffer>`; ids are never reused. `dirty()` returns sorted ids.
- `file.rs` — `load`/`decode`: a NUL in the first 8000 bytes → `LoadError::Binary`; BOM stripped and remembered; invalid UTF-8 is decoded lossily and marked read-only; a missing file loads empty. The rope keeps the file's bytes exactly (mixed line endings included); `eol` is only what Enter and paste insert. `encode` = BOM + rope bytes. `atomic_write`: temp file `.{name}.red-{pid}.tmp` in the same dir, fsync, keep permissions, rename, fsync dir; writes through symlinks (canonicalizes the target).

Tests here use `Buffer::from_str` (path `/test.txt`) and `tempfile` for the atomic write.
