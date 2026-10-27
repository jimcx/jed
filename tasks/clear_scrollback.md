---
id: clear_scrollback
name: Clear terminal scrollback
summary: clear a shell / task tab's scrollback and screen, back to the last shell prompt mark when the integration is installed
status: in-progress
priority: 0
dependencies: []
---

See [Tasks](../PLAN.md#tasks-throwaway-terminals). "Terminal: Clear" (Cmd+K on macOS, Ctrl+K on Linux, VS Code's "Terminal: Clear") clears a shell or task tab's output — both the scrollback and the live screen, leaving a blank screen — without touching the running program

  - Needs: the shell / task tabs (`tasks::Run`), the `scrollback` setting (already user + `.workspace` config, read when a run is created — the amount kept stays that setting, unchanged)
  - A `Clear` on `ShellCommand` and `TaskCommand` (`Context::Shell` / `Context::Task`) so the key acts on the active shell or the shown task; `Run::clear` blanks the screen and empties the parser's kept lines, the cursor back at the top
  - Needs a vt100 fork: `vt100` 0.16 exposes no clear-scrollback API (`set_scrollback` moves the view offset only; the kept lines are a private `VecDeque`), so add `Screen::clear_scrollback` in a jed fork, as the crossterm `fwdback` fork is done (`[patch.crates-io]`)
  - The Cmd binding updates `docs/terminals/` (the README table and every snippet) and the cheat sheet
  - Done when: over a shell and a task tab with output scrolled off the top, Cmd+K / Ctrl+K blanks the screen and empties the scrollback so the wheel and Ctrl+Shift+Up have nothing to go back to, while the running program is untouched (the next prompt redraws); a `.workspace` `scrollback = 50` still caps a new run's scrollback at 50 lines
  - Prompt marks (`clear_scrollback`): with the shell integration installed, Clear goes back only to the last prompt-start mark (OSC `133;A`) instead of wiping everything, so the whole prompt (all its lines) stays at the top of the cleared screen. The snippets emit `133;A` at the start of the prompt (zsh `precmd` + `PS1` prefix, bash at the start of `PS1` inside `\[ \]`, fish `fish_prompt` wrapper) and keep the user's own `PROMPT_COMMAND` (bash: prepend, don't overwrite). The vt100 fork records the row of the last mark and keeps it correct as scrollback grows and is trimmed. No mark seen (no integration, an older snippet, a program that never prompts) → the full clear above
  - vt100: jed moves from `vt100 0.16.2` to the fork (based on 0.18.1, `~/vt100-rust`; `[patch.crates-io]` to a jimcx fork like crossterm's `fwdback`); the move's API changes (callbacks, `Parser`) are part of `-a`
  - Needs: the fork pushed to a remote the `[patch]` can point at (the user does this; until then `-a` may use a `path = "../vt100-rust"` patch locally and say so)
  - Subtasks (one agent and one commit each):
    - ✅ clear_scrollback-a: move jed to vt100 0.18 via the fork and add to it `Screen::clear_scrollback()` plus last-prompt-mark tracking (an OSC 133 `A` hook recording the mark's row, adjusted as scrollback grows / is trimmed, and a `clear_before_prompt()` that drops the lines above it); fix `TitleCallback` and anything else the upgrade breaks; no behaviour change in jed yet; fork's own tests for the new API
      - Done: jed is on vt100 0.18.1 through `[patch.crates-io] vt100 = { path = "/Users/jim/vt100-rust" }` (temporary, absolute path: commit and push the fork, then switch to a tag like crossterm's). The fork has a per-row `prompt_mark` flag: `Screen::mark_prompt()` (cursor row) and `Screen::clear_before_prompt() -> bool` (false and unchanged without a mark); `Screen::clear_scrollback()` already existed. The fork's `tests/prompt_mark.rs` has 14 tests
      - For -b / -c: `tasks::select::size(&Screen)` gives `(rows, cols)` as u16s (0.18's sizes are `NonZeroU16`); `Run::scrollbar_extent` reads `Screen::scrollback_len()`; tui-term's `vt100` feature is off (it pins vt100 0.16) so `ui/task.rs` has a `TermCell` newtype and `Shifted` is an owned snapshot of the visible cells. Clear while a program is on the alternate screen finds no mark (marks live on the active grid); `CSI 2K` / `CSI 2J` erase a row's mark but `CSI K` doesn't, so the -c snippets should emit `133;A` after any full-line erase their prompt does, and a mark sent from precmd is lost on zsh's Ctrl+L redraw; a cursor above the mark ends at row 0
      - Also: four stale `~` path-selection tests fixed
      - Still to check by hand: a task / shell tab's colours, bold, inverse and wide characters look as before (`TermCell` rendering)
    - clear_scrollback-b: `Run::clear` (full clear: screen blanked, kept lines emptied, cursor at the top, the program untouched), `ShellCommand::Clear` / `TaskCommand::Clear`, Cmd+K / Ctrl+K bindings, `docs/terminals/` table and every snippet, the cheat sheet; end-to-end tests over a shell and a task tab, and `scrollback = 50` still capping a new run
    - clear_scrollback-c: the integration emits `133;A` before the full prompt in the zsh / bash / fish snippets (bash keeps the user's `PROMPT_COMMAND`), `TitleCallback` records the mark in the parser, Clear goes back to the last mark when there is one; real-bash PTY test showing the whole prompt survives a clear; hand checks; marks the task ✅
