---
id: scons_settings
name: SCons settings in `.workspace`
summary: a `[scons]` table for `sconstruct`, `scons_path` and `enabled`
status: done
priority: 0
dependencies: []
---

See [LSP](../PLAN.md#lsp). A `[scons]` table, read with the rest of `.workspace` and live on its reload (ty restarted when it changes, as for an `[lsp.*]` change):

  - `sconstruct = "build/SConstruct"` (relative to the root, or absolute): the build file when it isn't at the root. `scons::prepare` then treats the workspace as an SCons one from that path (today only the root's `SConstruct` / `Sconstruct` / `sconstruct` / `….py`), and `site_scons/` beside it goes on ty's `extra-paths` automatically (SCons puts it on `sys.path`), as does the root's `site_scons/` without the setting
  - `scons_path = "/usr/lib/python3/dist-packages"` (a folder containing the `SCons` package; `~` expanded): added to ty's `extra-paths` beside the stub folder (`scons::with_extra_path`), so `site_scons` helpers importing `SCons` resolve; a folder without `SCons/__init__.py` logs a warning and is still passed. jed's stub stays (it covers the run-time globals the real package can't)
  - `enabled = false` turns the stub off (for a workspace that isn't an SCons one but has a file named `SConstruct`)
  - No trust prompt: these name no commands (as `[settings]`); `docs/ty.md` section 3 updated to use them in place of the "open the subfolder" note; `docs/dev/conventions.md` if a rule comes of it
  - Tests: `prepare` / `with_extra_path` unit tests for each key (a subfolder `SConstruct`, its `site_scons`, `scons_path` merged after the project's own `extra-paths` and the stub's, `enabled = false` → nothing written); the `initialize` params through the fake server with a `.workspace` reload changing them; one real-ty test (skipped when absent, the `.venv/bin/ty` lookup) on a subfolder `SConstruct` with a `site_scons` helper importing `SCons.Script` and `scons_path` pointing at a scratch install

✅ Done:
  - `WorkspaceConfig.scons` (`Box<SconsConfig>`: unboxed it tipped `AppEvent::Config` over clippy's `large_enum_variant`), in `TEMPLATE` and `config::schema`; not in the trust hash. `scons::prepare(root, support_dir, &SconsConfig) -> Vec<PathBuf>`: stub dir, then `site_scons/` beside the build file, then `scons_path`; a root `__builtins__.pyi` or a failed stub write skips only the stub; no build file (or `enabled = false`) → nothing. `with_extra_path(init, &[PathBuf])` appends after the project's own paths
  - `Manager::scons` (last seen); `sync` restarts running / failed ty when `[scons]` changes (`restart`); stopped ones read it at their next launch. `app/lsp.rs start_servers` drops pending requests of servers no longer running after `sync`
  - Tests: unit tests in `lsp/scons.rs` and `workspace/config.rs`; `todo_tests.rs` `mod scons_settings` (fake-server `initialize` across four `.workspace` reloads, a disabled workspace, the real-ty test)
  - Deferred: a shared `~` expansion helper (`lsp::scons::expand_home` and `app/term_link.rs` each have their own; `paths::home()` falls back to `.`)
  - Still to check by hand: the real-ty test (`scripts/check test real_ty_resolves_scons`) on a machine with ty — it skipped here; and a real SCons project with `build/SConstruct` + `site_scons` in jed
