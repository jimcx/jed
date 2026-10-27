---
id: clear_scrollback
name: Clear terminal scrollback
summary: "\"Terminal: Clear\" (Cmd+K on macOS, Ctrl+K on Linux, VS Code's \"Terminal: Clear\") clears a shell or task tab's output — both the scrollback and the live screen, leaving a blank screen — without touching the running program"
status: open
priority: 0
dependencies: []
---

  - Needs: the shell / task tabs (`tasks::Run`), the `scrollback` setting (already user + `.workspace` config, read when a run is created — the amount kept stays that setting, unchanged)
  - A `Clear` on `ShellCommand` and `TaskCommand` (`Context::Shell` / `Context::Task`) so the key acts on the active shell or the shown task; `Run::clear` blanks the screen and empties the parser's kept lines, the cursor back at the top
  - Needs a vt100 fork: `vt100` 0.16 exposes no clear-scrollback API (`set_scrollback` moves the view offset only; the kept lines are a private `VecDeque`), so add `Screen::clear_scrollback` in a jed fork, as the crossterm `fwdback` fork is done (`[patch.crates-io]`)
  - The Cmd binding updates `docs/terminals/` (the README table and every snippet) and the cheat sheet
  - Done when: over a shell and a task tab with output scrolled off the top, Cmd+K / Ctrl+K blanks the screen and empties the scrollback so the wheel and Ctrl+Shift+Up have nothing to go back to, while the running program is untouched (the next prompt redraws); a `.workspace` `scrollback = 50` still caps a new run's scrollback at 50 lines
