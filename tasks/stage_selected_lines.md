---
id: stage_selected_lines
name: Stage / unstage / revert selected lines
summary: stage, unstage or revert just the selected lines
status: open
priority: 0
dependencies: []
---

See [Git](../PLAN.md#git-view-and-stage-only). VS Code's "Git: Stage Selected Ranges" / "Unstage Selected Ranges" / "Revert Selected Ranges" (palette, unbound): from a text tab, the lines its selections touch (every selection; an empty cursor = its line). Stage applies only those lines of each context-0 index → disk hunk to the index blob (an added line inside = added, a removed line inside = removed, the rest of the hunk left as it was — `git add -p`'s `e`dit result), checked against `Stale` like `stage_hunk`; unstage likewise HEAD → index; revert the same on the work tree (the buffer through `Buffer::edit`, one undo step). Saves a dirty buffer first as hunk staging does. A `git::stage::stage_lines` / `unstage_lines` / `revert_lines` in the model with tests against `git apply --cached` of the equivalent hand-edited patch
