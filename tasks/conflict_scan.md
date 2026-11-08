---
id: conflict_scan
name: Cheap merge-conflict scan
summary: ✅ the conflict-marker scan searches the rope's chunks with memchr instead of walking every line (1 MB: 1.03 ms → ~40 µs per edit)
status: done
priority: 0
dependencies: [conflicts]
---

Found while profiling the tests (2026-11-02): `conflict::scan` was most of the 1 MB CRLF round trip's frame time (`milestone_1_round_trip_one_mb_crlf`). `Conflicts` (`src/conflict.rs`) caches the scan by revision, but `map` drops the cache on every change set. So the first frame after each keystroke (`ui/editor.rs:411`) walks every line of the buffer: `pos::each_line` slices each line out of the rope (`RopeSlice::slice` + `trailing_line_break_idx`) and `Kind::of` checks its start. The cost is proportional to the line count on every keystroke, in every file, even a file with no markers. Debug builds hide part of the cost now that dependencies build optimised (Cargo.toml), but the per-line slicing stays.

  - Needs: `src/conflict.rs` (`scan`, `Conflicts::scan` / `map`), `buffer::pos` (`each_line`; only `pos` may look at ropey's chunks), the callers: `ui/editor.rs:411`, `edit.rs:369` and `:851`, `app/conflict.rs:22–49`, `app/diff_view.rs:776`
  - First, measure: a release budget test `milestone_conflict_scan_budget_typing_in_a_1mb_file` (name keeps `milestone`, CLAUDE.md's budget rule). One 1 MB file with no markers and one with a conflict every ~100 KB. Type 50 characters, timing `Conflicts::scan` after each edit, and separately the frame as M1 does. Report the numbers before the fix.
  - Change, cheapest first; stop when the budget is met:
    1. Find marker lines without visiting every line: a `pos` helper (e.g. `pos::lines_starting_with(rope, &[&str]) -> impl Iterator<Item = usize>`, or `ByteIdx` line starts) that searches the rope's chunks for `\n` followed by `<`, `|`, `=` or `>` (`memchr` / `memchr3` are already in the tree via `regex`; check `Cargo.lock` before adding it as a direct dependency) and checks the full 7-char prefix only there. It must handle a marker or its preceding `\n` split across a chunk boundary, the first line (no `\n` before it), and CR / CRLF / the other line breaks `LINE_TYPE` counts (a lone `\r` line break must still start a line here as in `each_line`). `scan` keeps its pairing logic and `line_bounds` calls for the few matches.
    2. If that isn't enough: an incremental rescan. `map` keeps the marker list, maps it through the change set (`ChangeSet::mapper()`), and marks only the lines the change touched (plus one line either side, since a join or split can make or break a marker) for rescanning. `scan` then rescans those and redoes the pairing over the markers, which are few.
  - Done when:
    - the budget test's per-edit scan of the marker-free 1 MB file is ≤ 0.1 ms in release (or 20× under today's, whichever is easier to meet; report both), and M1's frame budget still passes
    - `conflict.rs`'s unit tests and `conflicts_tests` pass unchanged; new unit tests in `pos` for the helper: markers at the text's start and end, a marker split across a chunk boundary (a rope built from chunks, as `pos`'s other chunk-boundary tests do), CRLF and lone-`\r` files, a `<<<<<<<` not at a line start (not found), and a random-text check that the helper finds the same lines as today's `each_line` + `Kind::of` walk
    - `scripts/check` and `cargo test --release milestone -- --nocapture --test-threads=1` are green
  - ✅ Done (step 1 only; the incremental rescan wasn't needed):
    - `pos::lines_starting_with(rope, prefixes) -> Vec<(ByteIdx, usize)>`: line starts beginning with a prefix (and which), found by `memchr` over each chunk for the prefixes' first bytes, a line-start check (text start, or LF / CR before; the previous chunk's last byte carries over) and the full prefix only at hits; a prefix split across chunks through a small reused window, built only when a chunk's tail holds a first byte. `conflict::scan` builds its markers from it (`line_of` + `line_bounds` per match) and keeps its pairing; `Kind::of` is gone.
    - Budget (`todo_tests::conflict_scan::milestone_conflict_scan_budget_typing_in_a_1mb_file`, release): per-edit scan of a marker-free 1 MB file 1.03 ms → 38 µs (asserted < 100 µs), with a conflict every 100 KB 1.02 ms → 47 µs (< 200 µs); frames ~270 µs. M1's frame budget passes (472 µs avg).
    - Also: `pos` unit tests for edges, chunk splits, CRLF / lone CR, not-at-line-start, and two random checks against the old `each_line` walk (one on randomly chunked ropes, asserting multi-chunk ropes occur).
    - Deferred: `Conflicts::map` still drops the cache on every edit (the incremental rescan, step 2) — only worth it if the scan shows up again.
    - Still to check by hand: none (covered by the budget and the existing `conflicts_tests`).
