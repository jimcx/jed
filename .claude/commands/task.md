---
description: Build a PLAN.md task with subagents — a single task in one commit, or a task's subtasks one commit each; implement, validate and commit in a worktree, then rebase/ff-merge back to main
argument-hint: "[task name or id, e.g. \"Render whitespace\" or soft_wrap]"
---

Build one task from PLAN.md's `## Tasks` list with subagents. The task: $ARGUMENTS

A task is either **single** (one implementer, one validator and its mutation runner, one commit) or **has subtasks** (a short id like `soft_wrap`, "Needs", "Done when" and a `Subtasks (one agent and one commit each):` list of `<id>-a: …` lines). A task with subtasks is built one subtask at a time, in order, each going through the same steps as a single task.

Each task runs in its own git worktree, on its own branch. All the work and commits happen there; main is untouched until the task is done, when the branch is rebased/ff-merged back and the worktree removed.

## Find where we are

1. `git log --oneline -15`, `git status`, and the task list without its bodies: `awk '/^## Tasks/,/^## Stretch/' PLAN.md | grep -n '^- ' | cut -c1-110` (the section is ~50 KB; don't read it whole). Also `git worktree list` to see whether this task already has one. Then read only the chosen task's entry. Tasks have **no order**; ✅ marks finished ones (and finished subtasks).
2. The task: the one named in the arguments (by name or id); else one with some subtasks ✅ but not all (work in progress); else list the open tasks (not ✅) in a few lines, those with subtasks marked, and ask the user which. Stop until they answer.
   - Subtask commits are named `<Task name> (<id>-a): …`; the last one shows the last finished subtask.
3. Interrupted work or resume:
   - If a worktree for this task already exists (`git worktree list`, under `.claude/worktrees/`), it's a resume: `EnterWorktree` with its `path` and continue from there (steps 5–11 as appropriate to what's left).
   - Else, if the tree has uncommitted changes, earlier work was interrupted (pre-worktree). Handle as before: if it's this task's (or its current subtask's) work and looks complete, run the validator and mutation runner (steps 7–8) and commit it in main; if it's partial, have an implementer finish it; if it's something else, or broken beyond repair, stop and tell the user. Don't stash or discard it yourself.

## Spin up the worktree

4. Enter the task's worktree with the `EnterWorktree` tool, passing the task's id (`soft_wrap`) or a short slug of its name (`render-whitespace`) as the name. Reuse the name on a later resume. Note the main checkout's path (it's where `ExitWorktree` returns).
   - `EnterWorktree` creates the worktree under `.claude/worktrees/<name>/` on a branch `worktree-<name>`. It branches from the default branch (remote `main`) unless `worktree.baseRef` is `"head"` — set `worktree.baseRef: "head"` in settings so the worktree carries this repo's unpushed local commits rather than forking from a stale `origin/main`.
   - Confirm the branch sits at main's current tip (`git log --oneline -1` matches main's); if it forked from a stale ref, `git rebase main` before starting.
   - All later `git`, shell, edit and subagent work happens in this worktree (subagents spawned from the isolated session run there too) until the task is done.

## Check the size first (a task without subtasks yet)

