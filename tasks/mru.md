---
id: mru
name: Most-recently-used commands and files
summary: the command palette and quick open list the last 20 commands picked from the palette and the last 20 files opened first, most recent first, when the input is empty
status: done
priority: 0
dependencies: []
---

  - `src/mru.rs` — `MAX = 20`, `Mru { commands: Vec<Command>, files: Vec<PathBuf> }` (Default, held on `App::mru`, in memory only) and `record(list, item)` (one generic `T: PartialEq` fn: drops an equal earlier copy, inserts first, truncates to `MAX`)
  - Commands recorded only in `accept_palette`'s `Action::Run(cmd)` arm (`src/app/palette.rs`), so a key-bound command (which runs `execute` directly) is excluded; files recorded once they open successfully (`app/files.rs`' `record_open`, called by `open_file`'s already-open / already-shown-hex arms and by `opened` on a successful load — not on a failed one), relative to the workspace root when inside it — quick open, explorer, command line, search / problem / location results and back / forward all pass through `open_file`
  - Empty-query ordering in `src/palette.rs`: `command_rows_with_recent` lists recent commands in recency order then the rest alphabetically (no duplicates, a stable sort); `file_rows_with_recent` prepends recent-file rows (file name, folder dimmed, `Action::Open`) and drops the walk rows already shown, then truncates to `MAX_FILE_ROWS`; a typed query keeps the fuzzy scoring. `files_matched` re-applies the recent rows as the walk streams in; `update_files_hint` drops "No matching files" while recent rows are shown
  - Tests: `mru::record` (order, dedupe, cap) and the two empty-query orderings in `src/palette.rs`; `todo_tests::mru` end to end (picked commands come back first, opened files come back first, the cap of 20, a key-bound command stays out)
  - Also: hex opens are recorded (their load reports `Loaded::Binary`, which `opened` records); files outside the workspace are stored absolute. A file is normalized at the record site (`workspace::normalize`), so a `..` / symlink / case-differing spelling of the same file stores the walk's spelling and can't appear twice in quick open
  - Deferred: none
  - Still to check by hand: the palette and quick open showing the recent items first in a real terminal (narrow widths), and that nothing persists across a quit / reopen
