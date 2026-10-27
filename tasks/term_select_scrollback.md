---
id: term_select_scrollback
name: Mouse selection in the terminal scrollback
summary: select scrolled-back terminal output with the mouse; a scroll keeps the selection on its text
status: done
priority: 0
dependencies: [term_select]
---

✅ Extend `term_select`'s left-drag to the scrolled-back view, so the older output the wheel and Ctrl+Shift+Up / Down reveal can be selected too — drawn highlighted and copied by Cmd/Ctrl+C, exactly as on the live screen. A program that has asked for the mouse keeps every event, unchanged.

  - Needs: `term_select` (`tasks/select.rs`, `Drag::Terminal` / `terminal_select` / `screen_cell` in `app/mouse.rs`), `tasks::run`'s `scroll` / `scroll_to`, the scroll-back bindings and the wheel
  - A drag over a shell or task tab seeds / extends the selection whether the view is live or scrolled back: the pointer maps to a screen cell through `screen_cell` / `select::top_row`, `draw_screen` paints the selection's rows, and `copy_selection` reads them through `contents_between` — the same path as a live-screen drag. Scrolled back, the wheel and the scroll keys still scroll the scrollback rather than forward to the program.
  - Scrolling the terminal does not affect the selected text: the selection moves with the text (this replaced the earlier "a scroll clears the selection"). New output and a resize still clear it.
  - Done when: over a shell and a task tab scrolled back with output above the screen, a left-drag highlights the older lines and Cmd+C / Ctrl+C copies exactly them (trailing spaces trimmed, wrapped rows joined); the wheel and Ctrl+Shift+Up / Down still scroll the scrollback and the selection moves with its text; a click still clears it and copies nothing; a mouse-reporting program (vim / tmux / htop) is unchanged; a live-screen drag still selects
  - What it changed: `select::Selection` cells are `Cell = (isize, u16)` in scroll-independent lines (0 = the live screen's first row, scrollback negative), converted with `select::cell_at(row, col, back)` / `viewport_row(line, back)`; `copy_text` takes `&mut Screen` and reads the selection a window at a time through `set_scrollback` (selections taller than the screen or scrolled out of view; `row_wrapped` joins wrapped rows across windows; the scroll offset is restored; lines older than the kept scrollback clamp). `Run::scroll` / `scroll_to` / `set_scrollback_from_top` no longer clear the selection; `resize` and new output still do. `terminal_select` no longer bails while scrolled back, and converts the press and each drag with the current scrollback, so a wheel mid-drag keeps the anchor on its text. `draw_screen` paints only the selection's visible rows.
  - Also: `term_select_output_and_the_wheel_clear_the_selection` became `term_select_output_clears_and_the_wheel_moves_the_selection`; `terminal_scroll_bar_extent_and_set_top` asserts the selection survives a scroll bar move
  - Deferred: keeping the selection across new output (vt100 doesn't report how many lines scrolled into the scrollback), auto-scrolling while a drag leaves the edge
  - Tests: `tasks::select` units (scrolled-back copy from any view, a selection taller than the screen, window boundaries with wrapped rows / the head's exclusive end / clamping); `term_select_tests.rs` `term_select_scrollback_shell` / `_task` (drag older lines, highlight, exact copy, wheel and Ctrl+Shift+Up / Down move it, scrolled out of view still copies, wheel mid-drag, click clears and copies nothing), and the mouse-reporting test scrolled back
  - Still to check by hand: in a real terminal, scroll back in a shell, drag, scroll further and back to live, then Cmd+C into another app; the same over a running task's panel; wide / CJK glyphs and soft-wrapped lines scrolled back
