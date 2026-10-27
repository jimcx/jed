---
id: term_select_scrollback
name: Mouse selection in the terminal scrollback
summary: select scrolled-back terminal output with the mouse
status: open
priority: 0
dependencies: [term_select]
---

Extend `term_select`'s left-drag to the scrolled-back view, so the older output the wheel and Ctrl+Shift+Up / Down reveal can be selected too — drawn highlighted and copied by Cmd/Ctrl+C, exactly as on the live screen. A program that has asked for the mouse keeps every event, unchanged.

  - Needs: `term_select` (`tasks/select.rs`, `Drag::Terminal` / `terminal_select` / `screen_cell` in `app/mouse.rs`), `tasks::run`'s `scroll` / `scroll_to`, the scroll-back bindings and the wheel
  - A drag over a shell or task tab seeds / extends the selection whether the view is live or scrolled back: the pointer maps to a screen cell through `screen_cell` / `select::top_row` (the viewport coordinates `vt100` reads), `draw_screen` paints the selection's rows, and `copy_selection` reads them through `contents_between` — the same path as a live-screen drag. Scrolled back, the wheel and the scroll keys still scroll the scrollback rather than forward to the program, and a scroll clears the selection (the rows move) as on the live screen, so a selection is made on whatever the view shows and then copied.
  - Scrolling the terminal should not effect the selected text. the selection moves with the text.
  - Done when: over a shell and a task tab scrolled back with output above the screen, a left-drag highlights the older lines and Cmd+C / Ctrl+C copies exactly them (trailing spaces trimmed, wrapped rows joined); the wheel and Ctrl+Shift+Up / Down still scroll the scrollback and clear the selection; a click still clears it and copies nothing; a mouse-reporting program (vim / tmux / htop) is unchanged; a live-screen drag still selects
