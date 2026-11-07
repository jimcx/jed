---
id: scm_branch_history
name: Source Control branch history
summary: a commits pane under the Source Control view listing the current branch's history; a click opens the commit tab
status: done
priority: 0
dependencies: [git_history]
---

The Source Control view splits vertically: the changes above, a collapsible "COMMITS" pane below with the current branch's commits (`git log` from HEAD, newest first, summary / author / date / short id as the Explorer's Git History rows), paged off-thread and reloaded when HEAD moves. A click on a commit opens the same commit tab a Git History row opens.

  - Built: `git::log::branch` (revwalk by committer date), `app/branch_history.rs` (`app.branch_history`, a `GitHistory` without a file; `sync_branch_history`, `GitEvent::BranchHistory`, `side_history()` picks it over the Explorer's under Source Control), `ui::side::render_history(.., branch)` shared with the Explorer section, `ui/scm.rs` splitting with `side_sections::split`; header click, divider drag, wheel and bar reuse the `Hit::History*` / `ScrollTarget::History` paths
  - Test: `scm_branch_history_pane` (`git_history_tests.rs`)
  - Deferred: no keyboard focus or keys for the pane; the collapse and height aren't kept in sessions
