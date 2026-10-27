---
id: compare_files
name: Compare any two files
summary: compare any two files in a diff tab
status: open
priority: 0
dependencies: []
---

See [Git](../PLAN.md#git-view-and-stage-only). "File: Compare Active File With…" (palette, unbound) picks another file with quick open's file list and opens a diff tab of that file ↔ the active buffer (`diff_view::Text::File(path)` — a file's text, its open buffer's when open, else the disk — on the left; the right side `Buffer` as File History's Compare); "Explorer: Select for Compare" then "Explorer: Compare with Selected" (selected rows, or the active file for the second); titled `a.rs ↔ b.rs`, no staging / revert, follows edits on either side (`Later` reloads as now), next / previous change; sessions keep it (`Side::File`); binary files say "binary" as now
