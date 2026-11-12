---
id: jterm
name: "jterm: shared terminal engine"
summary: jed's terminal (PTY run, vt100 screen and compressed scrollback, command records, selection, search, key and mouse encoding, shell tab model, shell integration, screen drawing) moves to `crates/jterm`, jed re-exporting it
status: open
priority: 0
dependencies: [jtui]
---

See [Shared crates and jmux](../PLAN.md#shared-crates-and-jmux), [jmux's plan](../jmux/PLAN.md#crates), `docs/dev/crates.md`.

  - Needs: `jtui` (scroll bar model, `TextField`, `Theme`); `tasks/{run,select,search,keys,mouse}.rs`, `tasks/templates.rs` (`Spec` only), `shell.rs`, `shell_integration.rs`, `ui/task.rs` (`draw_cells`, `cursor_in`, `draw_screen`, `draw_search`, `render_shell`)
  - Moves as in `jtui`: code and unit tests go to `crates/jterm`, the jed modules become re-exports (`tasks::run`, `tasks::select`, … keep their paths), jed's behaviour and end-to-end tests (`shell_tests`, `term_*_tests`, `milestone12_tests`) do not change and are the safety net
  - Stays in jed: `Tasks` (the run list), task templates, environments and `tasks.toml` (`tasks/mod.rs`, `templates.rs` minus `Spec`), `app/tasks.rs`, `app/shell.rs`, `ui::task::render` (the panel's task tab with its info bar)
  - No application names: `Run::start` takes the env variable names it sets and strips (`JED_IPC` / `JED_TERMINAL` for jed) in a small `EnvNames` value; `shell::spec` likewise; the integration snippets take the guard variable name and the state dir, so jed's written files are byte-identical to today's (a test compares them)
  - Unit tests that reached into jed (`crate::milestone12_tests::group_alive`, `crate::workspace::normalize`) get a local helper in `jterm`
  - Two additions jmux needs, with jed's values unchanged: the command ring's size is a `Run::start` argument (jed passes `COMMAND_RING` 50), and a `CommandRecord` also carries its wall-clock start time
  - Done when: `cargo test -p jterm` passes alone, real-shell tests included; `grep -rn 'JED_\|"jed"' crates/jterm/src` finds nothing outside tests; jed's `scripts/check` passes with no end-to-end test changed; the release `milestone` budget run is within noise of before; by hand, a jed shell tab still shows titles, the command list over the Claude integration (`get_recent_commands`), selection, search and the scroll bar
  - Subtasks (one agent and one commit each):
    - jterm-a: the engine — `run`, `select`, `keys`, `mouse` and `Spec`, with `EnvNames` and the ring-size argument and wall-clock time; jed's `tasks/mod.rs` re-exports
    - jterm-b: `search` (over `jtui`'s `TextField`), the shell tab model (`ShellTab`, `title`, `base`, `spec`, the glyphs) and `shell_integration` with its names passed in
    - jterm-c: drawing — the screen, cursor, selection and search-match drawing from `ui/task.rs` into `jterm::draw` over `jtui::Theme`; jed's `ui/task.rs` keeps the task tab's frame and calls it. Also the done-when checks, the docs entries, and marks the task ✅
