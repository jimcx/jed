---
description: Do one PLAN.md TODO item as a single task — check its size, then implement, validate and commit it with subagents
argument-hint: "[TODO item name, e.g. \"Render whitespace\"]"
---

Do one item from PLAN.md's `## TODO` list with subagents, as one task and one commit. The item: $ARGUMENTS

## Pick the item

1. `git log --oneline -15`, `git status`, and the `## TODO` section of PLAN.md.
2. If no item was named, list the open TODO items (not ✅) in a few lines and ask the user which one. Stop until they answer.
3. If the tree has uncommitted changes, earlier work was interrupted:
   - If it's this item's work and looks complete, run the validator (step 6) and commit.
   - If it's partial, have an implementer finish it (tell it what's already there).
   - If it's something else, or broken beyond repair, stop and tell the user. Don't stash or discard it yourself.

## Check the size first

4. A TODO item is meant to be one task (one implementer, one validator, one commit). Before starting, check that it is:
   - Read the item, every PLAN.md section it links, and enough of the code it touches (CLAUDE.md's module map says where) to judge the work.
   - It's too big when it touches many unrelated areas, needs a groundwork refactor before the feature, has parts that are each a feature on their own, or would leave fmt, clippy or tests red partway through. Example: soft wrap (the view's one-row-per-line assumption, up / down, mouse hits, scroll).
   - It's also not ready when the plan leaves a design decision open that would be costly to reverse, or it needs something the agents can't get (tools to install, credentials, a terminal that isn't on this machine).
   - If it's too big or not ready, **stop and report to the user**: why, and the open question if there is one. Don't start building. For an item that's too big, offer both ways forward and say which you'd pick:
     - **Split it** into 2–5 smaller TODO items that each leave the checks green. If they agree, write the new items into the TODO list in place of the old one, in a plan-only commit, then do the first one when asked.
     - **Make it a milestone** — better when the parts build on each other towards one feature with a clear "Done when" (e.g. soft wrap). Offer a draft entry: the next milestone number, a title, "Needs", the feature bullets (from the item and the code you read), "Done when", and a suggested split into 3–5 tasks. If they agree, add it to PLAN.md's `## Milestones` (after the last one, without ✅), move any design detail into the right Features section, remove the item from the TODO list (or leave a one-line pointer to the milestone), and commit it as a plan-only change. Then tell the user to run `/milestones` to build it (or run it if they ask).
   - If it fits, tell the user in a line or two what the task will cover, then go on.

## The task

5. **Implementer** (general-purpose agent, run in sequence and wait for it). Its prompt says:
   - Read `CLAUDE.md` at the repo root first, plus the nested CLAUDE.md of any directory it edits. PLAN.md is the source of truth for design.
   - Quote the TODO item in full, and read every PLAN.md section it links.
   - Build only this item. Unit tests go in modules; end-to-end tests go in `src/todo_tests.rs` (create it with a `//!` doc and register `#[cfg(test)] mod todo_tests;` in `main.rs` if it doesn't exist), one `mod <item>` block per item, tests named after the behaviour. Tests go through real code paths (see CLAUDE.md's Tests section for driving the app headlessly).
   - Any new Cmd binding also updates `docs/terminals/`; any new setting follows CLAUDE.md's "Settings layer per key"; any new colour goes through `Theme`.
   - `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass (`scripts/timeout 300 cargo test …`). If it touches editing, drawing or search hot paths, also the release `milestone` / `search` budget runs from CLAUDE.md.
   - Update the CLAUDE.md files for what it adds (module map, conventions, test helpers, background-work patterns).
   - Don't commit, and don't edit PLAN.md's TODO section.
   - Report concisely: what it built, files changed, CLAUDE.md changes, deviations, deferrals, hand checks, and check results.
6. **Validator** (a separate general-purpose agent, started after the implementer finishes). Its prompt says:
   - Read CLAUDE.md, then the TODO item. Judge whether the tests are useful and actually test the code. Look for tautologies, assertions that can't fail, checks against test-only hooks instead of real effects, missing negative cases, and flakiness.
   - Mutation-test the important behaviours: break the code, confirm a test fails, then revert.
   - Check coverage against every part of the TODO item (and its "Done when", if it has one).
   - Add or strengthen weak tests and rewrite worthless ones. Fix real bugs minimally and report them.
   - Run `cargo test` 3 times to catch flakiness. Run tests in the foreground with a timeout (`scripts/timeout 300 cargo test …`, which kills the whole process group; `alarm` + `exec` wrappers leave the test binary running), never as unattended background loops. Random or property-style tests cap their iterations and output sizes. All three checks must be clean.
   - Update CLAUDE.md if it changes helpers or conventions. Don't commit.
   - Report: verdict, mutations caught or survived, test changes, bugs fixed, and check results.
7. **Check and commit (you):**
   - Re-run all three checks yourself (see CLAUDE.md's Toolchain section for how to reach cargo on this platform). A flaky test elsewhere is fixed, not ignored.
   - Make sure the diff includes the CLAUDE.md updates.
   - Check no test binaries are left running (`pgrep -fl deps/jed-`); kill any strays.
   - In the same commit, mark the item ✅ in PLAN.md's TODO list, with short sub-bullets: what it changed (for later work), "Deferred:" and "Still to check by hand:". Fix other TODO items or plan sections if this work changed them.
   - Commit everything with a short imperative subject naming the feature (e.g. `Add render whitespace`), a blank line, then `- ` bullets of behaviour and notable internals. End with the Co-Authored-By trailer.
8. Give the user a short update: what landed, anything deferred, the hand checks. Don't start another TODO item unless asked.

## Stop and ask the user when

- The item is too big or not ready (step 4).
- A design decision isn't settled by PLAN.md and would be costly to reverse.
- Checks still fail after an agent says it's done, and a second attempt doesn't fix them.
