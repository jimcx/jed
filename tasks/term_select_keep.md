---
id: term_select_keep
name: Keep the terminal selection across new output
summary: highlighted text in a shell or task tab follows its text when output scrolls the screen; cleared only when the rows under it change
status: done
priority: 0
dependencies: [term_select_scrollback]
---

See [Tasks](../PLAN.md#tasks-throwaway-terminals). Today any new output clears a shell or task tab's mouse selection (`Event::Output` in `app/shell.rs`, the task equivalent in `app/tasks.rs`), so highlighted text vanishes the moment a prompt redraws or a build prints a line. Keep it: the selection stays on the same text while output scrolls the screen, as `term_select_scrollback` already does for the wheel.

  - Needs: `term_select_scrollback` (`select::Cell` is scroll-independent: line 0 = the live screen's first row, scrollback negative), `tasks::run` (`clear_selection`, `set_selection`, the reader that feeds the vt100 parser), the vt100 fork (`[patch.crates-io]`, tag `v0.18.1+jed`)
  - Know how many lines each chunk of output scrolled into the scrollback: vt100 doesn't report it (the earlier task's "Deferred"), so add a counter in the jed fork (e.g. `Screen::scrolled_lines`, total lines pushed into scrollback, monotonic and unaffected by the scrollback cap) and have `Run` compare it before and after `process`
  - After a chunk that scrolled `n` lines, shift the selection's anchor and head lines by `-n` (`select::Selection::shift`), so it follows its text up into the scrollback; a part that falls off the kept scrollback clamps to its oldest row (as `copy_text` already does), and a selection wholly gone is dropped
  - Output that rewrites cells in place without scrolling (a progress line, `\r`, a full-screen redraw, the alternate screen, `clear`) can change the highlighted text: clear the selection when the cells under it changed. Cheapest sound rule: keep it only while the rows it covers are unchanged since it was made (compare a hash or the `contents_between` text of the covered rows before and after each chunk); otherwise clear. Decide and document the rule in `tasks/select.rs`'s `//!`
  - A resize still clears it (the rows reflow); "Terminal: Clear" still clears it; a click still clears it; a program that asked for the mouse is unchanged
  - While the user holds a drag, output arriving mid-drag keeps the anchor on its text and the head on its screen cell
  - Done when: with a shell tab, select text from earlier output and run a command that prints enough lines to scroll the screen — the highlight moves up with its text and Cmd+C / Ctrl+C still copies exactly it; a prompt redraw that leaves the selected rows unchanged keeps it, while output that overwrites those rows clears it; a selection pushed past the kept scrollback (`scrollback = 50`) is clamped or dropped without panicking; the same in a task tab; resize, click and Clear still clear it
  - Deferred: auto-scrolling while a drag leaves the edge
  - Tests: `tasks::select` units for `shift` (clamp, drop, anchor/head order); `term_select_tests.rs` cases `term_select_keep_*` (output scrolls it, in-place rewrite clears it, scrollback cap, mid-drag output, resize / click / Clear clear it) for shell and task tabs; update `term_select_output_clears_and_the_wheel_moves_the_selection`
  - Still to check by hand: in a real terminal, select a line in a shell, run `seq 1 200` and `make`, and watch the highlight; a prompt with a clock or git status redrawing under the selection
  - Built: vt100 fork `Screen::scrolled_lines` (fork branch `scrolled-lines`, commit 7e68b36; to be tagged `v0.18.1+jed2`); `Run::process_chunk` snapshots the selection's rows before each PTY chunk and `select::follow` shifts / clamps / drops it after (`Selection::shift`), clearing it when a covered row's text or the scrollback length changed; a held drag shifts only the anchor (`Run::set_dragging` / `take_drag_lines`, `Drag::Terminal`). Tests: `term_select_keep_*`, `select` units.
  - Also: the old output test is renamed `term_select_output_keeps_and_the_wheel_moves_the_selection` (it had selected the wrong row).
  - Deferred: auto-scrolling while a drag leaves the edge. Known gap: if a left Up never reaches the terminal, `Run::dragging` stays set until the next press there.
