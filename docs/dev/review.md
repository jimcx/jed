# Review criteria

What a change to jed is held to. This file is criteria only, no workflow: `/task`'s validator, `/task-review` and
the review bot (which reads it from `main`, with [`.jim-bots.toml`](../../.jim-bots.toml)) all apply it, each with
its own steps. Change a rule here, not in a copy.

The work is written by agents, with a human directing who does not review by hand. So a review is not advice for
an author to weigh: every finding is either fixed by the author before the change lands or routed somewhere it will
be handled ([Dispositions](#dispositions)). Look and feel are judged by the human after the merge: report no findings
on them, and list what can only be checked by hand as hand checks, for information.

## Start from the task

Find the task the change builds: the `tasks/<id>.md` file the diff touches (`scripts/tasks show <id>`), else the
issue or the description. Write down what it requires — its text, its "Done when", for a subtask its line in the
subtask list — **before** reading the diff, then check each item against the code and its tests. Don't report what
the task file already lists under "Deferred:", unless it is blocking.

Review the diff, not the code around it: look further only to grep for an existing helper, to see how hot a path
is, or to see what a test exercises. Skip generated files and PLAN.md's index.

## Lenses

A finding is concrete: a location (`path:line`), the problem, the fix. A lens with no findings is a fine result;
don't pad, and don't report what rustfmt or clippy already catch, or matters of taste.

### Tests and coverage

- Are the tests useful, and do they actually test the code? Look for tautologies, assertions that can't fail, checks
  against test-only hooks instead of real effects, missing negative cases, and flakiness (a result that depends on
  timing). Random or property-style tests cap their iterations and output sizes.
- Coverage against every part of the task, as listed above. End-to-end tests go through real code paths
  ([testing.md](testing.md)) and sit where CLAUDE.md's "Tests" says (`jed/src/todo_tests.rs` for a single task,
  `jed/src/<id>_tests.rs` for a task with subtasks).
- Where there is [evidence](#evidence), use it to aim: a mutation that survived, or a required behaviour with no
  mutation and no test, is where to look first.

### Correctness

- Real bugs in the changed code, and CLAUDE.md's "Invariants" (positions, text only through `Buffer::edit`, the
  selection list, undo grouping, never blocking the UI thread, forges, logging).
- No `unwrap`, index or arithmetic that real input can make panic.

### Performance

- Work per edit, per frame or per selection against CLAUDE.md's "Many selections" and "Never block the UI thread":
  rope lookups or allocations in a loop, needless `clone` / `collect` / `to_string`, quadratic scans, work repeated
  that could be cached or moved out of a loop. Say why the path is hot: on a cold path, the clearer code wins.

### Code re-use

- New code that duplicates an existing helper (grep for the obvious names; [modules.md](modules.md), `buffer::pos`,
  the helpers in [testing.md](testing.md)), and logic or test setup repeated within the diff. Extract a helper only
  for callers that exist now; no abstractions for later.

### Language

- Rust used to its strengths: enums and newtypes over flag `bool`s, strings and raw `usize`s, so bad states can't
  be represented; exhaustive `match` over a `_` arm that would hide a new variant; borrowing over cloning; `?` and
  let-chains over nesting; iterator adapters and std APIs over hand-rolled loops, where they read better; visibility
  no wider than needed.

### What the next agent reads

The next agent knows this code only through these, so a wrong one costs more than a slow function:

- The docs match what the change does: the module map, conventions, test helpers and background-work patterns
  ([docs/dev/](.), the nested `CLAUDE.md` files, a module's `//!` doc), within CLAUDE.md's budget in "Keeping these
  files current". Text that describes behaviour the change removed is a finding, not only text that is missing.
- Descriptions an agent acts on (MCP tool descriptions, command help, error messages that say what to do) are true.
- The task file is updated: a finished subtask's line marked ✅ with notes for later subtasks; a finished task marked
  ✅ with "Also:", "Deferred:" and "Still to check by hand:", `status` (`in-progress` after the first subtask, `done`
  after the last) and `summary` set, and PLAN.md's index regenerated (`scripts/tasks check`).
- Any new Cmd binding also updates `docs/terminals/`; any new setting follows the settings convention; any new
  colour goes through `Theme`.

### Hygiene

- No private hosts: the diff, task files and commit messages contain none of the user's real server hostnames, IPs,
  usernames or private repo paths (CLAUDE.md "No private hosts"); `git.example.com` / `alice/project` instead.
- Commit messages follow CLAUDE.md's "Commits". A message already pushed is not rewritten: report its format only
  when it would mislead someone reading the log (a wrong task id, a subject that says the opposite), as a follow-up.

## Dispositions

Each finding gets one. Fixing is cheap and a deferred finding is usually lost, so **fix is the default**.

| disposition | when | what happens |
|---|---|---|
| `blocking` | see below | fixed before the change lands |
| `fix` | correct, local to the diff, keeps behaviour | fixed now: in a session, by the reviewer; on a merge request, by the branch's author (`/task`), asked once per MR (later ones are filed) |
| `followup` | reaches well outside the diff, or is a piece of work of its own | filed: the bot opens a follow-up issue; in a session, it goes under the task's "Deferred:" |

There is no "ask the human": where the task leaves a choice open, judge it. Accept it, or report it as `blocking`
or `fix` with the fix you recommend, or as a `followup`.

Blocking in this project:

- a real bug, or a broken invariant from CLAUDE.md;
- a missing or worthless test for behaviour the task requires, or part of the task not done;
- a test that fails alone ([Failing tests](#failing-tests));
- a doc, task note or tool description from "What the next agent reads" that is **wrong** (missing detail is `fix`);
- a private host anywhere in the diff or its commit messages;
- a design problem that is costly to fix after merge.

Fixes keep behaviour: weak or worthless tests get strengthened or rewritten, real bugs a minimal fix and a test,
wrong or missing docs corrected. Never weaken or delete a test; change one only to follow a rename or signature
change. Never write a private host while fixing.

## Failing tests

When a test fails in a run, re-run it alone with `scripts/check test <its name>`:

- **It passes alone:** note it (the test and how it failed) in the report and move on. It counts as passing: don't
  re-run the suite, debug it or add debugging to it.
- **It fails alone:** it's a real failure: blocking.

## Evidence

`/task` attaches what it measured to the merge request, under an `## Evidence` heading, so a reviewer checks facts
instead of forming an impression. One block per subtask:

- **Built by:** the workflow and the models of its agents (implementer, validator, mutation runner).
- **Mutations:** the runner's table (id, the behaviour broken, caught / survived / not applied, the failing test),
  and which behaviours of the task have no mutation.
- **Deviations:** where the work departs from the task text, and why.
- **Flaky tests:** each one met, how it failed, and whether it is related to the task.
- **To check by hand:** the task's "Still to check by hand:" items, one line each.

A reviewer trusts a mutation result as far as its test still exists and still fails for that reason; it is worth
re-running one when a finding turns on it.
