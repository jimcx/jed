---
id: compare_files
name: Compare any two files or buffers
summary: compare any two files, open or unsaved buffers (dirty or untitled), in a diff tab
status: open
priority: 0
dependencies: []
---

See [Git](../PLAN.md#git-view-and-stage-only).

  - Goal: a diff tab of any two texts, each a file on disk or an open buffer, saved or not (a dirty file's unsaved text, an untitled buffer), as VS Code's "Compare Active File With…", "Select for Compare" / "Compare with Selected" and "Compare Selected" do
  - Sides: a side is a file (`diff_view::Text::File(path)`: the open buffer's text when open, else the disk) or an open buffer by identity (`Text::Open(BufferId)`: its live text, saved or not; the only way to name an untitled buffer, which has no path). Not in `Text::Disk`: a dirty file compared with its own saved text stays "Compare with Saved"
  - Commands (palette, unbound unless noted): "File: Compare Active File With…" opens a picker listing the open buffers first (untitled ones by their title, dirty ones marked) then quick open's file list, and opens a diff tab of the picked text on the left ↔ the active buffer on the right; "File: Compare Open Editors…" picks two open buffers in turn; "Explorer: Select for Compare" then "Explorer: Compare with Selected" (two selected rows, or the active file for the second); "File: Select Active File for Compare" / "Compare Active File with Selected" for the editor side, so two buffers can be compared without the picker. Comparing a text with itself says so and opens nothing
  - Tab: titled `a.rs ↔ b.rs` (an untitled buffer by its title, a dirty side marked), no staging or revert, read-only; follows edits on either side as the buffer changes (`Later` reloads as now) and a closed buffer's side freezes to its last text rather than closing the tab; next / previous change; binary files say "binary" as now. Selecting text on a side first compares just the selections, as `diff_tab_selected_lines` does, if that task has landed
  - Sessions: a file side is kept (`Side::File`); an untitled side is kept by its crash-backup id (as `TabState::Untitled`), and the tab is dropped when that buffer no longer exists
  - Needs: the git diff view (`diff_view`, `DiffTab`) and quick open's file list; reads of unopened files go through `spawn_blocking` as for other diff sides
  - Tests: `todo_tests::compare_files`: file ↔ file; file ↔ dirty buffer (the unsaved text shows, the disk's doesn't); untitled ↔ file and untitled ↔ untitled; typing in either side updates the diff; closing a side's buffer freezes it; same text twice refused; Explorer select / compare; session round trip including an untitled side; side-by-side and inline layouts draw
  - Done when: any two files, dirty buffers or untitled buffers can be compared in a diff tab from the palette, Explorer or picker, and the diff follows edits on both sides; and `scripts/check` passes
  - Still to check by hand: comparing an untitled scratch buffer with a file while typing in it; a dirty file against another file; restoring a session with an untitled side
