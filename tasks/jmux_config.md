---
id: jmux_config
name: "jmux: settings, keybindings, docs and release"
summary: "`settings.toml` and `keybindings.toml` for jmux, the status bar's session info, user docs, and `jmux` in the release pipeline"
status: in-progress
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
  - "Reload Settings" re-reads both `settings.toml` and `keybindings.toml` (one command, as the plan names one)
  - Done when: a `scrollback` and `shell` set in `settings.toml` apply to the next terminal; a bad key is reported and the rest applies; a user binding for "split right" works and a bad one is reported; the status bar shows the counts and updates; a release build produces both binaries
  - Subtasks (one agent and one commit each):
    - ✅ jmux_config-a: settings file — `jmux/src/settings.rs` reads `~/.config/jmux/settings.toml` (`shell`, `scrollback`, `scrollback_compression`, `command_history`, `side_bar_width`, `cursor_style`) leniently (a bad key reported with its line and skipped, the rest applies); the server reads it before its first shell (config dir passed in, never `XDG_CONFIG_HOME` in tests); a "Reload Settings" command re-reads it for the next terminal; errors shown as a notice. Tests: `scrollback` and `shell` reach the next terminal; a bad key is reported and the rest applies
      - `jmux/src/settings.rs`: `Settings` (defaults = the old constants; `scrollback()` gives a `jterm::run::Scrollback`), `CursorStyle`, `parse(text) -> (Settings, Vec<String>)` (each key on its own: `settings.toml: line N: …`, skipped; a syntax error gives defaults and one error), `load(path)` (blocking; missing file = defaults), `path()`; `COMMAND_HISTORY` moved here from `app`
      - `app::Config`: `settings`, `settings_path: Option<PathBuf>` (None: reload says so), `settings_errors` (the first load's, shown as the first notice, "(and N more)"); `run_server` loads before the runtime; `App::program()` is `shell` else `$SHELL` else `/bin/sh`
      - `Command::ReloadSettings` (Ctrl+Shift+R, `session.reloadSettings`): `App::reload_settings` (`app/commands.rs`) reads in `spawn_blocking` → `AppEvent::Settings(Settings, Vec<String>)` → `apply_settings` (next terminal's shell / scrollback, `History::set_cap`, side bar width; notice: first error or "Settings reloaded"). b extends this one job to re-read `keybindings.toml`
      - The client reads `cursor_style` from the same file when it attaches (next attach only)
      - Tests: `jmux/src/jmux_config_tests.rs` (`jmux_config_a_*`, a settings file in a temp dir), units in `settings.rs` and `history.rs`
    - ✅ jmux_config-b: keybindings and status bar — `keybindings.toml` over the defaults through `jtui`'s `Keymap::load`, re-read by "Reload Settings", errors as a notice; the status bar shows session name, terminal count, running-command count, the detach key, and notices. Tests: a user "split right" binding works, a bad one is reported; the counts show and update
      - `jmux/src/keymap.rs`: `FILE_NAME`, `load(path)` / `reload(path)` over jtui's `Keymap::load` / `reload` (jed's format; names are the commands' `name()`, e.g. `terminal.splitRight`); `Config` gains `keybindings_path` and `keymap: Option<JmuxKeymap>` (None: defaults; `Config` no longer derives `Debug` / `Clone`); the first load's problems join `settings_errors`
      - Reload: one `spawn_blocking` job reads both files; `AppEvent::Settings(Option<Settings>, Option<JmuxKeymap>, Vec<String>)`; an unparsable keybindings file keeps the old keymap, reported
      - Status bar (`ui.rs` `status_bar`): ` work  2 terminals · 1 running  <title> … Ctrl+Shift+D detach` (running only when > 0; narrow: the title gives way first); `App::counts()` shared with `App::info` (`jmux ls`)
      - Tests: `jmux_config_b_*`, `ui::status_bar_tests`, keymap unit tests
    - jmux_config-c: docs and release; task done — `jmux/README.md`, the root `README.md` line, jmux's Cmd bindings in `docs/terminals/` where they differ from jed's; the pipeline builds, packages and publishes `jmux` beside `jed`; `scripts/changelog` groups jmux's tasks; `/release` notes; remaining done-when gaps and hand checks; mark the task ✅
