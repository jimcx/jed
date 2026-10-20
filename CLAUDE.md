# jed

A VS Code-style terminal text editor in Rust (ratatui + crossterm + tokio, ropey 2 beta for text).
**PLAN.md is the source of truth for design and scope** — read the section you need instead of
guessing: Features (one subsection per area), [Keybindings](PLAN.md#keybindings-vs-code-defaults),
[Architecture](PLAN.md#architecture), [Text positions](PLAN.md#text-positions),
[Async and blocking work](PLAN.md#async-and-blocking-work), [Logging](PLAN.md#logging),
[Milestones](PLAN.md#milestones) (each has "Needs", "Deferred" and a "Done when" to test against).
Milestones 1–13 are done and keep their numbers; later ones have names and short ids, not numbers, and no order ([Named milestones](PLAN.md#named-milestones): those marked ✅ are done), each built task by task (`tools-a`, `tools-b`, …), one commit per task, via `/milestones <name>`. Further work is PLAN.md's TODO list, one item per commit via `/todo` (an item too big becomes smaller items or a new named milestone). Finished milestones and TODO items keep one line in PLAN.md; their full entries (what was built, "Deferred", hand checks, notes for later work) are in [docs/plan-done.md](docs/plan-done.md) — grep it when something refers back to one.

**This file is the summary; details are read on demand**: [docs/dev/modules.md](docs/dev/modules.md) (every module in full, the data flow step by step), [docs/dev/conventions.md](docs/dev/conventions.md) (feature-specific rules), [docs/dev/testing.md](docs/dev/testing.md) (test helpers per feature), [docs/dev/app.md](docs/dev/app.md) (every `src/app/` submodule in full), and the nested `CLAUDE.md` files in `src/app`, `src/buffer`, `src/lsp`, `src/syntax`, `src/ui`, `docs/terminals`.

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

## Module map (`src/`)

One line each; [docs/dev/modules.md](docs/dev/modules.md) has every module's types, functions and rules — read the entries for what you touch. "Model" modules hold no app state and draw nothing: `app/<area>.rs` drives them, `ui/<area>.rs` draws them.

- `main.rs` — args (`jed [<folder> | <file>[:line[:col]]]`, `--log-level`); loads the first file, `.workspace`, `settings.toml`, `tasks.toml`, `keybindings.toml` before raw mode; builds the runtime, runs `App`
- `app/` — `App`: all state, the event loop, `execute(Command)`, prompts, mouse, background results (see `src/app/CLAUDE.md` + [docs/dev/app.md](docs/dev/app.md))
- `command.rs` — `Command` and sub-enums; `all()`, `name()` (`edit.copy`), `from_name`, `title()`
- `config/keymap.rs` — `Keymap` (`(KeyCombo, Option<Context>) -> Command`), `Context`s and their parents, defaults per `Platform`, `keybindings.toml`
- `config/settings.rs` — `SettingsOverrides` (`settings.toml` / `.workspace` `[settings]`), `layer`, lenient `load`, `TEMPLATE`
- `edit.rs` — movement and editing at every selection (`edit::run`, `insert_char`, `paste`, `copy_text`, `cut`), `Settings`
- `completion.rs` — completion's model: `parse`, `Filter`, `Anchor`, `Popup`, `insert`
- `column.rs` — add cursor above / below, cursors at line ends, column (box) select
- `commit_view.rs` — a commit tab's model (`CommitDoc` on a read-only `Buffer`)
- `comment.rs` — toggle line comment (Cmd+/)
- `indent.rs` — auto-indent on Enter and on typed closers
- `brackets.rs` — bracket matching (`match_at`, `Openers`, `Matches`)
- `pairs.rs` — auto-close, type-over, wrap selection, delete pair; `tests::{setup, show}` mark text with `|` / `«…»`
- `lines.rs` — move / copy lines
- `shift.rs` — indent / outdent lines, Tab over lines
- `buffer/` — `Buffer`/`Buffers`, `ChangeSet`, undo `History`, load/save, **all** position types in `buffer::pos` (see `src/buffer/CLAUDE.md`)
- `selection.rs` — `Selection`, `Selections` (sorted, merged, never empty, `Arc` copy-on-write; primary and newest)
- `view.rs` — `View`: one tab's selections + scroll, `map_through`, reveal, Cmd+U history
- `multicursor.rs` — Cmd+D / Cmd+Alt+D / Cmd+Shift+L
- `navigation.rs` — back / forward history's model
- `group.rs` — `Tab::{Text, Diff}`, `Group`, `Groups` (+ `SplitTree`)
- `layout.rs` — `SidePanel`, `Regions`, `PanelTab`, `SplitTree`, `HitMap` / `Hit` for mouse hit-testing
- `ui/` — ratatui drawing, `ui::render(f, &mut App)` (see `src/ui/CLAUDE.md`)
- `theme.rs` — `Theme::dark_plus`: named keys → `Style`, syntax keys = capture names, 256-colour fallback
- `diagnostics.rs` — `Severity`, `Diagnostic`, `Marks` (on every buffer, mapped through edits), `runs`
- `explorer.rs` — explorer tree model; `walker` (shared by quick open and workspace search), blocking `ops`
- `palette.rs` — palette model: `Picker`s, `Mode`s, row builders, `FileMatcher`
- `field.rs` — `TextField`, the one-line input
- `find.rs` — `FindWidget`, the find / replace widget's model + match cache
- `search.rs` — the one matcher (`Matcher`, `Replacement`) for find, Cmd+D and workspace search
- `workspace_search.rs` — workspace search job, `Results`; `#[cfg(test)] reference`
- `path_filter.rs` — files to include / exclude (`PathFilter`)
- `grouped.rs` — lists grouped by file (`Group`, `RowKey`, `ListState`): reuse it for new ones
- `search_view.rs` — the Search view's model
- `clipboard.rs` — arboard / OSC 52 / `memory()` in tests
- `workspace/` — `Workspace`, `.workspace` config (`config.rs`), trust (`trust.rs`)
- `syntax/` — `Language`, `detect`, `Syntax` (tree-sitter tree + highlight spans per buffer) (see `src/syntax/CLAUDE.md`)
- `hover.rs`, `rename.rs`, `format.rs`, `locations.rs`, `problems.rs` — LSP features' models (`rename::changes` turns any `TextEdit[]` into a `ChangeSet`)
- `lsp/` — language servers: `Manager`, server tasks, document sync, diagnostics, `fake` server for tests (see `src/lsp/CLAUDE.md`)
- `git/` — `git2`, blocking: `Repo`, `Status`, `diff`, `stage`, `log`, `show`; `test_repo` for tests
- `git_show.rs` — a commit rendered as `git show --stat`
- `disk.rs` — files changed on disk: check, reload changes, banner actions
- `quick_diff.rs` — gutter git markers (on every buffer)
- `blame.rs` — inline blame (on every buffer)
- `diff_view.rs` — the diff view's model (`Text`, `Pair`, `Doc`, `DiffTab`)
- `git_history.rs` — the Explorer's "Git History" section's model
- `scm.rs` — the Source Control view's model
- `history.rs` — file history's storage under `<root>/.jed/history/`
- `scrollbar.rs` — `Scrollable`, `Scroll`, thumb maths
- `session.rs` — sessions' model and file
- `recent.rs` — recent workspaces' model and file
- `tasks/` — task templates, PTY runs (`Run`), key encoding, `Tasks`
- `watcher.rs` — `notify` watcher → app channel
- `terminal.rs` — raw mode, alt screen, mouse, kitty flags, pointer shape, tmux passthrough
- `backup.rs` — crash backups' model and files
- `panic.rs` — panic hook (backs up dirty buffers, restores the terminal)
- `logs/` — tool logs: `Logs` rings, disk files, log tabs' model, the `tracing` layer
- `logging.rs` / `paths.rs` — file logging and its reloadable filter; state / config dirs
- `tools_view.rs` — the Tools view's model
- `milestone*_tests.rs`, `<id>_tests.rs`, `todo_tests.rs` — end-to-end tests (see Tests)

## Data flow

[docs/dev/modules.md](docs/dev/modules.md#data-flow) has each step in full.

1. `App::run` (`app/mod.rs`): `tokio::select!` over crossterm events, the `AppEvent` channel and one timer (`next_timer` → `on_timer`); drains queued input into one frame; then the per-turn syncs (`sync_syntax`, `sync_quick_diff`, `sync_blame`, `sync_diffs`, `sync_commit_tabs`, `sync_git_history`, `sync_backups`, `sync_logs`, `sync_servers`); redraws only when `needs_redraw`.
2. `handle_event`: a prompt gets every key; else `keymap.lookup(&key, self.context())` → `Command`; else the palette / find input / Search view / task tab takes it; else an unbound char is typed via `edit::insert_char`. Each event runs between `nav_begin` / `nav_end` (back / forward recording).
3. `execute(cmd)`: app-level commands in `App`; diff tabs through `diff_command`; the rest to `edit::run` on the active tab (after a read-only check for edits).
4. `edit` builds one `ChangeSet` over all selections → `Buffer::edit` (one undo transaction).
5. `sync_views` maps every *other* view through the applied changes, then `track_selections` (Cmd+U, ends a Cmd+D session).
6. Background work reports back as `AppEvent` variants → `handle_app_event`. On quit `run` awaits `App::shutdown` (servers, tasks, the session, backups).
7. `ui::render(f, &mut app)` draws and writes back render-time state (view sizes, list heights, PTY sizes, `app.hits` for the *next* mouse event).

## Invariants and conventions

- **Positions:** `ByteIdx` (UTF-8 byte offset, char boundary; cursors on grapheme boundaries) is the canonical unit; `DisplayCol` for screen columns. Line/column/grapheme/word conversions live only in `buffer::pos` (`line_of`, `line_start`, `line_end`, `display_col`, `byte_at_col`, `next_grapheme`, `word_left`, …). Other modules hold `&Rope` but only call `len()`, `slice(..)`, `to_string()`, `clone()`, `floor_char_boundary()` on it. No raw `usize` byte offsets across modules (line indices are plain 0-based `usize`).
- **Text only changes through `Buffer::edit` / `undo` / `redo`** (the sole `rope.insert/remove` is in `ChangeSet::apply`, called only from `Buffer::apply`, which also feeds each change set to `Syntax::edit` and, while a language server has the document open, records it with the rope from before it for `take_tracked`). Bypassing it breaks undo, other views' cursors, the parse tree and the servers' copies.
- **Parse trees:** `buf.syntax.tree()` is `Some` only while the tree matches the text exactly (never a stale one); between an edit and the next `sync_syntax`, and while a background parse runs, it is `None`. Drawing never reads the tree: it asks `buf.syntax.spans(rope, lines)`, which keeps the last colours (mapped through the edits) until the new tree arrives. tree-sitter `Point`s come from `pos::ts_point` (rows count LF only, as tree-sitter does).
- **Selection list everywhere:** every command works on `view.selections` (a `Selections`, never a single cursor). Build new lists with `Selections::new`/`single`/`transform` so they stay sorted and merged.
- **Undo grouping:** `EditKind::Insert`/`Delete` join consecutive same-kind edits (<1 s, selections continuous); `EditKind::Other` (paste, cut, comment line, wrapping a selection) is always its own step; an auto-close seals before itself, so it starts a step that later typing joins (VS Code). Non-edit actions (moves, clicks, tab/group switches, select-all) call `buf.seal_undo()`.
- **Read-only buffers:** `App::active_writable()` gates edits, cut and paste; copy still works.
- **Tabs:** a tab is a text editor or a diff view (`group::Tab`); log and commit tabs are text tabs (see Log tabs). Code acting on "the active editor" uses `Group::active_tab()` / `App::active_mut()` / `active_buffer()`, which are `None` on a diff tab — so editor commands, find, completion, hover, LSP requests and the gutter do nothing there without checks of their own. What a diff tab should still do goes through `App::active_file_buffer()` (a text tab's buffer, or the open buffer of a diff tab's file: Save, Reload, Keep Mine, Compare with Disk) or the diff's own arms in `execute` (`diff_command`, `diff_stage`). A new tab kind adds a `Tab` variant, its drawing in `ui::mod` and `ui::tabs`, its `split` copy and its context — or, when it's text to read, is a read-only buffer as logs and commits are.
- **Keymaps are data:** commands never look at keys. Default bindings live in `keymap::defaults` (a `Vec<Binding>`; list order decides which key the palette shows), per platform (`cmd` = Super on macOS, Ctrl on Linux; `word` = Alt on macOS, Ctrl on Linux). A context binding wins over a global (`None`) one. `keybindings.toml` (`[[binding]]` with `key`, `command`, optional `context` = `editor`/`explorer`/`palette`/`find`/`replace`/`search`/`searchResults`/`problems`/`locations`/`suggest`/`diff`/`scm`/`tools`/`gitHistory`/`commit`/`task`) is loaded once at startup in `main` (`App::set_keymap` reports errors): every command it names loses all its default keys; a user key also overrides whatever was on it in that context. `Command` must stay `Copy + Hash` (no owned payloads).
- **Settings layer per key:** `edit::Settings::default()` ← `~/.config/jed/settings.toml` ← `.workspace` `[settings]`, only in `config::settings::layer` (via `Workspace::settings()`, stored in `App::settings`, recomputed on a `.workspace` reload). `settings.toml` is read once at startup (live reload is a TODO in PLAN.md). Adding a setting: a field in `edit::Settings` (+ default), an `Option` field in `SettingsOverrides` and a line in its `apply`, a row in `config/settings.rs`'s doc table and a line in its `TEMPLATE` (`template_lists_every_key_at_its_default` names every field without `..`, so it won't compile until the new one is added there, and then fails if `TEMPLATE` lacks it); it then works from both files.
- **Colours:** no `Color::`/`Modifier::` outside `theme.rs`; widgets call `theme.get("key")` (a missing key logs `missing theme key`). Add new keys to `Theme::dark_plus`.
- **Never block the UI thread:** file reads, saves, listings, file ops, config reloads, workspace search, first parses and every `git2` call go through `tokio::task::spawn_blocking` and report back with an `AppEvent`; language servers are async tokio tasks (process spawn included) reporting via `AppEvent::Lsp`; task runs spawn their PTY process in `spawn_blocking` and read / wait on their own threads, reporting via `AppEvent::Task` (output throttled to one event per frame). Blocking functions are documented `/// Blocking: call from spawn_blocking`. `App::new` must not spawn anything (it can run before the runtime is entered); `App::start` does.
- **Language server requests:** `App::send_request(group, buffer, at, method, supported, Request)` (`app/lsp.rs`) sends through `lsp::Manager::request_at` (`send_doc_request` / `Manager::request_doc`: no `position`, for whole-document requests like formatting) and keeps a `Pending` (request kind, group, buffer, revision, position, encoding) in `App::requests` by (server, id); a newer request of the same kind cancels the older (`$/cancelRequest`), `check_requests` after each input event cancels ones whose place moved on (buffer edited; the cursor — the active view of the active group — or, for a mouse hover, the pointer elsewhere), answers are taken only for pending and still current requests (else dropped as stale; errors that are `RpcError::is_routine` — cancelled, ContentModified (rust-analyzer while loading) — dropped quietly, no message) and go to their kind's handler (`hover_answer`, `definition_answer`, `completion_answer`, `references_answer`, `rename_answer`, `format_answer`, …); `Failed` / `Exited` drop that server's pending ones. A new request kind adds a `Request` variant and an arm in `App::answer`.
- **Edits from background results** (a rename's `WorkspaceEdit`) go through `Buffer::edit` like any other; the code sets the active view's selections itself when it shows the buffer, then calls `App::follow_edits` (`sync_views` without the Cmd+U / Cmd+D tracking) so every other view follows at once. A file not open is loaded through the `opening` map (`OpenRequest { background, edits }`), never on the UI thread.
- **Logging:** `tracing` only — never print while the UI is up. Targets are module paths (`jed::app`, `jed::buffer`, …) except explicit `target: "input"` (key/mouse/paste at trace), `target: "panic"`, `"lsp::wire"` (every JSON-RPC message in / out at trace, with `server`) and `"lsp::stderr"` (server stderr lines at debug; a crash logs the last 20 at warn). Each `execute` runs in a `command` span with the command name. User-visible messages go through `App::notify(Level, text)`, which also logs. `JED_LOG=input=trace` / `--log-level` set the filter.
- **Scroll bars:** anything scrolled gets the one-column bar by implementing `scrollbar::Scrollable` (rows, height, top as a row index, set top — O(1) / per group, never per row: lists keeping their top by key convert by binary search), drawing it in its area's last column (`ui::scrollbar::take_column`, always reserved, + `render` with `look(app, target)`) and reporting the `ScrollBarHit` under a `ScrollTarget`, plus an arm in `App::with_scrollable`; clicks, drags, hover and the wheel then work as for the others.
- **Many selections:** Cmd+Shift+L can make 60k+ selections. Anything per edit or per frame must stay O(selections · log n): map positions with `ChangeSet::mapper()` (not `map` in a loop, and in order), no rope lookup or allocation per selection (build changes with `ChangeSet::push`, borrow the text), draw only `Selections::overlapping` the viewport, and don't scan the selection list per cell. `ChangeSet::apply` rebuilds the rope in one pass from 256 changes up. A rope line lookup costs a few hundred ns: commands that need each selection's line (add cursor, box select, line commands) visit them through `pos::LineWalker` in order instead of `line_of`/`display_col` per selection.
- Slow-work warnings: frames > 16 ms (`SLOW_FRAME`), saves and parses > 500 ms (`SLOW_TASK`).
- **Feature conventions** — read the one in [docs/dev/conventions.md](docs/dev/conventions.md) before touching its area:
  - *Files changed on disk*: buffers keep their file's `Stamp` (length + SHA-256, never times); checks run off-thread; clean buffers reload, edited ones get the banner; a save refuses on a conflict.
  - *Log tabs* (and commit tabs): read-only `Buffer`s with an empty path. Anything keyed by a file must skip them: check `Buffer::has_file()`, or use `active_path()` / `active_file_buffer()` (`None` there). `tools_every_command_on_a_log_tab_leaves_it_alone` runs every command on one.
  - *File history*: recorded after a save in a second blocking task; renames move it; watcher events under `<root>/.jed/` are dropped.
  - *Sessions*: read once in `start`, written on quit (not while the restore's read runs), never on a crash; off in tests unless `app.state_dir` is set.
  - *Crash backups*: dirty buffers every `backup::INTERVAL`, removed when clean / closed, offered at start after the session restore.
  - *Back / forward*: one before / after compare per event; a jump that doesn't go through `open_file` calls `nav_jump()`.
  - *Recent workspaces*: recorded at start off-thread; jed never switches its own workspace.
  - *Tool logs*: server tasks push to `app.logs` off the UI thread; log tabs take new lines each turn; nothing polls.

## Tests

- Unit tests sit in `#[cfg(test)] mod tests` at the bottom of each module.
- End-to-end tests: `src/milestone_tests.rs` (M1), `src/milestone2_tests.rs` … `milestone8_tests.rs`, named `milestone_N_*`; named milestones' `src/<id>_tests.rs` (`tools_tests.rs`), named `<id>_*`. A named milestone adds `src/<id>_tests.rs` (e.g. `tools_tests.rs`, tests named `<id>_*`; a release budget test keeps `milestone` in its name, e.g. `milestone_soft_wrap_budget_*`, so the release command above picks it up) and a `#[cfg(test)] mod` line in `main.rs`. Each file opens with a `//!` doc saying what "done when" it covers.
- Driving the app headlessly (helpers are `#[cfg(test)] impl App` in `app/mod.rs`):
  - Build: `App::new(Workspace::load(root), TrustStore::load(dir.join("trust.toml")), Some(InitialFile { buffer, goto: None }), true)`, then `app.test_start()` (watcher + first listing). See `open_app` in each milestone file and `milestone_tests::test_app`. With user settings: `Workspace::load(root).with_user_settings(config::settings::load(path).overrides)`, set `app.config_dir`, and `app.report_errors(errors)` as `main` does (`milestone8_tests::user_app`); never set `XDG_CONFIG_HOME` (tests run in parallel).
  - Use `tempfile::tempdir()` and `workspace::normalize(dir.path())` for the root (macOS temp paths are symlinks). Keep the trust store inside the tempdir.
  - `app.test_open(buffer)` opens a second buffer in a new active tab (no file I/O).
  - Input: `press(KeyCode, KeyModifiers)`, `press_file_start()` / `press_file_end()` (the platform's file start / end key: Cmd+Up / Down on macOS — Cmd+Home / End are unbound there, as in VS Code — Ctrl+Home / End on Linux), `type_text(&str)`, `mouse(kind, x, y)`, `mouse_with(kind, x, y, mods)` (Alt+Click, Shift+Alt+drag), `handle_test_event(Event)` (e.g. `Event::Paste`). Two clicks on one cell within 500 ms are a double click. Use a local `cmd()` helper (SUPER on macOS, CONTROL elsewhere).
  - Background work: `app.settle(|a| cond).await` pumps `AppEvent`s (and fires the Search view's debounce) until `cond` (false after 5 s) — always `assert!` it. `app.pump()` handles what's already queued without waiting (for timing a frame). Both run `sync_syntax` as the event loop's turn does, so after typing, `pump` re-parses and `settle(|a| a.active_buffer().unwrap().syntax.tree().is_some())` waits for a background parse (`sync_syntax` does nothing outside a tokio runtime). `app.search_tick(Instant::now() + search_view::DEBOUNCE)` fires a pending Search view debounce without sleeping; `app.test_completion_tick(now)` the quick-suggestion timer (`settle` fires it too). Tests that spawn use `#[tokio::test(flavor = "multi_thread")]`.
  - Rendering/mouse: `Terminal::new(TestBackend::new(w, h))`, `term.draw(|f| crate::ui::render(f, &mut app))`; draw before mouse events (hit map comes from the last frame); read cells via `term.backend().buffer()`.
  - Render whitespace (default `all`) draws spaces as `·`: tests that read drawn text with spaces in it call `app.test_plain_whitespace()` after building the app (the session toggle, so it survives `.workspace` reloads); the earlier milestones' builders that draw do. `todo_tests::render_whitespace` reads the glyphs.
  - Under `cfg(test)` the app uses `Clipboard::memory()` — read it with `app.clipboard.read()`.
  - Feature-specific helpers (files on disk, git, diff tabs, history, tasks, sessions, backups, tool logs, LSP / fake server, completion, find, scroll bars, …): [docs/dev/testing.md](docs/dev/testing.md). Read the ones for the area you test.
- **`terminal_docs_list_every_cmd_binding`** (`config/keymap.rs`) fails if the macOS Cmd bindings and the table in `docs/terminals/README.md` differ. Any new/changed/removed Cmd binding means updating that table *and* every terminal snippet (see `docs/terminals/CLAUDE.md`).

## Adding a command end to end

1. `command.rs`: add a variant to `Command` (or `ExplorerCommand` / `PaletteCommand`), its name in `Command::name()` (`area.camelCase`), its `title()` (both matches are exhaustive) and add it to `Command::all()` (not checked by the compiler; the palette and `keybindings.toml` only know commands listed there).
2. Behaviour: text/cursor work goes in `edit.rs` — add an arm in `edit::run`, and to `edit::is_edit` if it changes text; build changes per selection with `replace_each`/`replace_each_landing`; seal undo if it's not an edit. App-level work gets an arm in `App::execute` (`app/mod.rs`), with helpers in the right `app/` submodule.
3. Binding: add to the `editor`, `explorer`, `palette`, `find`, `replace`, `search` or `global` list in `keymap::defaults`, for both platforms (`cmd`, `word`, `if mac { … }`). Don't bind plain characters in the editor, find, replace or search contexts (they'd stop typing).
4. If it uses Cmd on macOS: add a row to `docs/terminals/README.md` and the key to every snippet.
5. Tests: a unit test next to the code, plus the end-to-end case in the milestone's test file through `press`.
6. PLAN.md's Milestones entry (✅, "Deferred", hand checks) is updated in the milestone's own commit.

## Commits

Look at `git log`. Milestone work: subject `<Milestone name> (<task id>): <short summary>` (one task, e.g. `Tools view and logs (tools-a): …`; the numbered milestones used `Milestone N (Na): …`), blank line, `- ` bullets of user-visible behaviour and notable internals (wrapped ~75 cols). Smaller changes: short imperative subject (`Mark milestone 4 done in the plan`, `Fix crash opening a file inside its workspace; add .workspace`) with a prose body explaining the why when non-obvious. Plan-only edits go in their own commit. End with the `Co-Authored-By:` trailer.

## Keeping these files current

When a change adds a module, a convention, a test helper or a new way of doing background work, update the docs in the same change. Keep them short and only state what the code actually does. **Budget:** this root file stays a summary (~30 KB): a change adds at most a line or two here (a module's one-line entry, a cross-cutting rule); the detail goes in [docs/dev/](docs/dev/) (modules.md, conventions.md, testing.md) or the nested `CLAUDE.md` of the directory it's about. Don't repeat what a module's `//!` doc already says — point at it. When adding detail, cut what has gone stale rather than only appending.
