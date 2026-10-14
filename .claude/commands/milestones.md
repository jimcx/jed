---
description: Resume the milestone workflow — implement, validate and commit PLAN.md milestones task by task with subagents
---

<!-- TEMPORARY: delete this command (and its PLAN.md TODO entry) once the last milestone in PLAN.md is done. -->

Work through the milestones in PLAN.md with subagents, one task at a time, in sequence. $ARGUMENTS

## Find where we are

1. `git log --oneline -15`, `git status`, and the `## Milestones` section of PLAN.md.
   - The first milestone without ✅ is the current one.
   - Task commits are named `Milestone N (Na): …`. The last one shows the last finished task.
2. If the tree has uncommitted changes, a task was interrupted:
   - If it looks complete, run the validator (step 4) and commit.
   - If it's partial, have an implementer finish it (tell it what's already there).
   - If it's broken beyond repair, `git stash` it and restart the task. Tell the user.
3. If the current milestone has no task commits yet, split its PLAN.md entry into 3–5 tasks that build on each other. Each task must leave fmt, clippy and tests green. If it has some, re-derive the split from the plan and the commits so far. Tell the user the split in a few lines.

## Each task

4. **Implementer** (general-purpose agent, run in sequence and wait for it). Its prompt says:
   - Read `CLAUDE.md` at the repo root first, plus the nested CLAUDE.md of any directory it edits. PLAN.md is the source of truth for design.
   - List every task in the milestone, mark which one is its own, and say what's already done.
   - Read the milestone entry and every PLAN.md section it links.
   - Build only this task. Write tests in `src/milestoneN_tests.rs` (create it and register it in `main.rs` if it doesn't exist) and unit tests in modules. Tests go through real code paths.
   - Any new Cmd binding also updates `docs/terminals/`.
   - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass.
   - Update the CLAUDE.md files for what it adds (module map, conventions, test helpers, background-work patterns).
   - Don't commit, and don't edit PLAN.md's Milestones section.
   - Report concisely: what it built, files changed, CLAUDE.md changes, deviations, deferrals, hand checks, and check results.
5. **Validator** (a separate general-purpose agent, started after the implementer finishes). Its prompt says:
   - Read CLAUDE.md. Then judge whether the task's tests are useful and actually test the code. Look for tautologies, assertions that can't fail, checks against test-only hooks instead of real effects, missing negative cases, and flakiness.
   - Mutation-test the important behaviours: break the code, confirm a test fails, then revert.
   - Check coverage against the plan's bullets for the task, and against "Done when" for the milestone's last task.
   - Add or strengthen weak tests and rewrite worthless ones. Fix real bugs minimally and report them.
   - Run `cargo test` 3 times to catch flakiness. Run tests in the foreground with a timeout (macOS has no `timeout`: `perl -e 'alarm shift; exec @ARGV' 300 cargo test …`), never as unattended background loops. Random or property-style tests cap their iterations and output sizes, so a bug fails them quickly instead of eating CPU and RAM. All three checks must be clean.
   - Update CLAUDE.md if it changes helpers or conventions. Don't commit.
   - Report: verdict, mutations caught or survived, test changes, bugs fixed, and check results.
6. **Check and commit (you):**
   - Re-run all three checks yourself (cargo needs `export PATH="$HOME/.rustup/toolchains/1.99-aarch64-apple-darwin/bin:$PATH"`).
   - Make sure the diff includes the CLAUDE.md updates.
   - Commit everything with the subject `Milestone N (Na): <summary>`, a blank line, then `- ` bullets of behaviour and notable internals. End with the Co-Authored-By trailer.
7. **On the milestone's last task**, in the same commit:
   - Mark it ✅ in PLAN.md and note any "Also:", "Deferred:" and "Still to check by hand:" items.
   - Fix later milestones' entries if this work changed them.
   - Bump "Milestones 1–N are done" in CLAUDE.md.
8. Give the user a short update after each commit. Then move on to the next task or milestone.

## Stop and ask the user when

- A task needs something the agents can't get, such as tools to install or credentials (for example clangd or pyright for M9).
- A design decision isn't settled by PLAN.md and would be costly to reverse.
- Checks still fail after an agent says it's done, and a second attempt doesn't fix them.