5. Skip this for a task that already has a subtask list: follow it. Otherwise check whether the task is one commit's work:
   - Read the task, every PLAN.md section it links, and enough of the code it touches (CLAUDE.md's module map says where) to judge the work.
   - It's too big when it would touch more than ~6 files or 2 subsystems (an agent's cost grows with the square of its turns: two 50-call subtasks cost far less than one 100-call one), touches many unrelated areas, needs a groundwork refactor before the feature, has parts that are each a feature on their own, or would leave fmt, clippy or tests red partway through. Example: soft wrap (the view's one-row-per-line assumption, up / down, mouse hits, scroll).
   - It's not ready when the plan leaves a design decision open that would be costly to reverse, or it needs something the agents can't get (tools to install, credentials, a terminal that isn't on this machine). Stop and report that to the user, with the open question.
   - If it fits, tell the user in a line or two what the task will cover, then go on to step 6.
   - For a task with a subtask list, still read enough of the code its current subtask touches to write the brief below.
   - Write a **brief** to `<scratchpad>/brief.md` (≤ ~60 lines) so the implementer doesn't re-explore: the files and functions to change (`path:line`), existing helpers to reuse, similar code to copy from, the test helpers (from docs/dev/testing.md) and the test file. Use the ranges you've already read for this; don't read more just for it.
   - If it's too big, **stop and report to the user** why, and offer both ways forward, saying which you'd pick:
     - **Subtasks** — better when the parts build on each other towards one feature with a clear "Done when" (e.g. soft wrap). Draft: a short id, "Needs", the feature bullets (from the task and the code you read), "Done when", and 3–5 subtasks, each leaving fmt, clippy and tests green, the last one doing the done-when tests, gaps and hand checks and marking the task ✅.
     - **Separate tasks** — better when the parts are independent: 2–5 smaller single tasks that each leave the checks green.

     If they agree, write it into PLAN.md (the task's entry gains its id, "Needs", "Done when" and `- Subtasks (one agent and one commit each):` list; or the new tasks replace it in the list), move any design detail into the right Features section, and commit it as a plan-only change. Then build the first subtask (or the first new task) if they asked, else stop.
   - If you stop here without building anything (not ready, or the user declines), `ExitWorktree` and `git worktree remove` first.

## Each task or subtask

6. **Implementer** (general-purpose agent with `model: "sonnet"`, run in sequence and wait for it). Its prompt says:
   - Read `CLAUDE.md` at the repo root first, plus the nested CLAUDE.md of any directory it edits. PLAN.md is the source of truth for design.
   - The plan text pasted into the prompt (you, the orchestrator, paste it): the task in full and every PLAN.md section it links. For a subtask: every subtask, which one is its own, and what's already done. It doesn't re-read PLAN.md.
   - Start from the brief at `<scratchpad>/brief.md` (give the path) rather than a fresh tour of the codebase.
   - **Read narrowly** (every tool result stays in its context and is paid for again on each later call): find code with `grep -n` or the LSP tool (definition, references), then read ranges of ≤ 150 lines. Never `cat` a file over ~300 lines or read big ranges of `src/app/mod.rs`; grep the module's entry in `docs/dev/app.md` / `modules.md` instead. Make independent reads and greps in one message.
   - Build only this task (or subtask). Unit tests go in modules. End-to-end tests: a single task's go in `src/todo_tests.rs` (one `mod <task>` block, tests named after the behaviour); a task with subtasks' go in `src/<id>_tests.rs`, named `<id>_*` (create the file with a `//!` doc and register it in `main.rs` if it doesn't exist). Tests go through real code paths (docs/dev/testing.md: driving the app headlessly, then the feature's helpers).
   - Any new Cmd binding also updates `docs/terminals/`; any new setting follows CLAUDE.md's "Settings layer per key"; any new colour goes through `Theme`.
   - `scripts/check` must pass (fmt, clippy, tests, with quiet output). Iterate with `scripts/check test <filter>` (or `scripts/check clippy`) and run the full `scripts/check` once at the end, not after each change. If it touches editing, drawing or search hot paths, also the release `milestone` / `search` budget runs from CLAUDE.md (through `tail -40`).
   - Update the docs for what it adds (module map, conventions, test helpers, background-work patterns), within the budget in CLAUDE.md's "Keeping these files current": at most a line or two in the root CLAUDE.md, the detail in `docs/dev/` or the directory's nested CLAUDE.md.
   - Don't commit, and don't edit PLAN.md's Tasks section.
   - Report concisely: what it built, files changed with the key entry points (`path:line` of the main new functions and tests, for the validator), CLAUDE.md changes, deviations, deferrals, hand checks, and check results.
7. **Validator** (a separate general-purpose agent with `model: "opus"`, started after the implementer finishes: the judgement half). Its prompt includes the implementer's report verbatim and says:
   - Read CLAUDE.md. The task (or subtask) text is pasted into the prompt, as for the implementer; it doesn't re-read PLAN.md. Start from `git status` / `git diff --stat` and the report's entry points; read the changed code and its tests, not a fresh tour of the codebase.
   - Read narrowly, as the implementer does: diff one file or hunk at a time (`git diff <file>`, never a whole multi-file diff), ranges of ≤ 150 lines, no `cat` of files over ~300 lines; independent reads in one message.
   - Judge whether the tests are useful and actually test the code. Look for tautologies, assertions that can't fail, checks against test-only hooks instead of real effects, missing negative cases, and flakiness.
   - Check coverage against every part of the task — for a subtask, its line in the subtask list, and the task's "Done when" on the last subtask.
   - Add or strengthen weak tests and rewrite worthless ones. Fix real bugs minimally and report them.
   - Write a **mutation plan** for the important behaviours, 4–8 mutations, to `<scratchpad>/mutations.md` (the orchestrator's scratchpad path, given in the prompt). Each: an id, the file, the exact text to replace and its replacement (unique in the file, so it applies mechanically), what behaviour it breaks, and the `scripts/check test <filter>` that should fail. Don't run the mutations itself.
   - Run `scripts/check` once (all three checks) after its own changes. Random or property-style tests cap their iterations and output sizes, so a bug fails them quickly instead of eating CPU and RAM.
   - Update the docs if it changes helpers or conventions (CLAUDE.md's budget applies). Don't commit.
   - Report: verdict, test changes, bugs fixed, the mutation plan's path, check results.
8. **Mutation runner** (general-purpose agent with `model: "haiku"`: the mechanical half; started after the validator). Its prompt gives the plan's path and says:
   - Don't edit tests or code except to apply and undo the planned mutations. Before the first, save `git diff | shasum` and copy each file it will mutate to the scratchpad.
   - For each mutation in order: apply it exactly (if the text isn't found, record "not applied" and go on), run its `scripts/check test <filter>`, record caught (a test failed) or survived (all passed; note a compile error as "caught by the compiler"), then restore the file from the copy. After the last, check `git diff | shasum` matches the saved one.
   - Then run `scripts/check` 3 times to catch flakiness (it already caps runs with `scripts/timeout`; never background loops or `alarm` + `exec` wrappers, which leave the test binary running). Check `pgrep -fl deps/jed-` is empty.
   - Report only a table (id, caught / survived / not applied, the failing test) and the three runs' result lines, plus the output of any failing test, and say if any output contains `FLAKE`.

   Then (you, not in the runner's prompt): if mutations survived, or a test related to this task (one it added or changed, or one covering code it touched) was flaky, send the table to the validator (`SendMessage`, so it keeps its context) to strengthen the tests or fix the flake then and there, then have the runner (`SendMessage`) re-run just those mutations and the 3 runs. A second round that still leaves survivors goes to the user. A flaky test unrelated to the task goes through [Flaky tests](#flaky-tests) instead.
9. **Check and commit (you):**
   - Run `scripts/check` once yourself; don't re-read files the agents already reported on. A flaky test elsewhere follows [Flaky tests](#flaky-tests): never ignored, never silently retried until it passes.
   - Make sure the diff includes the doc updates (CLAUDE.md files, `docs/dev/`).
   - Check no test binaries are left running (`pgrep -fl deps/jed-`); kill any strays.
   - Update PLAN.md in the same commit:
     - A subtask that isn't the last: mark its line ✅, with notes on what it changed for later subtasks.
     - A single task, or a task's last subtask: mark the task ✅ with short sub-bullets — what it changed (for later work), "Also:", "Deferred:" and "Still to check by hand:" — then move its full entry (subtask list included) to the end of `docs/plan-done.md`'s Tasks list (`](#` links rewritten as `](../PLAN.md#`), leaving one line in PLAN.md: `- ✅ **<name>** (<id>, if it has one): <its one-line summary, if it has one> — [details](docs/plan-done.md)`.
     - Fix other tasks or plan sections if this work changed them.
   - Commit everything. Subject: a single task's is a short imperative naming the feature (e.g. `Add render whitespace`); a subtask's is `<Task name> (<id>-a): <summary>`. Then a blank line and `- ` bullets of behaviour and notable internals. End with the Co-Authored-By trailer.
10. Give the user a short update after each commit: what landed, anything deferred, the hand checks. For a task with subtasks, move on to the next subtask (still in the worktree). When the task is done, go to step 11.

## Merge back to main

11. When the whole task is done (a single task, or its last subtask is committed):
    - Leave the worktree with the `ExitWorktree` tool (returns to the main checkout).
    - Bring the branch in: prefer a fast-forward, `git merge --ff-only worktree-<name>`, which keeps history linear. If main has moved since the branch forked, `git rebase main worktree-<name>` first, then `git merge --ff-only worktree-<name>`. (Pushes are denied and main is otherwise untouched during a task, so the ff-only path is the norm.)
    - Remove the worktree (`git worktree remove .claude/worktrees/<name>`) and delete the merged branch (`git branch -d worktree-<name>`).
    - Confirm `git status` is clean and `git log --oneline -5` shows the task's commits on main. Then stop: don't start another task unless asked.

## Flaky tests

A flaky test related to the task (one it added or changed, or one covering code it touched) is always fixed then and there by the validator (step 7). The rest of this section is for a test unrelated to the task that fails and then passes on a re-run with no code change (in any agent's run or your own):

- **First time (no `FLAKE` in its output):** don't try to fix it. Have the agent that saw it (or do it yourself) add debugging to that test, tagged with the keyword `FLAKE` so it's easy to grep: a `// FLAKE: <date>, <how it failed>` comment above the test, and assertion messages / an `eprintln!("FLAKE <test name>: …")` on the failing path that dump the state needed to diagnose it next time (the values compared, timings, events seen, the settle condition's inputs). The debugging must not change what the test checks or add sleeps or retries. Confirm `scripts/check test <its name>` passes, then move on with the task. The debugging goes in the task's commit, with a bullet saying which test flaked and how; tell the user in the post-commit update.
- **Seen again (a failure whose output contains `FLAKE`):** stop and report to the user: the test, the `FLAKE` output, and what it suggests is wrong. Don't fix it until they confirm. Their answer decides whether the fix goes in this task's commit, its own commit, or waits.
- Tell every agent's prompt (implementer, validator, runner) this rule in one line: a flaky test unrelated to the task gets `FLAKE` debugging and no fix (one related to it is fixed), and any `FLAKE` output it sees is reported back to you, not acted on.

## Stop and ask the user when

- The task is too big or not ready (step 5).
- A subtask needs something the agents can't get, such as tools to install or credentials.
- A design decision isn't settled by PLAN.md and would be costly to reverse.
- Checks still fail after an agent says it's done, and a second attempt doesn't fix them.
- A test fails with `FLAKE` in its output (see [Flaky tests](#flaky-tests)).
