---
description: Do a small fix or feature from a one-line instruction yourself (no subagents, no task file) — in a worktree, asking the user only when something needs clarifying; commit, open a merge request in Gitea, fix the review bot's findings until it approves, then remove the worktree
argument-hint: "[instruction, e.g. \"fix the crash when closing the last tab of a group\"]"
---

Do one small change: a bug fix or a little feature. The instruction: $ARGUMENTS

This is `/task`'s light sibling. You do the work yourself (**no subagents**), there is no `tasks/<id>.md` file, and testing is proportionate: a focused test for what changed, no validator, no mutation runs. It runs **straight through**: once the work is done and the checks pass you commit and open the MR without asking. Ask the user only when something genuinely needs clarifying — an ambiguous instruction, a choice that is theirs (behaviour, key binding, setting name), a change bigger than a quick task, or a review finding you disagree with. Everything else — worktree, commit style, `scripts/mr`, the review loop, the invariants in CLAUDE.md — is as in [task.md](task.md), which this file points to rather than repeats.

## 1. Understand and agree (with the user)

- If `$ARGUMENTS` is empty, ask what to do and stop until they answer.
- Check the MR can be opened at the end: `command -v tea >/dev/null || [ -n "$GITEA_TOKEN" ]`. If neither holds, say so now (`GITEA_TOKEN` exported in this session's environment, or `tea` logged in); never ask for the token's value.
- Find the code: CLAUDE.md's "Where things are", [docs/dev/modules.md](../../docs/dev/modules.md), `grep -n` or the LSP tool, then ranges of ≤ 150 lines. Read the nested CLAUDE.md of each directory you'll edit and any [conventions](../../docs/dev/conventions.md) section the change touches. For a bug, find the cause before proposing a fix.
- If it turns out bigger than a quick task (more than ~4 files, a second subsystem, a design question PLAN.md doesn't settle), say so and suggest `/task` with a task file instead; stop unless the user wants to go on anyway.
- Tell the user in a few lines what's wrong / what you'll add, the files you'll change and the test you'll write, then carry on — don't wait for a go-ahead. Ask with `AskUserQuestion` only where something needs clarifying (an ambiguous instruction, a choice that's theirs: behaviour, key binding, setting name), and adjust the plan to their answer.

## 2. Worktree

- `EnterWorktree` with a short slug of the change (`fix-close-last-tab`) as the name: the branch is `worktree-<slug>`. Confirm it sits at local main's tip (`git log --oneline -1`); if not, `git rebase main`. Note the main checkout's path.
- A worktree with that name already existing means a resume: enter it with its `path` and carry on from where it is (`git log main..HEAD`, `git status`).

## 3. Do the work

- Make the change, following CLAUDE.md's invariants (positions, `Buffer::edit`, selection lists, keymaps as data, settings via `for_buffer`, colours via `Theme`, never blocking the UI thread).
- **Tests, light:** one or two tests that pin the change — for a bug, a regression test that fails without the fix (check that once by reverting the fix line, then restore it); for a feature, its main behaviour. A unit test in the module when it can show it; else an end-to-end test in `jed/src/todo_tests.rs` in a `mod <slug>` block, driven by `test_execute` (docs/dev/testing.md). No test for a pure doc or text change.
- Iterate with `scripts/check test <filter>` and `scripts/check clippy`; run the full `scripts/check` **once**, at the end, before committing (the review and merge bots run it too, but a red branch costs a review round). A flaky test unrelated to the change: note it for the report, don't fix it.
- Docs: only if the change adds or alters a command, binding, setting, convention or helper (a new Cmd binding also updates `docs/terminals/`). CLAUDE.md's budget applies.
- No private hosts in the diff or commit message (CLAUDE.md).

## 4. Commit

- Tell the user in a few lines what changed (`git diff --stat`), the test, and anything to check by hand (how to see it in the running app), then commit and go straight on to the MR — don't ask first.
- Commit: a short imperative subject (`Fix …`, `Add …`), a prose or `- ` bullet body saying why when it isn't obvious, no `Co-Authored-By:` trailer.
- Write `<scratchpad>/evidence.md`: `### <slug>`, then **Built by:** `/quicktask` in Claude Code (the model), no subagents; **Mutations:** none (quick task) — the regression test was checked to fail without the fix, if it was; **Instruction:** the user's request in a line and any decisions they made in step 1 (the bot reviews against this, as there is no task file); **To check by hand:** the items, if any.

## 5. Review loop

- If the token check fails now, push with `git push --force-with-lease -u origin worktree-<slug>`, tell the user no MR was opened and the branch name, and stop (keep the worktree).
- Run `scripts/mr -e <scratchpad>/evidence.md -w worktree-<slug> "<subject>"` with the Bash tool's `run_in_background` and wait for its notification (don't poll). Tell the user the MR URL as soon as it prints. Its exit status, as in [task.md step 11](task.md#hand-over-for-review):
  - **0, approved (or merged):** go to step 6.
  - **1, changes requested:** list the findings for the user in a line each, with what you'll do about each. Fix the ones that are clearly right straight away; for one you think is wrong, or one that changes behaviour the user chose, ask them (`AskUserQuestion`) before acting. A finding you don't follow is still answered — in the commit body, with the reason, or by a test showing it wrong. Run `scripts/check` once, commit `Review fixes: <subject>` with `- ` bullets, and run the same `scripts/mr … -w …` again. After 3 rounds still ending in changes, stop and hand the open findings to the user (keep the worktree).
  - **3, waits for the user:** report what it printed under "needs the user" and stop (keep the worktree); they approve with `bot:review-approved` or ask for changes.
  - **4, no usable result:** a timeout gets one more `scripts/mr -w worktree-<slug>`; anything else, or a second timeout, report its output and stop (keep the worktree).

## 6. Clean up

- `ExitWorktree` with `remove` (the commits are on `origin`), then from the main checkout `git branch -D worktree-<slug>`. The merge bot merges it.
- Report in a few lines: the MR URL, rounds taken, what the review changed, anything to check by hand, flaky tests met.
