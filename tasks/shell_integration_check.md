---
id: shell_integration_check
name: Warn when the shell integration is missing or stale
summary: when a shell tab opens, warn the user if their shell's integration isn't installed or is an older version than the one jed embeds, pointing at the wizard that fixes it.
status: open
priority: 0
dependencies: []
---

  - Detection (`shell_integration.rs`, pure): `status(home, config_home, state_dir, shell_base) -> Option<Status>` — `None` for a shell without a snippet (`sh`, …; `init_for` is already `None` there); else read the init file and the snippet file on disk:
    - `NotInstalled` when the init file doesn't contain the snippet's `source` line (the path `init_for` would append — a user who hand-added it differently still reads `NotInstalled`)
    - `Stale` when the snippet file's text isn't the embedded `SNIPPETS` text (a missing file counts as stale: re-installing writes it)
    - `UpToDate` otherwise; a missing state dir is `NotInstalled` (nothing was ever written)
  - Checked once per app run, on the first shell tab (`new_shell`): the reads run in `spawn_blocking` (the UI never reads files), reported back as `AppEvent::ShellIntegrationStatus`, and a `bool` gates the one `Level::Warning` `notify` — "Shell integration for zsh isn't installed" / "… is out of date", each with "run 'Terminal: Install Shell Integration' to fix it"
  - The wizard already rewrites the snippets and appends the line, so the warning only needs to point at it; no new command or binding
  - Needs: `shell_integration` (`SNIPPETS`, `init_for`, `snippet_path`), the shell tab path (`app/shell.rs::new_shell`), `AppEvent` / `notify`
  - Done when: opening a shell tab with the integration never installed warns "not installed" once (a second tab doesn't repeat it); a stale snippet on disk warns "out of date"; the current snippet plus its `source` line in the init file warns nothing; `sh` (no snippet) warns nothing; and the `status` unit tests and the end-to-end tests (stale snippet on disk, missing source line, up-to-date) pass
