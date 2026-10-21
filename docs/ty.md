# Configuring ty

jed starts [ty](https://docs.astral.sh/ty/) for Python files with no configuration: it runs `ty server` (the `ty` found on `PATH`) in the workspace root. Everything below is optional.

## 1. Installing ty

Anywhere on `PATH`:

```sh
uv tool install ty        # or: pipx install ty
```

Or in a project virtual environment, next to the packages ty should see:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install ty
```

jed doesn't look inside `.venv` by itself. Point the workspace at it in `.workspace` (a path with a `/` is relative to the workspace root):

```toml
# .workspace
[lsp.python]
command = ".venv/bin/ty"
args = ["server"]
```

A `.workspace` that names a command asks once whether to trust it, as for clangd (see [clangd.md](clangd.md#trust)). A server already running keeps its command: run **Tools: Restart Server** after changing it.

## 2. Project settings

ty reads its own `ty.toml`, or `[tool.ty]` in `pyproject.toml`, at the workspace root. jed doesn't read either. ty finds a `.venv` at the root by itself; set another one, and folders to import from, with:

```toml
# ty.toml
[environment]
python = "venvs/dev"
extra-paths = ["scripts", "vendor"]
```

The same settings can come from `.workspace`, as ty's `initializationOptions` (`configuration` is ty's inline settings, merged with `ty.toml`):

```toml
# .workspace
[lsp.python]
command = "ty"
args = ["server"]

[lsp.python.init_options.configuration.environment]
extra-paths = ["scripts", "vendor"]
```

## 3. SCons (`SConstruct`, `SConscript`)

jed treats `SConstruct`, `SConscript` and their spellings as Python, and opens them in ty as Python even though they have no `.py`. SCons runs these files with its globals (`Environment`, `Import`, `Glob`, `ARGUMENTS`, …) already defined, so ty on its own reports each of them as `Name 'Environment' used when not defined`.

**What jed does by itself:** in a workspace with an `SConstruct` at its root, jed writes a stub of SCons's globals to `~/.local/state/jed/lsp/scons/__builtins__.pyi` (the source is `src/lsp/scons_builtins.pyi`). It adds that folder to ty's `environment.extra-paths` when it starts any server whose command is named `ty`, including a `.venv/bin/ty`. ty treats the names in a `__builtins__.pyi` on its search path as builtins. Nothing is written into the workspace, and `SConstruct` gets completion, hover and go to definition with no false errors.

What the stub can't know is what your own build adds. Here is an example project:

```text
SConstruct
lib/SConscript
site_scons/helpers.py
```

```python
# SConstruct
import helpers                     # from site_scons/, which SCons puts on sys.path

env = Environment(CXXFLAGS=["-O2"])
sources = helpers.cpp_files(Glob("src/*.cpp"))
Default(env.Program("app", sources))
Export("env")
SConscript("lib/SConscript")
```

```python
# lib/SConscript
Import("env")                      # SCons defines `env` here at run time
env.Library("util", ["util.cpp"])
```

Here ty still reports two errors: `import helpers` can't be resolved, and `env` in `lib/SConscript` is undefined. To fix both:

1. **`site_scons/` on ty's search path.** Put this in `ty.toml` at the root:

   ```toml
   # ty.toml
   [environment]
   extra-paths = ["site_scons"]
   ```

   Or, to keep it out of the repository, put it in `.workspace` as in section 2:

   ```toml
   [lsp.python.init_options.configuration.environment]
   extra-paths = ["site_scons"]
   ```

2. **The names your `SConscript` files `Import()`.** Use your own `__builtins__.pyi` at the workspace root. When that file exists, jed doesn't add its stub, because ty would resolve only one `__builtins__` module. So start from jed's stub and add your names at the end:

   ```sh
   cp path/to/jed/src/lsp/scons_builtins.pyi __builtins__.pyi
   cat >> __builtins__.pyi <<'EOF'

   # What this project's SConscript files Import().
   env: Environment
   EOF
   ```

   Then, in `lib/SConscript`, hovering over `env.Library` shows `bound method Environment.Library(target: Any = None, source: Any = None, **kw: Any) -> list[Node]` and completion lists `env`'s methods.

**Tools: Restart Server** picks up a new `ty.toml` or `__builtins__.pyi`.

Notes:

- jed only adds its stub when the `SConstruct` is at the workspace root. For a build in a subfolder, open that folder, or use your own `__builtins__.pyi` as above.
- A root `__builtins__.pyi` applies to every Python file in the workspace, not only the SCons ones. That doesn't matter for a build-only folder. In a mixed project, ty won't flag a misspelled `env` in ordinary modules either.
- Outside jed, `ty check` checks extensionless files only when you name them: `ty check SConstruct lib/SConscript`. It uses the same `ty.toml` and `__builtins__.pyi`, but not jed's stub. That makes the root `__builtins__.pyi` the way to get the same result from the command line or CI.

## 4. Checking it works

- **Tools view** (Cmd+Shift+U on macOS, Ctrl+Shift+U on Linux): the `ty` row shows whether it's running and its error count. Enter on it opens its log.
- **Tools: Open Log** → `ty`: the command line, the `initializationOptions` sent (look for `extra-paths` and the stub folder in an SCons workspace) with **Tools: Toggle Protocol Trace** on, and ty's stderr.

| What you see | Why | Fix |
|---|---|---|
| `Cannot resolve imported module` for an installed package | ty is using another environment | `[environment] python = ".venv"` in `ty.toml` |
| A row `✗ ty` and "Can't start" in its log | Not on `PATH` | Install it, or `command = ".venv/bin/ty"` |
| `Name 'Environment' used when not defined` in `SConstruct` | The `SConstruct` isn't at the workspace root | Open its folder, or use your own `__builtins__.pyi` |
| `Name 'env' used when not defined` in an `SConscript` | The name comes from `Import()` | Add it to your own `__builtins__.pyi` (section 3) |
