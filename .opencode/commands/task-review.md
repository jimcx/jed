---
description: Review a task's changes yourself (no subagents) — tests and coverage, performance, code re-use, language effectiveness and hygiene — fix what is local, and run the checks
agent: task
---

Review one task's work and fix what is local. You do the review yourself: **no subagents**. The target: $ARGUMENTS

This is the review half of `/task` on its own: the criteria of its validator and reviewer and the checks of its commit step, applied to work that already exists. review-bot reads the Claude Code twin, `.claude/commands/task-review.md`, as its criteria for task MRs, so keep the Validator, Reviewer, Hygiene and Failing tests sections the same in both; a change to the steps belongs in both too.

## Harness

- Don't call the `subagent` tool: you do every step yourself.
- `CLAUDE.md` is loaded as project instructions by `opencode.jsonc`. Nested `CLAUDE.md` files are not auto-loaded; read the nested one for each directory you edit.

## Find the diff

1. Work out what to review, then stop on nothing to review:
   - **Empty:** the uncommitted changes (`git status`, `git diff HEAD --stat`); if the tree is clean, the last commit (`HEAD~1..HEAD`).
   - **A task id or name:** if a worktree for it exists (`execute` → `tools.opencode.worktree_list()`), move into it with `tools.opencode.session_move({ directory })` (note the main checkout's path first) and review `main..HEAD` plus any uncommitted changes there. Else its commits on main (`git log --oneline --grep '(<id>-' --grep '<Task name>'`): the range from the first one's parent to the last.
   - **A branch or git range:** that (`main..<branch>` for a branch).
2. Find the task the change builds from the `tasks/` file(s) the diff touches, the commit subjects, or the argument, and read it (`scripts/tasks show <id>`): its text, its "Done when", and for a subtask its line in the subtask list. Don't re-read PLAN.md sections beyond what a finding needs.
3. `git diff --stat <range>` for the files. Skip generated files and PLAN.md's index.
   - Review the diff, not the code around it: look further only to grep for an existing helper, to see how hot a path is, or to see what a test exercises.
   - Read narrowly: diff one file or hunk at a time (`git diff <range> -- <file>`, never a whole multi-file diff), ranges of ≤ 150 lines, no `cat` of files over ~300 lines (grep `docs/dev/app.md` / `modules.md` for `src/app/mod.rs` and the like); independent reads and greps in one message.

## Review

4. Apply the three sets of criteria below. Findings are concrete: each has a location (`path:line`), the problem and the fix. A lens with no findings is a fine result; don't pad, and don't report what rustfmt or clippy already catch or matters of taste. Write them to `/tmp/opencode/jed-task/<task id or slug>/review.md` (create it with `mkdir -p`) as you go: an id, the lens, `path:line`, the problem, the fix, and its status (fixed, or left open and why).

### Validator (/task step 7)

- Are the tests useful, and do they actually test the code? Look for tautologies, assertions that can't fail, checks against test-only hooks instead of real effects, missing negative cases, and flakiness. Random or property-style tests cap their iterations and output sizes.
- Coverage against every part of the task — for a subtask, its line in the subtask list, and the task's "Done when" on the last subtask. End-to-end tests go through real code paths (`docs/dev/testing.md`) and sit where CLAUDE.md's "Tests" says (`src/todo_tests.rs` for a single task, `src/<id>_tests.rs` for a task with subtasks).
- Real bugs in the changed code.

### Reviewer (/task step 8)

- **Performance:** work per edit, per frame or per selection against CLAUDE.md's "Many selections" and "Never block the UI thread" rules; rope lookups or allocations in a loop, needless `clone` / `collect` / `to_string`, quadratic scans, work repeated that could be cached or moved out of a loop. Say why the path is hot: on a cold path, the clearer code wins.
- **Code re-use:** new code that duplicates an existing helper (grep for the obvious names; `docs/dev/modules.md`, `buffer::pos`, the helpers in `docs/dev/testing.md`), and logic or test setup repeated within the diff. Extract a helper only for callers that exist now; no abstractions for later.
- **Language effectiveness:** Rust used to its strengths: enums and newtypes over flag `bool`s, strings and raw `usize`s, so bad states can't be represented; exhaustive `match` over a `_` arm that would hide a new variant; borrowing over cloning; `?` and let-chains over nesting; iterator adapters and std APIs over hand-rolled loops, where they read better; no `unwrap` that real input can reach; visibility no wider than needed.

### Hygiene (/task step 9)

- The docs are updated for what the change adds (module map, conventions, test helpers, background-work patterns), within CLAUDE.md's budget in "Keeping these files current".
- Any new Cmd binding also updates `docs/terminals/`; any new setting follows the settings convention; any new colour goes through `Theme`.
- The task file is updated: a finished subtask's line marked ✅ with notes for later subtasks; a finished task marked ✅ with "Also:", "Deferred:" and "Still to check by hand:", `status` (`in-progress` after the first subtask, `done` after the last) and `summary` set, and PLAN.md's index regenerated (`scripts/tasks check`).
- No private hosts: the diff, task files and commit messages contain none of the user's real server hostnames, IPs, usernames or private repo paths (CLAUDE.md "No private hosts"); `git.example.com` / `alice/project` instead.
- Commit messages follow CLAUDE.md's "Commits": a single task's subject is a short imperative naming the feature; a subtask's is `<Task name> (<id>-a): <summary>`; a blank line, then `- ` bullets; no `Co-Authored-By:` trailer.

## Fix

5. Fix what is local and keeps behaviour: weak or worthless tests get strengthened or rewritten, real bugs a minimal fix and a test, missing docs or task-file notes added. Leave open, without doing it, anything that would change behaviour, reach well outside the diff, or be costly to reverse. Don't weaken or delete tests; change them only to follow a rename or signature change. Never write a private host while fixing.
6. Run `scripts/check` once after the fixes, in the foreground (iterate with `scripts/check test <filter>` / `scripts/check clippy` before that; if you must background it, `scripts/check --bg` + `scripts/check wait`, never a `pgrep -f` loop). If the fixes touch editing, drawing or search hot paths, also the release `milestone` / `search` budget runs from CLAUDE.md (through `tail -40`). A failing test follows [Failing tests](#failing-tests). Check no test binaries are left running (`pgrep -fl deps/jed-`).
7. Commit only when the reviewed work was already committed (a range, a task's commits or worktree branch): one commit, subject `Review fixes: <task name or short summary>`, then `- ` bullets of what changed. No `Co-Authored-By:` trailer. Uncommitted work stays uncommitted, with the fixes in the tree. Make no commit when nothing was fixed.
   - In a task's worktree with an open MR, re-run `scripts/mr <branch>` (`git branch --show-current`) to update it (its "already exists" error is expected), then `session_move` back to the main checkout, keeping the worktree.

## Report

8. Short, per lens: what you fixed, what you left open and why (and whether it belongs under the task's "Deferred:"), tests that failed in a run but passed alone, check results, the commit if any, and the absolute path of `review.md`. A design decision that's costly to reverse: ask the user rather than deciding.

## Failing tests

When a test fails in any run, re-run it alone with `scripts/check test <its name>`:

- **It passes alone:** note it (the test and how it failed) in the report and move on. It counts as passing: don't re-run the suite, debug it or add debugging to it.
- **It fails alone:** it's a real failure; fix it (or, if you can't edit, report it as a blocking finding).
