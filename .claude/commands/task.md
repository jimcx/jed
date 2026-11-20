---
description: Build a PLAN.md task with subagents — a single task in one commit, or a task's subtasks one commit each; implement, validate and commit in a worktree, then open a merge request in Gitea
argument-hint: "[task name or id, e.g. \"Render whitespace\" or soft_wrap]"
---

Build one task from PLAN.md's `## Tasks` list with subagents. The task: $ARGUMENTS

A task is either **single** (one implementer, one validator and its mutation runner, one commit) or **has subtasks** (a short id like `soft_wrap`, "Needs", "Done when" and a `Subtasks (one agent and one commit each):` list of `<id>-a: …` lines). A task with subtasks is built one subtask at a time, in order, each going through the same steps as a single task, all in one run: the workflow doesn't stop between subtasks, only after the last one.

Each task runs in its own git worktree, on its own branch. All the work and commits happen there; main is untouched; when the task is done the branch is pushed and a Gitea merge request opened (via `scripts/mr`), carrying the evidence the run produced. From there the bots take it ([Hand over for review](#hand-over-for-review)): the review bot reviews it against [docs/dev/review.md](../../docs/dev/review.md) and files follow-ups; this run fixes what it asks for, until it approves; the merge bot merges it. The user sees only what is theirs to decide.

## Find where we are

1. `git log --oneline -15`, `git status`, and the task list without its bodies: `scripts/tasks list` (and `scripts/tasks list --status done` for finished ones); `scripts/tasks check` is the sanity check. Also `git worktree list` to see whether this task already has one. Then read the chosen task's entry with `scripts/tasks show <id>` (and read `tasks/<id>.md`). Tasks have **no order**; ✅ marks finished ones (and finished subtasks).
   - Also check that the MR can be opened at the end: `command -v tea >/dev/null || [ -n "$GITEA_TOKEN" ]`. If neither holds, ask the user now (before any work) to export `GITEA_TOKEN` (a Gitea access token with repository write access) in the environment this session runs in, or install and log in `tea`, and wait for their answer; the check is repeated before step 11. Never ask for the token's value in chat or write it to a file.
2. The task: the one named in the arguments (by name or id); else one with some subtasks ✅ but not all (work in progress); else list the open tasks (`scripts/tasks list --status open`) in a few lines, those with subtasks marked, and ask the user which. Stop until they answer.
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

     If they agree, edit `tasks/<id>.md` (the task's entry gains its id, "Needs", "Done when" and a `Subtasks (one agent and one commit each):` list; or new `tasks/<id>.md` files replace it), move any design detail into the right Features section of PLAN.md, regenerate the index with `scripts/tasks index --write`, and commit it as a plan-only change. Then build the first subtask (or the first new task) if they asked, else stop.
   - If you stop here without building anything (not ready, or the user declines), `ExitWorktree` and `git worktree remove` first.

## Each task or subtask

6. **Implementer** (general-purpose agent with `model: "sonnet"`, run in sequence and wait for it). Its prompt says:
   - Read `CLAUDE.md` at the repo root first, plus the nested CLAUDE.md of any directory it edits. PLAN.md is the source of truth for design.
   - The plan text pasted into the prompt (you, the orchestrator, paste it): the task in full and every PLAN.md section it links. For a subtask: every subtask, which one is its own, and what's already done. It doesn't re-read PLAN.md.
   - Start from the brief at `<scratchpad>/brief.md` (give the path) rather than a fresh tour of the codebase.
   - **Read narrowly** (every tool result stays in its context and is paid for again on each later call): find code with `grep -n` or the LSP tool (definition, references), then read ranges of ≤ 150 lines. Never `cat` a file over ~300 lines or read big ranges of `jed/src/app/mod.rs`; grep the module's entry in `docs/dev/app.md` / `modules.md` instead. Make independent reads and greps in one message.
   - Build only this task (or subtask). Unit tests go in modules. End-to-end tests: a single task's go in `jed/src/todo_tests.rs` (one `mod <task>` block, tests named after the behaviour); a task with subtasks' go in `jed/src/<id>_tests.rs`, named `<id>_*` (create the file with a `//!` doc and register it in `jed/src/main.rs` if it doesn't exist). Tests go through real code paths (docs/dev/testing.md: driving the app headlessly, then the feature's helpers). A crate's unit tests go at the bottom of its module in `crates/<c>/src/`; jmux's end-to-end tests go in `jmux/src/<id>_tests.rs`, named `<id>_*` (docs/dev/crates.md).
   - Tests run features through their command (`test_execute` / jmux `run`), not key presses (docs/dev/testing.md).
   - Any new Cmd binding also updates `docs/terminals/`; any new setting follows CLAUDE.md's "Settings layer per key"; any new colour goes through `Theme`.
   - `scripts/check` must pass (fmt, clippy, tests, with quiet output). Run it in the foreground; if you must background it, use `scripts/check --bg` + `scripts/check wait`, never a `pgrep -f` loop (it matches itself and never exits). Iterate with `scripts/check test <filter>` (or `scripts/check clippy`) and run the full `scripts/check` once at the end, not after each change. If it touches editing, drawing or search hot paths, also the release `milestone` / `search` budget runs from CLAUDE.md (through `tail -40`).
   - Update the docs for what it adds (module map, conventions, test helpers, background-work patterns), within the budget in CLAUDE.md's "Keeping these files current": at most a line or two in the root CLAUDE.md, the detail in `docs/dev/` or the directory's nested CLAUDE.md.
   - Don't commit, and don't edit PLAN.md or `tasks/`.
   - Report concisely: what it built, files changed with the key entry points (`path:line` of the main new functions and tests, for the validator), CLAUDE.md changes, deviations, deferrals, hand checks, and check results.
7. **Validator** (a separate general-purpose agent with `model: "sonnet"`, started after the implementer finishes: the judgement half). Its prompt includes the implementer's report verbatim and says:
   - Read CLAUDE.md. The task (or subtask) text is pasted into the prompt, as for the implementer; it doesn't re-read PLAN.md. Start from `git status` / `git diff --stat` and the report's entry points; read the changed code and its tests, not a fresh tour of the codebase.
   - Read narrowly, as the implementer does: diff one file or hunk at a time (`git diff <file>`, never a whole multi-file diff), ranges of ≤ 150 lines, no `cat` of files over ~300 lines; independent reads in one message.
   - Read [docs/dev/review.md](../../docs/dev/review.md) and apply **all** of it to the diff: every lens (tests and coverage, correctness, performance, code re-use, language, what the next agent reads, hygiene), starting from the task as it says. It is the same file the review bot applies after the merge request opens: what the validator settles here costs one agent's turn, what it leaves costs a review round.
   - Act on each finding by its disposition: fix every `blocking` and `fix` finding itself (add or strengthen weak tests, rewrite worthless ones, fix real bugs minimally with a test, correct docs); list each `followup` in its report without doing it.
   - Write a **mutation plan** for the important behaviours, 4–8 mutations, to `<scratchpad>/mutations.md` (the orchestrator's scratchpad path, given in the prompt). Each: an id, the file, the exact text to replace and its replacement (unique in the file, so it applies mechanically), what behaviour it breaks, and the `scripts/check test <filter>` that should fail. Don't run the mutations itself.
   - Run `scripts/check` once (all three checks) after its own changes. Random or property-style tests cap their iterations and output sizes, so a bug fails them quickly instead of eating CPU and RAM.
   - Update the docs if it changes helpers or conventions (CLAUDE.md's budget applies). Don't commit, and don't edit PLAN.md or `tasks/`.
   - Report: verdict, test changes, bugs fixed, other findings fixed (by lens), every `followup` (location, problem, the fix), the mutation plan's path, check results.
8. **Mutation runner** (general-purpose agent with `model: "haiku"`: the mechanical half; started after the validator). Its prompt gives the plan's path and says:
   - Don't edit tests or code except to apply and undo the planned mutations. Before the first, save `git diff | shasum` and copy each file it will mutate to the scratchpad.
   - For each mutation in order: apply it exactly (if the text isn't found, record "not applied" and go on), run its `scripts/check test <filter>`, record caught (a test failed) or survived (all passed; note a compile error as "caught by the compiler"), then restore the file from the copy. After the last, check `git diff | shasum` matches the saved one.
   - Then run `scripts/check` 3 times to catch flakiness (it already caps runs with `scripts/timeout`; never background loops or `alarm` + `exec` wrappers, which leave the test binary running). Check `pgrep -fl deps/jed-` is empty.
   - Report only a table (id, caught / survived / not applied, the failing test) and the three runs' result lines, plus the output of any failing test, and say if any output contains `FLAKE`.

   Then (you, not in the runner's prompt): if mutations survived, or a test related to this task (one it added or changed, or one covering code it touched) was flaky, send the table to the validator (`SendMessage`, so it keeps its context) to strengthen the tests or fix the flake then and there, then have the runner (`SendMessage`) re-run just those mutations and the 3 runs. A second round that still leaves survivors goes to the user. A flaky test unrelated to the task goes through [Flaky tests](#flaky-tests) instead.
9. **Check and commit (you):**
   - Run `scripts/check` once yourself; don't re-read files the agents already reported on. A flaky test elsewhere follows [Flaky tests](#flaky-tests): never ignored, never silently retried until it passes.
   - Make sure the diff includes the doc updates (CLAUDE.md files, `docs/dev/`).
   - Anonymise: the diff, task files and commit message must not contain the user's real server hostnames, IPs, usernames or private repo paths (CLAUDE.md "No private hosts"). Grep the diff for any you know of and replace them with `git.example.com` / `alice/project` before committing. Tell every subagent this rule in its prompt.
   - Check no test binaries are left running (`pgrep -fl deps/jed-`); kill any strays.
   - Update `tasks/<id>.md` (and regenerate PLAN.md's index) in the same commit:
     - A subtask that isn't the last: mark its line ✅, with notes on what it changed for later subtasks.
     - A single task, or a task's last subtask: mark the task ✅ with short sub-bullets — what it changed (for later work), "Also:", "Deferred:" and "Still to check by hand:" — then set `status: in-progress` after the first subtask and `status: done` after the last, and update `summary`; regenerate the index with `scripts/tasks index --write`. The full entry already lives in `tasks/<id>.md`; nothing moves.
     - The validator's `followup` findings go under the task's "Deferred:" (one line each: where, what, the fix).
     - Fix other tasks or plan sections if this work changed them.
   - Append this task's (or subtask's) block to `<scratchpad>/evidence.md`, in the format of docs/dev/review.md's "Evidence": a `### <task or subtask id>` heading, then "Built by" (`/task` in Claude Code; the model each agent ran on), the mutation runner's final table plus the behaviours the plan left without a mutation, deviations from the task text, flaky tests met, and the hand checks. Facts from the agents' reports only; no private hosts. It is not committed: `scripts/mr` puts it in the merge request.
   - Commit everything. Subject: a single task's is a short imperative naming the feature (e.g. `Add render whitespace`); a subtask's is `<Task name> (<id>-a): <summary>`. Then a blank line and `- ` bullets of behaviour and notable internals. Do not add a `Co-Authored-By:` trailer.
10. Give the user a short update after each commit: what landed, anything deferred, the hand checks. For a task with subtasks, go straight on to the next subtask (still in the worktree) without stopping or asking: keep going until every subtask is committed and the task is marked done, pausing only for a "Stop and ask the user when" case. When the task is done, go to step 11.

## Hand over for review

11. When the whole task is done (a single task, or its last subtask is committed), open a merge request in Gitea and see its review through. You fix what the review bot asks for: it never pushes to the branch.
    - Re-run the token check from step 1. If it fails, don't ask: push the branch with `git push --force-with-lease -u origin <branch>` (the script would have done the same), skip the rest of this step, and say in the final summary that no MR was opened (no token) and give the branch name. Keep the worktree.
    - From the task's worktree (clean, on the branch), run `scripts/mr -e <scratchpad>/evidence.md -w worktree-<name> "<task name>"` with the Bash tool's `run_in_background` (a review takes minutes to half an hour, past the foreground limit), and wait for its notification; don't poll its output. It pushes the branch, opens the MR against `main` (or updates the open one's description), prints the URL, then waits for the review bot's result on the pushed head and prints it. If merge-bot pushes the branch meanwhile (a resolved rebase), it follows the new head and resets the worktree to it. Act on its exit status:
      - **0, approved (or already merged):** the review is done; stop and clean up. `ExitWorktree` with `remove` (the commits are on `origin`; discard is safe), then `git branch -D worktree-<name>` from the main checkout. The merge bot rebases it onto `main`, runs `scripts/check` and merges it; nothing more is yours to do.
      - **1, changes requested:** it printed the findings to fix (`blocking`, and `fix` the first time; later `fix` findings the bot files itself). Fix every one, as the validator would: send them to the implementer (`SendMessage`, so it keeps its context; a new general-purpose agent with the brief if it's gone), real bugs with a test, never weakening a test. Then run `scripts/check` yourself, check the fixes don't add a private host, and commit: subject `Review fixes: <task name>`, then `- ` bullets of what changed, no `Co-Authored-By:` trailer. Run the same `scripts/mr … -w …` again (in the background) for the next round. A finding you judge wrong is still answered in code or by a test that shows it wrong, never skipped silently: say which and why in the commit body. After **3 rounds** that still end in changes, stop: keep the worktree and report the open findings to the user.
      - **3, waits for the user** (a path `.jim-bots.toml` holds, a diff the bot could not read in full, a broken policy): stop and report what it printed under "needs the user"; keep the worktree. The user approves it by adding `bot:review-approved`, or asks for changes.
      - **4, no usable result:** a timeout (is `jim-bots@review` running?) gets one more `scripts/mr -w worktree-<name>` (no new commits: it only waits again); a review error, an empty diff or a closed MR, or a second timeout: stop and report its output, keeping the worktree.
    - "hand check (information only)" lines are for the report, never a reason to wait or change code.
    - Report, shortest first: anything that needs the user (open findings, a hold), the MR URL, the review result and rounds taken, the task's hand checks, and a **flaky tests** section (every unrelated flake met along the way: test, how it failed, whether `FLAKE` debugging was added or already existed, and the `FLAKE` output with what it suggests). Then stop: don't start another task unless asked.
    - A later task branches from local `main`, so it won't contain a task whose MR is still open; if it depends on one, say so and wait for the merge.

## Flaky tests

A flaky test related to the task (one it added or changed, or one covering code it touched) is always fixed then and there by the validator (step 7). The rest of this section is for a test unrelated to the task that fails and then passes on a re-run with no code change (in any agent's run or your own):

- **First time (no `FLAKE` in its output):** don't try to fix it. Have the agent that saw it (or do it yourself) add debugging to that test, tagged with the keyword `FLAKE` so it's easy to grep: a `// FLAKE: <date>, <how it failed>` comment above the test, and assertion messages / an `eprintln!("FLAKE <test name>: …")` on the failing path that dump the state needed to diagnose it next time (the values compared, timings, events seen, the settle condition's inputs). The debugging must not change what the test checks or add sleeps or retries. Confirm `scripts/check test <its name>` passes, then move on with the task. The debugging goes in the task's commit, with a bullet saying which test flaked and how; note it for the final report (don't ask what to do about it).
- **Seen again (a failure whose output contains `FLAKE`):** still don't stop or ask, and don't fix it (unless it's related to the task, above). Keep the existing debugging, extending it only if its output was too thin to diagnose from, and carry on. Collect the test, the `FLAKE` output and what it suggests is wrong for the final report.
- Tell every agent's prompt (implementer, validator, runner) this rule in one line: a flaky test unrelated to the task gets `FLAKE` debugging and no fix (one related to it is fixed), and any `FLAKE` output it sees is reported back to you, not acted on.

## Stop and ask the user when

- The task is too big or not ready (step 5).
- A subtask needs something the agents can't get, such as tools to install or credentials.
- A design decision isn't settled by PLAN.md and would be costly to reverse.
- Checks still fail after an agent says it's done, and a second attempt doesn't fix them.
