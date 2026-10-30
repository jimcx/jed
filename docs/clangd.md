# Configuring clangd

jed starts `clangd` for C, C++, Objective-C and Objective-C++ files with no configuration: it runs the `clangd` found on `PATH`, with no arguments, in the workspace root. Everything below is optional.

There are three places to configure it, from most to least common:

1. **Tell clangd how your code is compiled** — `compile_commands.json` / `compile_flags.txt` / `.clangd`. These are clangd's own files; jed doesn't read them.
2. **Change the command jed runs** — `[lsp.c]` / `[lsp.cpp]` in `.workspace` (another binary, extra arguments, `initializationOptions`).
3. **Check what's happening** — the Tools view and the server's log.

## 1. Compile commands

clangd needs each file's compiler flags (include paths, defines, `-std=`), or it reports false errors such as `'foo.h' file not found`. It looks for them in the folders above the file being opened.

**CMake** — export the database and link it into the root:

```sh
cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
ln -s build/compile_commands.json .
```

Or put this in `CMakeLists.txt` so every build writes it:

```cmake
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
```

**Make, SCons, anything else** — record a build with [Bear](https://github.com/rizsotto/Bear):

```sh
bear -- make -j8
bear -- scons -j8
```

**A small project with the same flags everywhere** — a `compile_flags.txt` at the root, one flag per line:

```text
-std=c++20
-Iinclude
-Ithird_party/fmt/include
-DDEBUG=1
-Wall
```

**Tweaks on top of the database** — a `.clangd` file at the root (YAML, clangd's [config file](https://clangd.llvm.org/config)):

```yaml
CompileFlags:
  CompilationDatabase: build/debug   # where compile_commands.json is, if not at the root
  Add: [-Wno-unused-parameter]
  Remove: [-mno-direct-extern-access, -fconserve-stack]  # gcc-only flags clang rejects
Diagnostics:
  UnusedIncludes: Strict
  ClangTidy:
    Add: [modernize-*, bugprone-*]
    Remove: [modernize-use-trailing-return-type]
---
If:
  PathMatch: tests/.*
CompileFlags:
  Add: [-DTESTING]
```

After changing any of these, run **Tools: Restart Server** (palette) so clangd re-reads them.

## 2. The command jed runs: `.workspace`

A `[lsp.<language id>]` table replaces the built-in server for that language. C, C++, Objective-C and Objective-C++ are four languages (`c`, `cpp`, `objective-c`, `objective-cpp`), so set each you use; when their tables are equal, they share one clangd.

```toml
# .workspace
[lsp.c]
command = "clangd"
args = [
  "--compile-commands-dir=build",  # compile_commands.json isn't at the root
  "--background-index",            # index the whole project for references / rename
  "--clang-tidy",                  # clang-tidy checks as diagnostics
  "--header-insertion=never",      # completion doesn't add #includes
  "--completion-style=detailed",   # one completion item per overload
  "-j=8",
]

[lsp.cpp]
command = "clangd"
args = [
  "--compile-commands-dir=build",
  "--background-index",
  "--clang-tidy",
  "--header-insertion=never",
  "--completion-style=detailed",
  "-j=8",
]
```

### A specific clangd

`command` is a bare name (looked up on `PATH`), an absolute path, or a path with a `/` relative to the workspace root:

```toml
[lsp.cpp]
command = "/usr/lib/llvm-19/bin/clangd"
```

```toml
[lsp.cpp]
command = "tools/llvm/bin/clangd"   # checked into the repo, relative to the root
```

### Cross-compilers

clangd only trusts the system compilers. To use a cross toolchain's own headers (`arm-none-eabi-gcc`, …), let it ask that compiler with `--query-driver`:

```toml
[lsp.c]
command = "clangd"
args = ["--query-driver=/opt/gcc-arm/bin/arm-none-eabi-*", "--compile-commands-dir=build"]

[lsp.cpp]
command = "clangd"
args = ["--query-driver=/opt/gcc-arm/bin/arm-none-eabi-*", "--compile-commands-dir=build"]
```

### `initializationOptions`

`init_options` is sent as the `initialize` request's `initializationOptions` (TOML, converted to JSON). clangd reads a few [extensions](https://clangd.llvm.org/extensions#initialization-options) there, e.g. flags for files missing from the database:

```toml
[lsp.cpp]
command = "clangd"
init_options = { fallbackFlags = ["-std=c++20", "-Iinclude"] }

# or, more readable for bigger tables:
[lsp.c]
command = "clangd"

[lsp.c.init_options]
fallbackFlags = ["-std=c17", "-Iinclude"]
compilationDatabasePath = "build"
```

### Trust

A `.workspace` that names commands asks once whether to trust it (the dialog lists every command). Until you choose **Trust**, jed runs neither the workspace's server nor the built-in one for that language; in **Restricted** mode it runs the built-in `clangd`. Changing any `[lsp.*]` or `[tasks.*]` entry asks again. Change your answer later with **Workspaces: Manage Workspace Trust**.

`.workspace` is reloaded when it changes, but a server already running keeps its command: run **Tools: Restart Server** to pick up new arguments.

## 3. Checking it works

- **Tools view** (Cmd+Shift+U on macOS, Ctrl+Shift+U on Linux): one row per server — `○` starting, `●` running, `✗` failed, `–` exited — with its error count. Enter on a row opens its log.
- **Tools: Open Log** → `clangd`: the command line it was started with, its stderr (clangd logs which compile command it used for each file: look for `ASTWorker building file … version … with command`), and every message it sent.
- **Tools: Toggle Protocol Trace**: every JSON-RPC message in the log too.
- **Tools: Restart Server**: stops it and starts a new one; also the way back after it failed or crashed.

Logs are also written to `~/.local/state/jed/logs/tools/clangd.log`.

Common problems:

| What you see | Why | Fix |
|---|---|---|
| `'stdio.h' file not found`, every file | No compile commands; or a cross toolchain clangd won't query | Section 1; `--query-driver` |
| A row `✗ clangd` and "Can't start" in its log | Not on `PATH` | Install it, or `command = "/full/path/clangd"` |
| Changes to `.clangd` / `compile_commands.json` ignored | clangd caches them | **Tools: Restart Server** |
| Nothing at all in a C++ file | `.workspace` has `[lsp.cpp]` but trust isn't decided | Answer the trust dialog |
| A C project's headers parsed as C++ | `.h` files are C++ by default | `[files] associations = { "*.h" = "c" }` in `.workspace` |
