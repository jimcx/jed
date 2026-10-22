---
description: Build a PLAN.md task with subagents — a single task in one commit, or a task's subtasks one commit each; implement, validate and commit
argument-hint: "[task name or id, e.g. \"Render whitespace\" or soft_wrap]"
---

Build one task from PLAN.md's `## Tasks` list with subagents. The task: $ARGUMENTS

A task is either **single** (one implementer, one validator and its mutation runner, one commit) or **has subtasks** (a short id like `soft_wrap`, "Needs", "Done when" and a `Subtasks (one agent and one commit each):` list of `<id>-a: …` lines). A task with subtasks is built one subtask at a time, in order, each going through the same steps as a single task.

## Find where we are

1. `git log --oneline -15`, `git status`, and the task list without its bodies: `awk '/^## Tasks/,/^## Stretch/' PLAN.md | grep -n '^- ' | cut -c1-110` (the section is ~50 KB; don't read it whole). Then read only the chosen task's entry. Tasks have **no order**; ✅ marks finished ones (and finished subtasks).
2. The task: the one named in the arguments (by name or id); else one with some subtasks ✅ but not all (work in progress); else list the open tasks (not ✅) in a few lines, those with subtasks marked, and ask the user which. Stop until they answer.
   - Subtask commits are named `<Task name> (<id>-a): …`; the last one shows the last finished subtask.
3. If the tree has uncommitted changes, earlier work was interrupted:
   - If it's this task's (or its current subtask's) work and looks complete, run the validator and mutation runner (steps 6–7) and commit.
   - If it's partial, have an implementer finish it (tell it what's already there).
   - If it's something else, or broken beyond repair, stop and tell the user. Don't stash or discard it yourself.

## Check the size first (a task without subtasks yet)

4. Skip this for a task that already has a subtask list: follow it. Otherwise check whether the task is one commit's work:
   - Read the task, every PLAN.md section it links, and enough of the code it touches (CLAUDE.md's module map says where) to judge the work.
   - It's too big when it would touch more than ~6 files or 2 subsystems (an agent's cost grows with the square of its turns: two 50-call subtasks cost far less than one 100-call one), touches many unrelated areas, needs a groundwork refactor before the feature, has parts that are each a feature on their own, or would leave fmt, clippy or tests red partway through. Example: soft wrap (the view's one-row-per-line assumption, up / down, mouse hits, scroll).
   - It's not ready when the plan leaves a design decision open that would be costly to reverse, or it needs something the agents can't get (tools to install, credentials, a terminal that isn't on this machine). Stop and report that to the user, with the open question.
   - If it fits, tell the user in a line or two what the task will cover, then go on to step 5.
   - For a task with a subtask list, still read enough of the code its current subtask touches to write the brief below.
   - Write a **brief** to `<scratchpad>/brief.md` (≤ ~60 lines) so the implementer doesn't re-explore: the files and functions to change (`path:line`), existing helpers to reuse, similar code to copy from, the test helpers (from docs/dev/testing.md) and the test file. Use the ranges you've already read for this; don't read more just for it.
   - If it's too big, **stop and report to the user** why, and offer both ways forward, saying which you'd pick:
     - **Subtasks** — better when the parts build on each other towards one feature with a clear "Done when" (e.g. soft wrap). Draft: a short id, "Needs", the feature bullets (from the task and the code you read), "Done when", and 3–5 subtasks, each leaving fmt, clippy and tests green, the last one doing the done-when tests, gaps and hand checks and marking the task ✅.
     - **Separate tasks** — better when the parts are independent: 2–5 smaller single tasks that each leave the checks green.

     If they agree, write it into PLAN.md (the task's entry gains its id, "Needs", "Done when" and `- Subtasks (one agent and one commit each):` list; or the new tasks replace it in the list), move any design detail into the right Features section, and commit it as a plan-only change. Then build the first subtask (or the first new task) if they asked, else stop.

## Each task or subtask

5. **Implementer** (general-purpose agent on the session's model (Opus), run in sequence and wait for it). Its prompt says:
   - Read `CLAUDE.md` at the repo root first, plus the nested CLAUDE.md of any directory it edits. PLAN.md is the source of truth for design.
   - The plan text pasted into the prompt (you, the orchestrator, paste it): the task in full and every PLAN.md section it links. For a subtask: every subtask, which one is its own, and what's already done. It doesn't re-read PLAN.md.
   - Start from the brief at `<scratchpad>/brief.md` (give the path) rather than a fresh tour of the codebase.
   - **Read narrowly** (every tool result stays in its context and is paid for again on each later call): find code with `grep -n` or the LSP tool (definition, references), then read ranges of ≤ 150 lines. Never `cat` a file over ~300 lines or read big ranges of `src/app/mod.rs`; grep the module's entry in `docs/dev/app.md` / `modules.md` instead. Make independent reads and greps in one message.
   - Build only this task (or subtask). Unit tests go in modules. End-to-end tests: a single task's go in `src/todo_tests.rs` (one `mod <task>` block, tests named after the behaviour); a task with subtasks' go in `src/<id>_tests.rs`, named `<id>_*` (create the file with a `//!` doc and register it in `main.rs` if it doesn't exist). Tests go through real code paths (see CLAUDE.md's Tests section for driving the app headlessly, and docs/dev/testing.md for the feature's helpers).
   - Any new Cmd binding also updates `docs/terminals/`; any new setting follows CLAUDE.md's "Settings layer per key"; any new colour goes through `Theme`.
   - `scripts/check` must pass (fmt, clippy, tests, with quiet output). Iterate with `scripts/check test <filter>` (or `scripts/check clippy`) and run the full `scripts/check` once at the end, not after each change. If it touches editing, drawing or search hot paths, also the release `milestone` / `search` budget runs from CLAUDE.md (through `tail -40`).
   - Update the docs for what it adds (module map, conventions, test helpers, background-work patterns), within the budget in CLAUDE.md's "Keeping these files current": at most a line or two in the root CLAUDE.md, the detail in `docs/dev/` or the directory's nested CLAUDE.md.
   - Don't commit, and don't edit PLAN.md's Tasks section.
   - Report concisely: what it built, files changed with the key entry points (`path:line` of the main new functions and tests, for the validator), CLAUDE.md changes, deviations, deferrals, hand checks, and check results.
6. **Validator** (a separate general-purpose agent with `model: "opus"`, started after the implementer finishes: the judgement half). Its prompt includes the implementer's report verbatim and says:
   - Read CLAUDE.md. The task (or subtask) text is pasted into the prompt, as for the implementer; it doesn't re-read PLAN.md. Start from `git status` / `git diff --stat` and the report's entry points; read the changed code and its tests, not a fresh tour of the codebase.
   - Read narrowly, as the implementer does: diff one file or hunk at a time (`git diff <file>`, never a whole multi-file diff), ranges of ≤ 150 lines, no `cat` of files over ~300 lines; independent reads in one message.
   - Judge whether the tests are useful and actually test the code. Look for tautologies, assertions that can't fail, checks against test-only hooks instead of real effects, missing negative cases, and flakiness.
   - Check coverage against every part of the task — for a subtask, its line in the subtask list, and the task's "Done when" on the last subtask.
   - Add or strengthen weak tests and rewrite worthless ones. Fix real bugs minimally and report them.
   - Write a **mutation plan** for the important behaviours, 4–8 mutations, to `<scratchpad>/mutations.md` (the orchestrator's scratchpad path, given in the prompt). Each: an id, the file, the exact text to replace and its replacement (unique in the file, so it applies mechanically), what behaviour it breaks, and the `scripts/check test <filter>` that should fail. Don't run the mutations itself.
   - Run `scripts/check` once (all three checks) after its own changes. Random or property-style tests cap their iterations and output sizes, so a bug fails them quickly instead of eating CPU and RAM.
   - Update the docs if it changes helpers or conventions (CLAUDE.md's budget applies). Don't commit.
   - Report: verdict, test changes, bugs fixed, the mutation plan's path, check results.
7. **Mutation runner** (general-purpose agent with `model: "sonnet"`: the mechanical half; started after the validator). Its prompt gives the plan's path and says:
   - Don't edit tests or code except to apply and undo the planned mutations. Before the first, save `git diff | shasum` and copy each file it will mutate to the scratchpad.
   - For each mutation in order: apply it exactly (if the text isn't found, record "not applied" and go on), run its `scripts/check test <filter>`, record caught (a test failed) or survived (all passed; note a compile error as "caught by the compiler"), then restore the file from the copy. After the last, check `git diff | shasum` matches the saved one.
   - Then run `scripts/check` 3 times to catch flakiness (it already caps runs with `scripts/timeout`; never background loops or `alarm` + `exec` wrappers, which leave the test binary running). Check `pgrep -fl deps/jed-` is empty.
   - Report only a table (id, caught / survived / not applied, the failing test) and the three runs' result lines, plus the output of any failing test.

   Then (you, not in the runner's prompt): if mutations survived or a run was flaky, send the table to the validator (`SendMessage`, so it keeps its context) to strengthen the tests or fix the flake, then have the runner (`SendMessage`) re-run just those mutations and the 3 runs. A second round that still leaves survivors goes to the user.
8. **Check and commit (you):**
   - Run `scripts/check` once yourself; don't re-read files the agents already reported on. A flaky test elsewhere is fixed, not ignored.
   - Make sure the diff includes the doc updates (CLAUDE.md files, `docs/dev/`).
   - Check no test binaries are left running (`pgrep -fl deps/jed-`); kill any strays.
   - Update PLAN.md in the same commit:
     - A subtask that isn't the last: mark its line ✅, with notes on what it changed for later subtasks.
     - A single task, or a task's last subtask: mark the task ✅ with short sub-bullets — what it changed (for later work), "Also:", "Deferred:" and "Still to check by hand:" — then move its full entry (subtask list included) to the end of `docs/plan-done.md`'s Tasks list (`](#` links rewritten as `](../PLAN.md#`), leaving one line in PLAN.md: `- ✅ **<name>** (<id>, if it has one): <its one-line summary, if it has one> — [details](docs/plan-done.md)`.
     - Fix other tasks or plan sections if this work changed them.
   - Commit everything. Subject: a single task's is a short imperative naming the feature (e.g. `Add render whitespace`); a subtask's is `<Task name> (<id>-a): <summary>`. Then a blank line and `- ` bullets of behaviour and notable internals. End with the Co-Authored-By trailer.
9. Give the user a short update after each commit: what landed, anything deferred, the hand checks. For a task with subtasks, move on to the next subtask. When the task is done, stop: don't start another unless asked.

## Stop and ask the user when

- The task is too big or not ready (step 4).
- A subtask needs something the agents can't get, such as tools to install or credentials.
- A design decision isn't settled by PLAN.md and would be costly to reverse.
- Checks still fail after an agent says it's done, and a second attempt doesn't fix them.
