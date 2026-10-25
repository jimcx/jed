# jed

A VS Code-style terminal text editor in Rust (ratatui + crossterm + tokio, ropey 2 beta for text).
**PLAN.md is the source of truth for design and scope** — read the section you need instead of
guessing: Features (one subsection per area), [Keybindings](PLAN.md#keybindings-vs-code-defaults),
[Architecture](PLAN.md#architecture), [Text positions](PLAN.md#text-positions),
[Async and blocking work](PLAN.md#async-and-blocking-work), [Logging](PLAN.md#logging),
[Tasks](PLAN.md#tasks) (bigger ones have "Needs", "Deferred" and a "Done when" to test against).
Milestones 1–13 are done and keep their numbers. Everything after them is a task in PLAN.md's [Tasks](PLAN.md#tasks) list (no order; ✅ = done), built via `/task <name>`: a single task is one commit; a bigger one has a short id and subtasks (`tools-a`, `tools-b`, …), one commit each (a task too big for one commit gets split into subtasks or smaller tasks). OpenCode runs the same workflow from [.opencode/commands/task.md](.opencode/commands/task.md) (agents and instruction loading in [opencode.jsonc](opencode.jsonc)); keep it in step with `.claude/commands/task.md`. Finished milestones and tasks keep one line in PLAN.md; their full entries (what was built, "Deferred", hand checks, notes for later work) are in [docs/plan-done.md](docs/plan-done.md) — grep it when something refers back to one.

**This file is the summary; details are read on demand**:
- [docs/dev/modules.md](docs/dev/modules.md) — the module map (every `src/` module, one line each, then in full) and the data flow step by step. Read the entries for what you touch.
- [docs/dev/conventions.md](docs/dev/conventions.md) — rules for one area: tabs, keymaps, settings, language server requests, edits from background results, scroll bars, adding a command end to end, and each feature's conventions (files changed on disk, log tabs, file history, sessions, crash backups, back / forward, recent workspaces, tool logs).
- [docs/dev/testing.md](docs/dev/testing.md) — driving the app headlessly, and test helpers per feature.
- [docs/dev/app.md](docs/dev/app.md) / [docs/dev/ui.md](docs/dev/ui.md) — every `src/app/` submodule and `src/ui/` widget in full.
- The nested `CLAUDE.md` files in `src/app`, `src/buffer`, `src/lsp`, `src/syntax`, `src/ui`, `docs/terminals`.

## Toolchain and checks

`rust-toolchain.toml` pins 1.99. On Linux plain `cargo` works (the rustup proxy is on PATH); on macOS cargo isn't
on PATH, so prefix every command with `export PATH="$HOME/.rustup/toolchains/1.99-aarch64-apple-darwin/bin:$PATH"`.

```sh
cargo fmt --check                            # rustfmt.toml: max_width = 120
cargo clippy --all-targets -- -D warnings
cargo test                                   # ~150 tests, ~5 s (search's random reference test is most of it)
cargo test --release milestone -- --nocapture --test-threads=1   # M1 and M6 (1 MB Cmd+Shift+L + typing; line/cursor commands at 61k selections; find + replace all) budgets: release only (M6 has coarse debug bounds); serial, since parallel tests nearly double the timings on a slow machine
cargo test --release search -- --nocapture      # so are the 1 MB find-all and 10 MB workspace-search budgets
```

**Agents run the checks through `scripts/check`** (sets the macOS PATH itself; prints only fmt's diff, clippy's warnings, failing tests and the result lines; tests under `scripts/timeout 300`): `scripts/check` runs all three, `scripts/check test <filter>` one test or module (`scripts/check fmt` / `clippy` alone). Don't run plain `cargo test` into an agent's context: thousands of lines of build and progress output. For a failure's full output, re-run just that test. The release budget runs above are the exception (their `--nocapture` timings are the point); pipe them through `tail -40`.

All three must pass before a commit. Cap test runs with `scripts/timeout 300 cargo test …` (macOS has no `timeout`; this kills the whole process group, so cargo's test binary can't outlive it). Edition 2024 (let-chains are used freely).

## Where things are

`main.rs` parses args and loads config before raw mode; `app/` holds `App` (all state, the event loop, `execute(Command)`); `ui/` draws it (`ui::render(f, &mut App)`, which also writes back render-time state such as view sizes and the mouse `HitMap`). Feature "model" modules in `src/` (`find.rs`, `diff_view.rs`, `scm.rs`, …) hold no app state and draw nothing: `app/<area>.rs` drives them, `ui/<area>.rs` draws them. Positions and the buffer live in `buffer/`, commands in `command.rs`, keys in `config/keymap.rs`, settings in `config/settings.rs` + `edit::Settings`. Find a module in [docs/dev/modules.md](docs/dev/modules.md#module-map-src); the event loop's steps are in [its data flow](docs/dev/modules.md#data-flow).
`vendor/crossterm/` is a fork of crossterm 0.29 (patched via `[patch.crates-io]`) adding the mouse back / forward buttons; re-vendor it and re-apply the patch on a crossterm upgrade.

## Invariants

- **Positions:** `ByteIdx` (UTF-8 byte offset, char boundary; cursors on grapheme boundaries) is the canonical unit; `DisplayCol` for screen columns. Line/column/grapheme/word conversions live only in `buffer::pos` (`line_of`, `line_start`, `line_end`, `display_col`, `byte_at_col`, `next_grapheme`, `word_left`, …). Other modules hold `&Rope` but only call `len()`, `slice(..)`, `to_string()`, `clone()`, `floor_char_boundary()` on it. No raw `usize` byte offsets across modules (line indices are plain 0-based `usize`).
- **Text only changes through `Buffer::edit` / `undo` / `redo`** (the sole `rope.insert/remove` is in `ChangeSet::apply`, called only from `Buffer::apply`, which also feeds the parse tree and the language servers' copies). Bypassing it breaks undo, other views' cursors, the parse tree and the servers' copies. Edits from background results go through it too ([conventions](docs/dev/conventions.md)).
- **Selection list everywhere:** every command works on `view.selections` (a `Selections`, never a single cursor). Build new lists with `Selections::new`/`single`/`transform` so they stay sorted and merged.
- **Undo grouping:** `EditKind::Insert`/`Delete` join consecutive same-kind edits (<1 s, selections continuous); `EditKind::Other` (paste, cut, comment line, wrapping a selection) is always its own step; an auto-close seals before itself, so it starts a step that later typing joins (VS Code). Non-edit actions (moves, clicks, tab/group switches, select-all) call `buf.seal_undo()`.
- **Read-only buffers:** `App::active_writable()` gates edits, cut and paste; copy still works.
- **Tabs:** a tab is a text editor, a diff view, a hex view or a shell (`group::Tab`). `active_buffer()` / `active_mut()` are `None` on a diff, hex or shell tab, so editor features need no checks of their own there; the rest is in [conventions](docs/dev/conventions.md#tabs).
- **Keymaps are data:** commands never look at keys; default bindings live in `keymap::defaults`, per platform. `Command` must stay `Copy + Hash`. Details: [conventions](docs/dev/conventions.md#keymaps).
- **Settings:** code measuring or indenting a buffer's text reads `tab_width` / `insert_spaces` from `settings.for_buffer(buf)`, never `app.settings` directly. Adding a setting: [conventions](docs/dev/conventions.md#settings).
- **Colours:** no `Color::`/`Modifier::` outside `theme.rs`; widgets call `theme.get("key")` (a missing key logs `missing theme key`). Add new keys to `Theme::dark_plus`.
- **Never block the UI thread:** file reads, saves, listings, file ops, config reloads, workspace search, first parses, plugin grammar loads and every `git2` call go through `tokio::task::spawn_blocking` and report back with an `AppEvent`; language servers are async tokio tasks reporting via `AppEvent::Lsp`; task runs spawn their PTY process in `spawn_blocking` and report via `AppEvent::Task` (output throttled to one event per frame). Blocking functions are documented `/// Blocking: call from spawn_blocking`. `App::new` must not spawn anything (it can run before the runtime is entered); `App::start` does.
- **Many selections:** Cmd+Shift+L can make 60k+ selections. Anything per edit or per frame must stay O(selections · log n): map positions with `ChangeSet::mapper()` (not `map` in a loop, and in order), no rope lookup or allocation per selection (build changes with `ChangeSet::push`, borrow the text), draw only `Selections::overlapping` the viewport, and don't scan the selection list per cell. A rope line lookup costs a few hundred ns: commands that need each selection's line visit them through `pos::LineWalker` in order instead of `line_of`/`display_col` per selection. `ChangeSet::apply` rebuilds the rope in one pass from 256 changes up.
- **Logging:** `tracing` only — never print while the UI is up. Targets are module paths (`jed::app`, …) except `input`, `panic`, `lsp::wire`, `lsp::stderr`. User-visible messages go through `App::notify(Level, text)`, which also logs. `JED_LOG=input=trace` / `--log-level` set the filter ([targets and spans](docs/dev/conventions.md#logging)). Slow-work warnings: frames > 16 ms (`SLOW_FRAME`), saves and parses > 500 ms (`SLOW_TASK`).
- **Feature conventions:** before touching files changed on disk, log / commit tabs, file history, sessions, crash backups, back / forward, recent workspaces, tool logs, LSP requests or scroll bars, read its section in [docs/dev/conventions.md](docs/dev/conventions.md). Adding a command: [its checklist](docs/dev/conventions.md#adding-a-command-end-to-end).

## Tests

- Unit tests sit in `#[cfg(test)] mod tests` at the bottom of each module.
- End-to-end tests: `src/milestone_tests.rs` (M1), `src/milestone2_tests.rs` … `milestone8_tests.rs`, named `milestone_N_*`; a task with subtasks adds `src/<id>_tests.rs` (tests named `<id>_*`, a `#[cfg(test)] mod` line in `main.rs`; a release budget test keeps `milestone` in its name, e.g. `milestone_soft_wrap_budget_*`); single tasks' go in `src/todo_tests.rs`, one `mod` per task. Each file opens with a `//!` doc saying what "done when" it covers.
- They drive the app headlessly through real code paths (`App::new` + `test_start`, `press`, `type_text`, `mouse`, `settle`, `TestBackend` drawing): read [docs/dev/testing.md](docs/dev/testing.md#driving-the-app-headlessly) first, then its section for the feature. Never set `XDG_CONFIG_HOME` (tests run in parallel); `settle` results are always `assert!`ed.
- **`terminal_docs_list_every_cmd_binding`** (`config/keymap.rs`) fails if the macOS Cmd bindings and the table in `docs/terminals/README.md` differ. Any new/changed/removed Cmd binding means updating that table *and* every terminal snippet (see `docs/terminals/CLAUDE.md`).

## Commits

Look at `git log`. Milestone work: subject `<Milestone name> (<task id>): <short summary>` (one task, e.g. `Tools view and logs (tools-a): …`; the numbered milestones used `Milestone N (Na): …`), blank line, `- ` bullets of user-visible behaviour and notable internals (wrapped ~75 cols). Smaller changes: short imperative subject (`Mark milestone 4 done in the plan`, `Fix crash opening a file inside its workspace; add .workspace`) with a prose body explaining the why when non-obvious. Plan-only edits go in their own commit. End with the `Co-Authored-By:` trailer.

## Keeping these files current

When a change adds a module, a convention, a test helper or a new way of doing background work, update the docs in the same change. Keep them short and only state what the code actually does. **Budget:** this root file stays a summary (~15 KB): a change adds at most a line here (a cross-cutting rule); a new module's line goes in [docs/dev/modules.md](docs/dev/modules.md), other detail in [docs/dev/](docs/dev/) or the nested `CLAUDE.md` of the directory it's about. Don't repeat what a module's `//!` doc already says — point at it. When adding detail, cut what has gone stale rather than only appending.
