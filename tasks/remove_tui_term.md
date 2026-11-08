---
id: remove_tui_term
name: Remove tui-term
summary: draw a shell / task tab's vt100 screen straight into the buffer and drop the tui-term dependency
status: done
priority: -10
dependencies: []
---

`tui-term` adds little: with its `vt100` feature off (it pins vt100 0.16, which the 0.18 fork can't share) all that is left is its `PseudoTerminal` widget and `Screen` / `Cell` traits, and `ui/task.rs` already supplies the glue itself (`TermCell`, an owned `Shifted` snapshot of the visible cells). Draw the cells straight into the ratatui buffer instead.

  - `ui/task.rs`: replace `PseudoTerminal` / `Shifted` / `TermCell` with a loop over the visible cells that writes each one's symbol and style into the buffer (the same colour / bold / italic / underline / inverse / dim mapping `TermCell::apply` has, wide characters leaving their continuation cell alone), reading the `vt100::Screen` directly from row `top` — no per-frame copy; the terminal cursor and the mouse selection unchanged
  - Remove `tui-term` from `Cargo.toml` / `Cargo.lock`; fix the mentions in `theme.rs`, `milestone12_tests.rs`, `tasks/run.rs`, `ui/CLAUDE.md`, `docs/dev/ui.md` and `docs/dev/modules.md`
  - Keep the existing tests (`term_cells_reach_the_buffer_with_their_look`, `a_shifted_screen_draws_from_its_top_row`) pointed at the new drawing code; they pin the look
  - Done when: `tui-term` is gone from the dependency tree, shell and task tabs draw exactly as before, and the checks pass

✅ Done.
  - `ui/task.rs`: `draw_cells(buf, area, screen, top)` resets each cell of the area (tui-term's `Clear`), then writes the screen's cells from row `top` (symbol only when it has contents, so a wide character's continuation cell stays blank; the full style replaced: colours, bold / italic / underline / inverse / dim); `cursor_in(screen, top)` is the terminal cursor shifted by `top`. No per-frame copy. `draw_screen` then runs `Theme::terminal_cell`, the selection, search and cursor as before.
  - Also: the two look tests call `draw_cells` / `cursor_in`; the old window checks became a draw into an area bigger than the screen (cells past it come out cleared).
  - Deferred: folding the `terminal_cell` pass into `draw_cells` (one pass over the area instead of three; needs `&Theme` there and the look tests' expected colours changed); `Modifier::set` for the modifier loop.
  - Still to check by hand: a shell tab and a task tab look as before (colours, bold, wide characters, a scrolled-back screen, the cursor).
