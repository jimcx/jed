---
description: Review a task's changes yourself (no subagents) against docs/dev/review.md — fix what is blocking or local, record the rest, and run the checks
argument-hint: "[task id or name, branch, or git range; empty = uncommitted changes, else the last commit]"
---

Review one task's work and fix what you find. You do the review yourself: **no subagents**. The target: $ARGUMENTS

The criteria are in [docs/dev/review.md](../../docs/dev/review.md): read it first and apply all of it (its lenses, its dispositions, its failing-tests rule). This file is only the steps for doing that review in a session, on work that isn't going through the review bot: uncommitted changes, commits on main, or a branch before its merge request. On an open merge request the bot does the same review from the same file; don't run both on one head.

## Find the diff

1. Work out what to review, then stop on nothing to review:
   - **Empty:** the uncommitted changes (`git status`, `git diff HEAD --stat`); if the tree is clean, the last commit (`HEAD~1..HEAD`).
   - **A task id or name:** if a worktree for it exists (`git worktree list`, under `.claude/worktrees/`), `EnterWorktree` with its `path`, `git pull --ff-only` if the branch has an upstream (the bot may have pushed fixes), and review `main..HEAD` plus any uncommitted changes there. Else its commits on main (`git log --oneline --grep '(<id>-' --grep '<Task name>'`): the range from the first one's parent to the last.
   - **A branch or git range:** that (`main..<branch>` for a branch).
2. `git diff --stat <range>` for the files. Read narrowly: diff one file or hunk at a time (`git diff <range> -- <file>`, never a whole multi-file diff), ranges of ≤ 150 lines, no `cat` of files over ~300 lines (grep `docs/dev/app.md` / `modules.md` for `jed/src/app/mod.rs` and the like); independent reads and greps in one message. Read every changed hunk: say so in the report if you didn't.

## Review

3. Apply docs/dev/review.md, starting from the task as it says. Write each finding to `<scratchpad>/review.md` as you go: an id, the lens, `path:line`, the problem, the fix, its disposition, and what became of it.

## Fix

4. Fix every `blocking` and `fix` finding, within the criteria's rules for fixes. Don't do a `followup` or a `decision`:
   - a `followup` goes under the task's "Deferred:" in `tasks/<id>.md` (one line: where, what, the fix), so the next task that reads the file finds it;
   - a `decision` goes to the user, with the options and your recommendation.
5. Run `scripts/check` once after the fixes, in the foreground (iterate with `scripts/check test <filter>` / `scripts/check clippy` before that; if you must background it, `scripts/check --bg` + `scripts/check wait`, never a `pgrep -f` loop). If the fixes touch editing, drawing or search hot paths, also the release `milestone` / `search` budget runs from CLAUDE.md (through `tail -40`). A failing test follows the criteria's "Failing tests". Check no test binaries are left running (`pgrep -fl deps/jed-`).
6. Commit only when the reviewed work was already committed (a range, a task's commits or worktree branch): one commit, subject `Review fixes: <task name or short summary>`, then `- ` bullets of what changed. No `Co-Authored-By:` trailer. Uncommitted work stays uncommitted, with the fixes in the tree. Make no commit when nothing was fixed.
   - In a task's worktree with an open MR, re-run `scripts/mr <branch>` (`git branch --show-current`) to update it (its "already exists" error is expected), then `ExitWorktree` with `keep`. The bot reviews the new head.

## Report

7. Short, and what needs the user first: any `decision`, then the task's hand checks. After that, per lens: what you fixed, what you filed as a follow-up, tests that failed in a run but passed alone, whether you read the whole diff, check results, the commit if any, and the full path of `review.md`.
