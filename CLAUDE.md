# red

A VS Code-style terminal text editor in Rust (ratatui + crossterm + tokio, ropey 2 beta for text).
**PLAN.md is the source of truth for design and scope** — read the section you need instead of
guessing: Features (one subsection per area), [Keybindings](PLAN.md#keybindings-vs-code-defaults),
[Architecture](PLAN.md#architecture), [Text positions](PLAN.md#text-positions),
[Async and blocking work](PLAN.md#async-and-blocking-work), [Logging](PLAN.md#logging),
[Milestones](PLAN.md#milestones) (each has "Needs", "Deferred" and a "Done when" to test against).
Milestones 1–5 are done; work happens one milestone at a time, split into tasks (6a, 6b, …), one commit per task.

## Toolchain and checks

`rust-toolchain.toml` pins 1.99. On Linux plain `cargo` works (the rustup proxy is on PATH); on macOS cargo isn't
on PATH, so prefix every command with `export PATH="$HOME/.rustup/toolchains/1.99-aarch64-apple-darwin/bin:$PATH"`.

```sh
cargo fmt --check                            # rustfmt.toml: max_width = 120
cargo clippy --all-targets -- -D warnings
cargo test                                   # ~150 tests, ~5 s (search's random reference test is most of it)
cargo test --release milestone -- --nocapture   # M1 and M6 (1 MB Cmd+Shift+L + typing; line/cursor commands at 61k selections) budgets: release only (M6 has coarse debug bounds)
cargo test --release search -- --nocapture      # so is the 1 MB find-all budget
```

All three must pass before a commit. Cap test runs with `scripts/timeout 300 cargo test …` (macOS has no `timeout`; this kills the whole process group, so cargo's test binary can't outlive it). Edition 2024 (let-chains are used freely).

## Module map (`src/`)

- `main.rs` — arg parsing (`red [<folder> | <file>[:line[:col]]]`, `--log-level`), loads the first file *before* raw mode, builds the tokio runtime, runs `App`
- `app/` — `App`: owns all state, the event loop, `execute(Command)`, prompts, mouse, background-result handling (see `src/app/CLAUDE.md`)
- `command.rs` — `Command` / `Motion` / `ExplorerCommand` / `PaletteCommand` enums; `Command::all()` (what the palette lists and keybindings can name), `name()` (stable dotted names like `edit.copy`), `from_name`, `title()` (palette text, `Category: Action`)
- `config/keymap.rs` — `Keymap` (data: `(KeyCombo, Option<Context>) -> Command`), `Context` (`Editor`, `Explorer`, `Palette`), `Platform`, `default_for(platform)`, `load(path, platform)` (defaults + `keybindings.toml`), `binding_for(cmd)` (the key the palette shows); `KeyCombo::parse` / `spec` (config spelling) / `label` (`Cmd+Shift+P`)
- `edit.rs` — movement and editing at every selection (`edit::run`, `insert_char`, `paste`, `copy_text`, `cut`), `Settings` (tab width, spaces). `paste(buf, view, text, whole_line, pieces)` spreads one part per cursor (VS Code's `multiCursorPaste: spread`): red's own copied pieces if there are as many, else the text's lines (one final line break ignored); never for a whole-line copy or one cursor. `copy_text` returns a `clipboard::Copied`; only one empty cursor's copy is whole-line (VS Code)
- `column.rs` — add cursor above / below (`add_cursor`, keeps the goal column), Shift+Alt+I (`cursors_at_line_ends`), column (box) select: `select(from, to)` per line between two display columns (nearest grapheme boundary, short lines skipped, VS Code's `ColumnSelection`), `step` for Cmd+Shift+Alt+arrows, `current` (the view's `ColumnBox` while its selections are unchanged, else the primary's anchor/head). Uses `pos::LineWalker`, not per-cursor lookups
- `lines.rs` — move lines (`move_lines`, Alt+Up/Down) and copy lines (`copy_lines`, Shift+Alt+Up/Down) at every selection: selections are grouped into line blocks (sharing a line; for moves also neighbouring lines); a non-empty selection ending at a line start doesn't take that line; one undo step (`EditKind::Other`), none if nothing moves; selections follow the moved / lower copied text
- `buffer/` — `Buffer`/`Buffers`, `ChangeSet`, undo `History`, file load/save, and **all** position types/conversions in `buffer::pos` (see `src/buffer/CLAUDE.md`)
- `selection.rs` — `Selection` (anchor, head, goal column) and `Selections` (sorted, merged, never empty; a primary and a *newest* — the one added last, which Cmd+D searches after; `new` takes the last list entry as newest, `with_newest` names it). The list is an `Arc` (copy on write): clones are cheap and equal clones compare by pointer; normalizing skips the sort when the list is already sorted (after an edit or mapping) and merges in place. Merging is VS Code's: overlapping, or touching when one is an empty cursor; two non-empty selections that only touch stay apart. `overlapping(from, to)` (binary search, for drawing) and `contains_range`
- `view.rs` — `View`: selections + scroll for one tab; `map_through` follows edits made via another view (and clears its Cmd+U history first, so the list is mapped in place; positions go start then end, in order, for the mapper's forward stepping); `reveal_range` (centre a range if off screen, set by Cmd+D); `reveal_pos` (scroll just enough to show a position, e.g. the newest added cursor); `column` (the last `column::ColumnBox`); `CursorHistory` (Cmd+U: up to 50 earlier selection lists + scroll, cleared by any text change)
- `multicursor.rs` — Cmd+D / Cmd+Alt+D (`next_match`, `Step::{Add, Skip}`) and Cmd+Shift+L (`select_all`) as VS Code's `MultiCursorSession`: a `Session` is word mode (empty single cursor: whole word, case-sensitive) or the primary's text literally with the find widget's case/whole-word (`App::find_options`, default off); searches after the newest selection, wraps, skips already-selected matches. Several selections with different text: Cmd+D first expands empty cursors to words
- `group.rs` — `Tab` (buffer id + view), `Group` (tabs, active), `Groups` (map + `SplitTree`, always ≥1 group)
- `layout.rs` — screen `Regions`, `SplitTree` of editor groups, `HitMap`/`Hit` for mouse hit-testing
- `ui/` — ratatui drawing, `ui::render(f, &mut App)`; colours only via `Theme` (see `src/ui/CLAUDE.md`)
- `theme.rs` — `Theme::dark_plus(truecolor)`: named keys (`"editor.selection"`, `"statusBar.error"`, …) → `Style`, 256-colour fallback
- `explorer.rs` — explorer tree model (no drawing), `list_dir` and `walk_files` (quick open) over one shared `ignore` walker (`.gitignore`, excludes, never `.git/` or `.red/`), `excludes`, blocking `ops::{new_file,new_folder,rename,delete}`
- `palette.rs` — palette model (no drawing): `Palette` (a `TextField` + `Row`s; `Picker::QuickOpen` switches `Mode` by first char: `>` commands, `:` go to line, else files; `Picker::LogLevel`), `command_rows` / `log_level_rows` (sync `nucleo` `Pattern`), `parse_line`, `file_query` (drops a `:line[:col]`, also half-typed), `FileMatcher` (a `nucleo::Nucleo` fed by the walk, matching on its own threads)
- `field.rs` — `TextField`: one-line input (text + byte cursor) used by `Input` prompts and the palette; `key()` returns `Outcome::{Ignored, Moved, Edited}`
- `search.rs` — the one matcher for find, Cmd+D and workspace search: `Matcher::new(pattern, Options { regex, case_sensitive, whole_word })` (`SearchError` is a one-line message for an invalid regex; an empty pattern matches nothing), then `find_next(rope, from, wrap)` / `find_prev(rope, before, wrap)` / `find_all` / `find_in(rope, range)` → `Match { start, end }` in `ByteIdx`. Searches line by line over `pos::each_line` (zero-copy per chunk); matches never span lines unless the pattern names a line break (newline, `\n`, `\r`, `\W` — VS Code's rule), which searches a copy of the whole text. Whole word = VS Code's boundary rule with `pos::is_word_char`. Also `impl grep_matcher::Matcher` so M7's `grep-searcher` uses the same matcher on files. `find_prev`/`is_empty` have `allow(dead_code)` outside tests until the find widget (6d) calls them
- `clipboard.rs` — `Clipboard`: arboard (lazy) / OSC 52 (SSH or arboard failure, tmux-wrapped) / `memory()` in tests; `write(Copied { text, whole_line, pieces })` remembers the last copy, `copied(text)` returns it while a paste's text still matches (newlines normalized)
- `workspace/` — `Workspace` (root, `.workspace` config, `settings()`), `config.rs` (`WorkspaceConfig` TOML), `trust.rs` (`TrustStore`, `Trust`)
- `watcher.rs` — `notify` watcher; the sync callback forwards paths into the app channel
- `terminal.rs` — enter/restore raw mode, alt screen, mouse, bracketed paste, kitty keyboard flags (`restore` is idempotent)
- `panic.rs` — panic hook: back up dirty buffers (`panic::set_dirty` snapshots), restore terminal, log, exit 101
- `logging.rs` / `paths.rs` — file logging under `paths::state_dir()/logs` (`$XDG_STATE_HOME/red` or `~/.local/state/red`); the filter sits in a `reload` layer (`LevelHandle`, `set_filter`, `current_filter`), `newest_log(dir)`. `paths::config_dir()` (`$XDG_CONFIG_HOME/red` or `~/.config/red`, holds `keybindings.toml`), `paths::strip_position` (`file:line:col`, shared by `main` and quick open)
- `milestone*_tests.rs` — end-to-end tests per milestone (`#[cfg(test)]` modules declared in `main.rs`)

Modules in PLAN.md's Architecture that don't exist yet (`syntax`, `lsp`, `git`, `history`) arrive with their milestone.

## Data flow

1. `App::run` (`app/mod.rs`): `tokio::select!` over crossterm `EventStream`, the `AppEvent` channel (`rx`) and the fs-debounce timer; then drains queued input so a burst becomes one frame; redraws only when `needs_redraw`.
2. `handle_event`: an open `prompt` gets every key; else `keymap.lookup(&key, self.context())` → `Command` (`context()` is `Palette` while the palette is open); else an open palette's `TextField` gets the key; else an unbound plain/Shift char in the editor is typed via `edit::insert_char`. `Event::Paste` (bracketed paste) → the prompt input, the palette, or `App::paste`.
3. `execute(cmd)`: any command except `Palette(_)` / `ShowCommands` / `QuickOpen` / `GoToLine` first closes an open palette. App-level commands (save, tabs, splits, focus, explorer, copy/cut/paste) are handled in `App`; everything else falls to `edit::run(cmd, buf, view, &settings)` on the active tab, after a read-only check if `edit::is_edit(cmd)`.
4. `edit` builds a `ChangeSet` over all selections and calls `Buffer::edit(changes, before, after, kind)` — one undo transaction.
5. `sync_views` (after every event) drains `Buffer::take_applied()` and maps every *other* view on that buffer through the changes, then `track_selections` records the active view's selection change for Cmd+U and ends the Cmd+D session if something else changed the selections or the editor lost focus.
6. Background results come back as `AppEvent` variants (`SaveDone`, `Opened`, `Listed`, `FsChanged`, `ConfigLoaded`, `FileOpDone`, `FilesMatched`, `FilesWalked`, `LogFileFound`) → `handle_app_event`.
7. `ui::render(f, &mut app)` draws and writes back render-time state: `view.height/width`, scroll for `reveal_cursor`, `explorer.height`, and `app.hits` (used by the *next* mouse event).

## Invariants and conventions

- **Positions:** `ByteIdx` (UTF-8 byte offset, char boundary; cursors on grapheme boundaries) is the canonical unit; `DisplayCol` for screen columns. Line/column/grapheme/word conversions live only in `buffer::pos` (`line_of`, `line_start`, `line_end`, `display_col`, `byte_at_col`, `next_grapheme`, `word_left`, …). Other modules hold `&Rope` but only call `len()`, `slice(..)`, `to_string()`, `clone()`, `floor_char_boundary()` on it. No raw `usize` byte offsets across modules (line indices are plain 0-based `usize`).
- **Text only changes through `Buffer::edit` / `undo` / `redo`** (the sole `rope.insert/remove` is in `ChangeSet::apply`). Bypassing it breaks undo and other views' cursors.
- **Selection list everywhere:** every command works on `view.selections` (a `Selections`, never a single cursor). Build new lists with `Selections::new`/`single`/`transform` so they stay sorted and merged.
- **Undo grouping:** `EditKind::Insert`/`Delete` join consecutive same-kind edits (<1 s, selections continuous); `EditKind::Other` (paste, cut) is always its own step. Non-edit actions (moves, clicks, tab/group switches, select-all) call `buf.seal_undo()`.
- **Read-only buffers:** `App::active_writable()` gates edits, cut and paste; copy still works.
- **Keymaps are data:** commands never look at keys. Default bindings live in `keymap::defaults` (a `Vec<Binding>`; list order decides which key the palette shows), per platform (`cmd` = Super on macOS, Ctrl on Linux; `word` = Alt on macOS, Ctrl on Linux). A context binding wins over a global (`None`) one. `keybindings.toml` (`[[binding]]` with `key`, `command`, optional `context` = `editor`/`explorer`/`palette`) is loaded once at startup in `main` (`App::set_keymap` reports errors): every command it names loses all its default keys; a user key also overrides whatever was on it in that context. `Command` must stay `Copy + Hash` (no owned payloads).
- **Colours:** no `Color::`/`Modifier::` outside `theme.rs`; widgets call `theme.get("key")` (a missing key logs `missing theme key`). Add new keys to `Theme::dark_plus`.
- **Never block the UI thread:** file reads, saves, listings, file ops and config reloads go through `tokio::task::spawn_blocking` and report back with an `AppEvent`. Blocking functions are documented `/// Blocking: call from spawn_blocking`. `App::new` must not spawn anything (it can run before the runtime is entered); `App::start` does.
- **Logging:** `tracing` only — never print while the UI is up. Targets are module paths (`red::app`, `red::buffer`, …) except explicit `target: "input"` (key/mouse/paste at trace) and `target: "panic"`. Each `execute` runs in a `command` span with the command name. User-visible messages go through `App::notify(Level, text)`, which also logs. `RED_LOG=input=trace` / `--log-level` set the filter.
- **Many selections:** Cmd+Shift+L can make 60k+ selections. Anything per edit or per frame must stay O(selections · log n): map positions with `ChangeSet::mapper()` (not `map` in a loop, and in order), no rope lookup or allocation per selection (build changes with `ChangeSet::push`, borrow the text), draw only `Selections::overlapping` the viewport, and don't scan the selection list per cell. `ChangeSet::apply` rebuilds the rope in one pass from 256 changes up. A rope line lookup costs a few hundred ns: commands that need each selection's line (add cursor, box select, line commands) visit them through `pos::LineWalker` in order instead of `line_of`/`display_col` per selection.
- Slow-work warnings: frames > 16 ms (`SLOW_FRAME`), saves > 500 ms (`SLOW_TASK`).

## Tests

- Unit tests sit in `#[cfg(test)] mod tests` at the bottom of each module.
- End-to-end tests: `src/milestone_tests.rs` (M1), `src/milestone2_tests.rs` … `milestone6_tests.rs`, named `milestone_N_*`. A new milestone adds `src/milestoneN_tests.rs` and a `#[cfg(test)] mod` line in `main.rs`. Each file opens with a `//!` doc saying what "done when" it covers.
- Driving the app headlessly (helpers are `#[cfg(test)] impl App` in `app/mod.rs`):
  - Build: `App::new(Workspace::load(root), TrustStore::load(dir.join("trust.toml")), Some(InitialFile { buffer, goto: None }), true)`, then `app.test_start()` (watcher + first listing). See `open_app` in each milestone file and `milestone_tests::test_app`.
  - Use `tempfile::tempdir()` and `workspace::normalize(dir.path())` for the root (macOS temp paths are symlinks). Keep the trust store inside the tempdir.
  - Input: `press(KeyCode, KeyModifiers)`, `type_text(&str)`, `mouse(kind, x, y)`, `mouse_with(kind, x, y, mods)` (Alt+Click, Shift+Alt+drag), `handle_test_event(Event)` (e.g. `Event::Paste`). Two clicks on one cell within 500 ms are a double click. Use a local `cmd()` helper (SUPER on macOS, CONTROL elsewhere).
  - Background work: `app.settle(|a| cond).await` pumps `AppEvent`s until `cond` (false after 5 s) — always `assert!` it. Tests that spawn use `#[tokio::test(flavor = "multi_thread")]`.
  - Rendering/mouse: `Terminal::new(TestBackend::new(w, h))`, `term.draw(|f| crate::ui::render(f, &mut app))`; draw before mouse events (hit map comes from the last frame); read cells via `term.backend().buffer()`.
  - State: `app.active_buffer()`, `app.group()`, `app.groups`, `app.prompt`, `app.ui.focus`, `app.explorer.rows()`, `app.quitting()`. `Buffer::from_str` exists for unit tests.
  - Under `cfg(test)` the app uses `Clipboard::memory()` — read it with `app.clipboard.read()`.
  - `app.last_command` (`cfg(test)`) is the last `Command` `execute` ran. `app.palette` (rows, `mode()`, `hint`) for the palette; quick open results arrive via `settle`. Set `app.log_dir` to a tempdir and `app.log` to a `reload::Handle` from a local `tracing_subscriber::registry()` (keep it alive) to test the log commands.
- **`terminal_docs_list_every_cmd_binding`** (`config/keymap.rs`) fails if the macOS Cmd bindings and the table in `docs/terminals/README.md` differ. Any new/changed/removed Cmd binding means updating that table *and* every terminal snippet (see `docs/terminals/CLAUDE.md`).

## Adding a command end to end

1. `command.rs`: add a variant to `Command` (or `ExplorerCommand` / `PaletteCommand`), its name in `Command::name()` (`area.camelCase`), its `title()` (both matches are exhaustive) and add it to `Command::all()` (not checked by the compiler; the palette and `keybindings.toml` only know commands listed there).
2. Behaviour: text/cursor work goes in `edit.rs` — add an arm in `edit::run`, and to `edit::is_edit` if it changes text; build changes per selection with `replace_each`/`replace_each_landing`; seal undo if it's not an edit. App-level work gets an arm in `App::execute` (`app/mod.rs`), with helpers in the right `app/` submodule.
3. Binding: add to the `editor`, `explorer`, `palette` or `global` list in `keymap::defaults`, for both platforms (`cmd`, `word`, `if mac { … }`). Don't bind plain characters in the editor context (they'd stop typing).
4. If it uses Cmd on macOS: add a row to `docs/terminals/README.md` and the key to every snippet.
5. Tests: a unit test next to the code, plus the end-to-end case in the milestone's test file through `press`.
6. PLAN.md's Milestones entry (✅, "Deferred", hand checks) is updated in the milestone's own commit.

## Commits

Look at `git log`. Milestone work: subject `Milestone N: <short summary>` (whole milestone) or `Milestone N (Na): <short summary>` (one task), blank line, `- ` bullets of user-visible behaviour and notable internals (wrapped ~75 cols). Smaller changes: short imperative subject (`Mark milestone 4 done in the plan`, `Fix crash opening a file inside its workspace; add .workspace`) with a prose body explaining the why when non-obvious. Plan-only edits go in their own commit. End with the `Co-Authored-By:` trailer.

## Keeping these files current

When a change adds a module, a convention, a test helper or a new way of doing background work, update this file (and the nested `CLAUDE.md` for that directory) in the same change. Keep them short and only state what the code actually does.
