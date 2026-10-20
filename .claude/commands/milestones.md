---
description: Resume the milestone workflow — implement, validate and commit PLAN.md milestones task by task with subagents
---

Build one of PLAN.md's milestones with subagents, one task at a time, in sequence. $ARGUMENTS

## Find where we are

1. `git log --oneline -15`, `git status`, and the `## Milestones` section of PLAN.md.
   - Milestones 1–13 are done. Later milestones are under `### Named milestones`: they have names and short ids (`tools`, `soft_wrap`, …), not numbers, and **no order**.
   - The current one: the milestone named in the arguments (by name or id); else one with some tasks ✅ but not all (work in progress); else list the named milestones without ✅ in a few lines and ask the user which. Stop until they answer.
   - Task commits are named `<Milestone name> (<id>-a): …`. The last one shows the last finished task.
2. If the tree has uncommitted changes, a task was interrupted:
   - If it looks complete, run the validator (step 4) and commit.
   - If it's partial, have an implementer finish it (tell it what's already there).
   - If it's broken beyond repair, stop and tell the user. Don't stash or discard it yourself.
3. If the current milestone has no task commits yet, split its PLAN.md entry into 3–5 tasks that build on each other. Each task must leave fmt, clippy and tests green. If it has some, follow the task list already in its PLAN.md entry. Tell the user the split in a few lines. Write the split into the milestone's PLAN.md entry as a `- Tasks (one agent and one commit each):` list (one line per task, `<id>-a: …`, saying what it covers); mark each task ✅ in its own commit, with notes on what it changed for later tasks.

## Each task

4. **Implementer** (general-purpose agent, run in sequence and wait for it). Its prompt says:
   - Read `CLAUDE.md` at the repo root first, plus the nested CLAUDE.md of any directory it edits. PLAN.md is the source of truth for design.
   - List every task in the milestone, mark which one is its own, and say what's already done.
   - Read the milestone entry and every PLAN.md section it links.
   - Build only this task. Write tests in `src/<id>_tests.rs`, named `<id>_*` (create it with a `//!` doc and register it in `main.rs` if it doesn't exist) and unit tests in modules. Tests go through real code paths.
   - Any new Cmd binding also updates `docs/terminals/`.
   - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass.
   - Update the docs for what it adds (module map, conventions, test helpers, background-work patterns), within the budget in CLAUDE.md's "Keeping these files current": at most a line or two in the root CLAUDE.md, the detail in `docs/dev/` or the directory's nested CLAUDE.md.
   - Don't commit, and don't edit PLAN.md's Milestones section.
   - Report concisely: what it built, files changed, CLAUDE.md changes, deviations, deferrals, hand checks, and check results.
5. **Validator** (a separate general-purpose agent, started after the implementer finishes). Its prompt says:
   - Read CLAUDE.md. Then judge whether the task's tests are useful and actually test the code. Look for tautologies, assertions that can't fail, checks against test-only hooks instead of real effects, missing negative cases, and flakiness.
   - Mutation-test the important behaviours: break the code, confirm a test fails, then revert.
   - Check coverage against the plan's bullets for the task, and against "Done when" for the milestone's last task.
   - Add or strengthen weak tests and rewrite worthless ones. Fix real bugs minimally and report them.
   - Run `cargo test` 3 times to catch flakiness. Run tests in the foreground with a timeout (`scripts/timeout 300 cargo test …`, which kills the whole process group; `alarm` + `exec` wrappers leave the test binary running), never as unattended background loops. Random or property-style tests cap their iterations and output sizes, so a bug fails them quickly instead of eating CPU and RAM. All three checks must be clean.
   - Update CLAUDE.md if it changes helpers or conventions. Don't commit.
   - Report: verdict, mutations caught or survived, test changes, bugs fixed, and check results.
6. **Check and commit (you):**
   - Re-run all three checks yourself (see CLAUDE.md's Toolchain section for how to reach cargo on this platform).
   - Make sure the diff includes the CLAUDE.md updates.
   - Check no test binaries are left running (`pgrep -fl deps/jed-`); kill any strays.
   - Commit everything with the subject `<Milestone name> (<id>-a): <summary>`, a blank line, then `- ` bullets of behaviour and notable internals. End with the Co-Authored-By trailer.
7. **On the milestone's last task**, in the same commit:
   - Mark it ✅ in PLAN.md with any "Also:", "Deferred:" and "Still to check by hand:" items, then move its full entry (task list included) to `docs/plan-done.md`'s Named milestones, leaving its first line in PLAN.md ending in ` — [details](docs/plan-done.md)`. Rewrite `](#` links in the moved text as `](../PLAN.md#`.
   - Fix later milestones' entries if this work changed them.
8. Give the user a short update after each commit. Then move on to the milestone's next task. When the milestone is done, stop: don't start another one unless asked.

## Stop and ask the user when

- A task needs something the agents can't get, such as tools to install or credentials (for example clangd or pyright for M9).
- A design decision isn't settled by PLAN.md and would be costly to reverse.
- Checks still fail after an agent says it's done, and a second attempt doesn't fix them.
