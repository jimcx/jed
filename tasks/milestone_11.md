---
id: milestone_11
name: File history
summary: snapshot on save, history list, compare (diff view), restore
status: done
priority: 0
dependencies: []
---

    - Needs: the atomic save path (M1), the palette (M5), the diff view (M10), explorer rename (M2)
    - The diff view's sides are `diff_view::Text` variants: add one for a history entry (`Pair` of entry ↔ `Text::Buffer`)
    - `history` module: `.jed/history/` with its `.gitignore`, snapshot after the save's rename, skip duplicates, `[history]` limits
    - Ignore watcher events under `.jed/` (the explorer and quick open already hide it, M5; search skips it, M7)
    - "File History: Show" in the palette, explorer and tab bar; compare; restore as one undo step
    - Done when: a history write failure never fails a save; restore and then undo gives back the exact buffer
    - Tasks (one agent and one commit each):
      - ✅ 11a: `history` module and recording: `.jed/history/` storage (per-file folder by path hash, `entries.toml`, snapshot files, `.jed/.gitignore`), snapshot off-thread after each successful save's rename, skip duplicates, `[history]` limits (`enabled`, `max_entries`, `max_file_size`), failures logged + status message (never fail the save); watcher events under `.jed/` ignored; explorer rename moves a file's history (delete keeps it)
        - `history.rs` (blocking, one process-wide lock): folder = 32 hex of SHA-256 of the `/`-separated relative path; `entries.toml` (path + `[[entry]]` id, time ms, size, hash; oldest first, temp + rename); `relative(root, path)` (`None` outside the root / under `.jed/`), `record` → Added / Duplicate / TooLarge / Disabled, `entries(root, rel)` newest first and `read(root, rel, id)` for 11b, `rename` (file or folder; merges by time into an existing history)
        - `app/history.rs`: `SaveJob::write` returns the written bytes; after `SaveDone` a second blocking task records, `AppEvent::HistoryDone` (warning "File history of <rel> not saved: …" on error); explorer rename moves history in its own blocking task (only when the rename succeeded); `App::fs_changed` drops paths under `.jed/`
        - Not done: two jed processes writing one history at once; delete-keeps-history has no test (explorer delete uses the real Trash)
      - ✅ 11b: "File History: Show" for the active file: a palette picker of entries (newest first, "3 min ago · 12 KB"), Compare (a `diff_view::Text` variant for an entry, `Pair` entry ↔ `Text::Buffer`) and Restore (buffer text replaced as one undoable edit, not saved)
        - `fileHistory.show` / `fileHistory.restore` (palette, unbound): entries read off-thread, then the palette opens on them (`Picker::History { restore }`; no history → a message, never an empty picker); Enter = Compare (a diff tab `name (History: 3 min ago)`, entry ↔ yours) or, for Restore, restores; Restore on a history diff tab restores its entry
        - `diff_view::Text::History(history::Snapshot)` (Copy: time + 8 hash bytes, read back with `history::read_snapshot`); restore decodes like a load, refuses binary / non-UTF-8 / read-only, diffs like `disk::changes` (re-diffed if the buffer changed meanwhile) and applies one undo step via `apply_changes`; keeps the buffer's BOM / EOL setting
        - Not done: absolute times are UTC (no time-zone database); no key inside the picker to restore instead of compare
      - ✅ 11c: entry points from the explorer (on a file) and the tab bar; the done-when tests (a history write failure never fails a save; restore then undo gives the exact buffer); marks M11 ✅
        - `explorer.showFileHistory` (palette, unbound; a folder gets a message) and right-click on an explorer row; right-click on a tab activates it and runs File History: Show for its file (also a diff tab's file). jed has no context menus: right-click runs the command directly
        - Files not open: Compare reads the disk (`Text::Buffer` falls back to the file, switching to the buffer once it opens); Restore opens the file first (`OpenRequest.restore`), nothing written
        - Done-when tests: `milestone_11_done_when_a_history_write_failure_never_fails_a_save`, `milestone_11_done_when_restore_then_undo_gives_back_the_exact_buffer` (text, selections, primary, BOM, dirty state)
    - Deferred: absolute times in UTC; no key in the picker to restore instead of compare; no context menus (right-click runs the command); two jed processes writing one history at once; delete keeping history is untested (explorer delete uses the real Trash)
    - Still to check by hand: right-click on explorer rows and tabs in real terminals (kitty, iTerm2, tmux may take right-click); Compare / Restore of a file not open from the explorer; restore then undo on a real file with multiple cursors; a broken `.jed/history` (e.g. read-only) still lets saves through with the warning; `git status` never shows `.jed/`; the `·` in the picker rows
