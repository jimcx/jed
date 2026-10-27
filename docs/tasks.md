# Configuring tasks

A task is a command jed runs in a throwaway terminal: build, test, lint, run a script. Each run gets its own tab in the bottom panel, runs in a PTY (colours and progress bars work), shows how it ended (`Task 'build' exited with code 2`) and keeps its output until you close the tab.

Tasks come from two files with the same format:

| File | For | Notes |
|---|---|---|
| `.workspace` at the workspace root | this project; meant to be committed | runs only once the workspace is trusted |
| `~/.config/jed/tasks.toml` | you, in every workspace | read at startup; restart jed after editing |

When both define a task with the same name, the workspace's wins.

## Format

```toml
[tasks.build]
command = "cargo build"
cwd = "crates/app"               # optional: relative to the workspace root (default: the root)
env = { RUST_LOG = "debug" }     # optional: added to jed's environment
```

- `command` is run by your shell as `$SHELL -c "<command>"` (`/bin/sh` if `$SHELL` isn't set), so pipes, `&&`, `;`, redirects, globs and `$VARS` work.
- `cwd`: relative paths are under the workspace root, `~/…` is your home folder, absolute paths are used as is. A missing folder fails the run with a message.
- `env` values are literal: `PATH = "$HOME/bin:$PATH"` sets exactly that text. Expand in the command instead: `command = "PATH=$HOME/bin:$PATH make"`.
- jed sets `TERM=xterm-256color` and `COLORTERM=truecolor` unless `env` sets them.
- Names are any TOML key; quote ones with spaces or dots: `[tasks."test: one file"]`.

## Running

- **Tasks: Run Task** in the palette (Cmd+Shift+P / Ctrl+Shift+P): pick a task by name (fuzzy, also on the command). The list shows each task's command and where it's from.
- The tab has the keys while focused: what you type goes to the program (answer prompts, Ctrl+C to interrupt). Shift+PageUp / PageDown and Ctrl+Shift+Up / Down scroll back, Ctrl+Home / End jump, Cmd+K (Ctrl+K) clears the screen and the scrollback (the program keeps running; at a shell prompt with Terminal: Install Shell Integration, the whole prompt stays at the top), Esc goes back to the editor, Cmd+W (Ctrl+W) closes the tab.
- **Tasks: Kill Task** stops it (SIGHUP to everything it started, SIGKILL a second later if anything is left). Closing a running task's tab kills it too.
- Run the same task again for a new tab beside the old one.
- The **Tools** view (Cmd+Shift+U / Ctrl+Shift+U) lists runs with their state.

Run Task has no default key. Give it one in `~/.config/jed/keybindings.toml`:

```toml
[[binding]]
key = "f9"
command = "task.run"
```

## Examples

### Rust

```toml
[tasks.build]
command = "cargo build --color=always"

[tasks.test]
command = "cargo test --color=always"

[tasks.check]
command = "cargo fmt --check && cargo clippy --all-targets --color=always -- -D warnings"

[tasks.run]
command = "cargo run -- --log-level debug"
env = { RUST_BACKTRACE = "1" }

[tasks.release]
command = "cargo build --release --color=always 2>&1 | tee target/build.log"
```

`--color=always`: cargo usually detects the terminal itself; add it when a pipe (`| tee`) would turn colours off.

### C / C++ with CMake

```toml
[tasks.configure]
command = "cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Debug -DCMAKE_EXPORT_COMPILE_COMMANDS=ON"

[tasks.build]
command = "cmake --build build -j"

[tasks.test]
command = "ctest --output-on-failure"
cwd = "build"

[tasks.clean]
command = "cmake --build build --target clean"

[tasks."run app"]
command = "cmake --build build -j && ./build/app --verbose"
env = { ASAN_OPTIONS = "detect_leaks=1:abort_on_error=1" }
```

`CMAKE_EXPORT_COMPILE_COMMANDS=ON` writes the database clangd needs (see [clangd.md](clangd.md)).

### C / C++ with Make or SCons

```toml
[tasks.build]
command = "make -j$(nproc)"
env = { CC = "clang", CXX = "clang++" }

[tasks.compiledb]
command = "bear -- make -j$(nproc)"   # rebuild compile_commands.json for clangd

[tasks.scons]
command = "scons -j8 --debug=time"

[tasks."scons clean"]
command = "scons -c"
```

### Python

```toml
[tasks.test]
command = ".venv/bin/pytest -x --color=yes"

[tasks.lint]
command = "ruff check . && ruff format --check ."

[tasks.serve]
command = ".venv/bin/python -m http.server 8000"
cwd = "site"
```

A long-running task like `serve` keeps running until you kill it or close its tab.

### Personal tasks in `~/.config/jed/tasks.toml`

Available in every workspace, run in the workspace root unless `cwd` says otherwise:

```toml
[tasks."git log"]
command = "git log --oneline --graph --decorate --color=always -30"

[tasks.todo]
command = "rg --color=always -n 'TODO|FIXME'"

[tasks."disk usage"]
command = "du -sh * | sort -h"

[tasks.notes]
command = "ls -lt"
cwd = "~/notes"
```

## Trust

A `.workspace` with tasks (or language servers) asks once whether to trust it, listing every command with its `cwd` and `env`. In **Restricted** mode its tasks don't show in Run Task; your `tasks.toml` tasks still do. Changing any task asks again. **Workspaces: Manage Workspace Trust** changes your answer.

## Errors

- `.workspace` is strict: an unknown key (`comand = …`) or a wrong type rejects the whole file and jed keeps its last good version, with a message.
- `tasks.toml` is lenient: a bad task (unknown key, wrong type, empty `command`) is reported at startup and skipped, the others still load; a TOML syntax error loses the file, reported with its line.
