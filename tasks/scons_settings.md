---
id: scons_settings
name: SCons settings in `.workspace`
summary: a `[scons]` table for `sconstruct`, `scons_path` and `enabled`
status: open
priority: 0
dependencies: []
---

See [LSP](../PLAN.md#lsp). A `[scons]` table, read with the rest of `.workspace` and live on its reload (ty restarted when it changes, as for an `[lsp.*]` change):

  - `sconstruct = "build/SConstruct"` (relative to the root, or absolute): the build file when it isn't at the root. `scons::prepare` then treats the workspace as an SCons one from that path (today only the root's `SConstruct` / `Sconstruct` / `sconstruct` / `….py`), and `site_scons/` beside it goes on ty's `extra-paths` automatically (SCons puts it on `sys.path`), as does the root's `site_scons/` without the setting
  - `scons_path = "/usr/lib/python3/dist-packages"` (a folder containing the `SCons` package; `~` expanded): added to ty's `extra-paths` beside the stub folder (`scons::with_extra_path`), so `site_scons` helpers importing `SCons` resolve; a folder without `SCons/__init__.py` logs a warning and is still passed. jed's stub stays (it covers the run-time globals the real package can't)
  - `enabled = false` turns the stub off (for a workspace that isn't an SCons one but has a file named `SConstruct`)
  - No trust prompt: these name no commands (as `[settings]`); `docs/ty.md` section 3 updated to use them in place of the "open the subfolder" note; `docs/dev/conventions.md` if a rule comes of it
  - Tests: `prepare` / `with_extra_path` unit tests for each key (a subfolder `SConstruct`, its `site_scons`, `scons_path` merged after the project's own `extra-paths` and the stub's, `enabled = false` → nothing written); the `initialize` params through the fake server with a `.workspace` reload changing them; one real-ty test (skipped when absent, the `.venv/bin/ty` lookup) on a subfolder `SConstruct` with a `site_scons` helper importing `SCons.Script` and `scons_path` pointing at a scratch install
