---
id: feature_test_files
name: Feature-based end-to-end test files
summary: "the `src/milestone*_tests.rs` files (and `src/todo_tests.rs`) are named after when the work was done, not what they test (`milestone2_tests.rs` = workspace, trust, explorer, tabs, file ops); move them to `src/tests/<feature>.rs` (e.g. `editing`, `multicursor`, `find`, `workspace_search`, `explorer`, `splits`, `palette`, `highlighting`, `lsp`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …), splitting a milestone's file where it spans features and merging tests of one feature from several milestones. Rules for every step:"
status: open
priority: 0
dependencies: []
---

  - Pure move and rename: no test logic changes; the test count stays the same (compare `cargo test -- --list` before / after)
  - Tests renamed from `milestone_N_*` / `<id>_*` to the behaviour; `done_when` tests keep a recognisable name (`done_when_*`), their file's `//!` doc saying which milestone's "Done when" they cover; the release budget tests become `budget_*` (CLAUDE.md's `cargo test --release milestone` commands change to `budget` in the step that renames the last of them)
  - Each step updates the CLAUDE.md references it moves (the Tests section's helper paths, the module map entry, nested CLAUDE.md files) and PLAN.md's references to test names
  - **Test support module**: `src/tests/mod.rs` and `src/tests/support.rs` with the shared helpers (`open_app`, `cmd`, `settle_git`, `draw_sized`, …, now duplicated or reached across files as `milestone10_tests::…`); callers updated, duplicates removed, no test moved yet
  - **Feature test files: editing** — M1, M4 and M6 (`milestone_tests.rs`, `milestone4_tests.rs`, `milestone6_tests.rs`) into `editing`, `clipboard`, `multicursor`, `find`, plus `todo_tests`' modules of those features
  - **Feature test files: workbench** — M2, M3, M5, M7 and M8 into `workspace`, `explorer`, `tabs`, `splits`, `palette`, `workspace_search`, `highlighting`, `settings`, plus `todo_tests`' modules of those features
  - **Feature test files: tools** — M9–M13 and the rest of `todo_tests` into `lsp`, `problems`, `completion`, `git`, `diff_view`, `file_history`, `tasks`, `sessions`, `backups`, `recent`, …; then CLAUDE.md, `.claude/commands/task.md` say new tests go in the feature's file (a task with subtasks too, instead of `src/<id>_tests.rs`)
