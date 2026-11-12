---
id: jmux_config
name: "jmux: settings, keybindings, docs and release"
summary: "`settings.toml` and `keybindings.toml` for jmux, the status bar's session info, user docs, and `jmux` in the release pipeline"
status: open
priority: 0
dependencies: [jmux_layout]
---

See [jmux's plan](../jmux/PLAN.md): [Settings, keys and theme](../jmux/PLAN.md#settings-keys-and-theme).

  - Needs: `jmux_layout`; `jtui`'s `Keymap` file reader and `paths`; jed's `config/settings.rs` (lenient parsing: a bad key reported and skipped) and `docs/terminals/` as models; `.github/workflows/build.yml` and the `/release` command
  - Settings as the plan lists, read before the server starts its first shell and re-read on a "Reload Settings" command; errors shown as a notice with the line
  - User keybindings over the defaults, same file format and error reporting as jed's
  - Status bar: session name, terminal count, running-command count, the detach key; notices show there
  - Docs: `jmux/README.md` (install, sessions, keys, settings, shell integration), a line in the root `README.md`; the Cmd-key terminal snippets in `docs/terminals/` gain jmux's bindings where they differ from jed's
  - Release: the pipeline builds and publishes `jmux` beside `jed` under the same `v<N>` tag; `scripts/changelog` groups jmux's tasks
  - Done when: a `scrollback` and `shell` set in `settings.toml` apply to the next terminal; a bad key is reported and the rest applies; a user binding for "split right" works and a bad one is reported; the status bar shows the counts and updates; a release build produces both binaries
