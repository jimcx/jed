# jed

A terminal text editor that works like VS Code: the same layout, the same keybindings, mouse and keyboard both
fully supported. Not modal, nothing like vim. Written in Rust (ratatui, crossterm, tokio, ropey, tree-sitter).

```
┌──┬────────────┬──────────────────────┬──────────────────────┐
│A │ Side panel │ Editor group 1       │ Editor group 2       │
│c │ (explorer, │ [tab][tab][tab]      │ [tab][tab]           │
│t │  search,   │                      │                      │
│i │  git,      │                      │                      │
│v │  tools)    │                      │                      │
│i │            ├──────────────────────┴──────────────────────┤
│t │            │ Panel (problems / tasks)                    │
│y │            │                                             │
├──┴────────────┴─────────────────────────────────────────────┤
│ Status bar: branch · diagnostics · Ln/Col · lang · encoding │
└─────────────────────────────────────────────────────────────┘
```

## Features

- **Editing:** multi-cursor (Cmd+D, Cmd+Shift+L, add cursor above / below, Alt+Click, box select), auto-close and
  auto-indent, bracket matching, comment / move / duplicate lines, undo grouped as VS Code does; smooth on 1 MB files
  and tens of thousands of cursors
- **Workspace:** file explorer, fuzzy quick open (Cmd+P), command palette (Cmd+Shift+P), find / replace,
  workspace-wide search and replace with include / exclude filters, split editor groups, back / forward
- **Syntax highlighting** (tree-sitter): C, C++, Python, Rust, Markdown, YAML, JSON / JSONC, XML, TOML, shell,
  CMake, Makefile, Dockerfile
- **Language servers:** clangd, ty and rust-analyzer out of the box (others configurable) — diagnostics,
  completion, hover, go to definition, references, rename, formatting, code actions
- **Git** (view and stage): Source Control view, gutter markers, side-by-side diffs, stage / unstage hunks, inline
  blame, file and commit history
- **File history:** a snapshot on every save; compare with or restore an earlier version
- **Tasks:** build / test / run commands in throwaway terminal tabs (PTY, so colours and progress bars work)
- **Sessions:** open tabs, splits and layout restored per workspace; recent workspaces; crash backups of unsaved
  work; files changed on disk reload or warn
- **Tools view:** every language server and task with its log, like VS Code's Output panel
- OSC 52 clipboard over SSH

Platforms: macOS first, Linux too. Windows is not supported.

## Building

```sh
cargo build --release
./target/release/jed [<folder> | <file>[:line[:col]]]
```

The toolchain is pinned in `rust-toolchain.toml`; rustup picks it up.

- `jed <folder>` opens the folder as the workspace
- `jed <file>` walks up to the nearest `.workspace` file, else uses the file's folder
- `jed` alone opens the current directory

## Terminal setup

jed needs a terminal with the **kitty keyboard protocol** (Kitty, Ghostty, WezTerm, iTerm2, Alacritty); without it
keys such as Ctrl+Shift+X and Ctrl+H can't be told apart. On macOS most terminals also keep Cmd keys for themselves
(Cmd+W may even close the terminal window), so set yours up first:

**→ [docs/terminals/](docs/terminals/README.md)** — a tested config snippet per terminal, and every Cmd binding jed
uses.

## Configuration

| File | What it holds | Docs |
|---|---|---|
| `~/.config/jed/settings.toml` | Editor settings (tab width, whitespace, cursor, …). "Preferences: Open User Settings" creates it with every key at its default | [src/config/settings.rs](src/config/settings.rs) (table at the top) |
| `~/.config/jed/keybindings.toml` | Your own key bindings (`[[binding]]` with `key`, `command`, optional `context`); command names are shown in the palette | [PLAN.md — Keybindings](PLAN.md#keybindings-vs-code-defaults) |
| `~/.config/jed/tasks.toml` | Task templates for every workspace | [docs/tasks.md](docs/tasks.md) |
| `~/.config/jed/languages/<id>/` | Syntax plugins: more languages from tree-sitter grammars, no rebuild | [docs/syntax-plugins.md](docs/syntax-plugins.md) |
| `<project>/.workspace` | Per-project settings, language servers, tasks, excludes, file associations, history limits. Meant to be committed; jed asks before trusting the commands in it | [PLAN.md — Workspace](PLAN.md#workspace) |

Language servers:

- [docs/clangd.md](docs/clangd.md) — C and C++ with clangd
- [docs/ty.md](docs/ty.md) — Python with ty (including SCons projects)

## Keybindings

VS Code's defaults, Cmd on macOS and Ctrl on Linux. The full list is in
[PLAN.md — Keybindings](PLAN.md#keybindings-vs-code-defaults); the command palette shows each command's current key.

## Documentation map

For users:

- [docs/terminals/](docs/terminals/README.md) — terminal setup (start here)
- [docs/tasks.md](docs/tasks.md) — configuring tasks
- [docs/syntax-plugins.md](docs/syntax-plugins.md) — adding languages with tree-sitter grammars
- [docs/clangd.md](docs/clangd.md), [docs/ty.md](docs/ty.md) — language servers

For contributors:

- [PLAN.md](PLAN.md) — the design and scope: features, keybindings, architecture, milestones, the task list
- [docs/plan-done.md](docs/plan-done.md) — finished milestones and tasks in full: what was built, what was deferred
- [CLAUDE.md](CLAUDE.md) — the codebase summary: module map, data flow, invariants, how to add a command, tests,
  commit style
- [docs/dev/modules.md](docs/dev/modules.md) — every module and the data flow step by step
- [docs/dev/app.md](docs/dev/app.md), [docs/dev/ui.md](docs/dev/ui.md) — the app state and the drawing code in full
- [docs/dev/conventions.md](docs/dev/conventions.md) — rules for specific features
- [docs/dev/testing.md](docs/dev/testing.md) — test helpers per feature

jed is developed with Claude Code: the `CLAUDE.md` files and `docs/dev/` are written for it, and read as well by
people.

## Development

```sh
scripts/check            # fmt, clippy and tests, quiet output
scripts/check test foo   # one test or module
cargo test --release milestone -- --nocapture --test-threads=1   # performance budgets
```

All checks must pass before a commit. See [CLAUDE.md](CLAUDE.md#toolchain-and-checks).
