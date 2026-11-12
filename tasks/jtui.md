---
id: jtui
name: "jtui: shared TUI building blocks"
summary: jed's application-neutral TUI pieces (host terminal setup, clipboard, dirs, split tree, borders, scroll bar, text field, context menu, theme and keymap machinery) move to `crates/jtui`, jed re-exporting them
status: in-progress
priority: 0
dependencies: [crates_workspace]
---

See [Shared crates and jmux](../PLAN.md#shared-crates-and-jmux), [jmux's plan](../jmux/PLAN.md#crates), `docs/dev/crates.md`.

  - Needs: the workspace (`crates_workspace`); `terminal.rs`, `clipboard.rs`, `paths.rs`, `url.rs`, `ipc.rs` (`socket_dir`, `prepare_dir`, `remove_socket`), `scrollbar.rs` + `ui/scrollbar.rs`, `field.rs`, `context_menu.rs`, `layout.rs` (`SplitTree`, `Node`, `SplitDir`, `DropSide`, `Divider`), `ui/borders.rs`, `theme.rs`, `config/keymap.rs`
  - Every move is a move, not a rewrite: the code and its unit tests go to `crates/jtui/src/<module>.rs`, and the jed module that held it becomes `pub use jtui::<module>::*` (plus whatever jed-only part stays), so no call site elsewhere in jed changes. jed's behaviour and its tests do not change
  - Shared code names no application. What was hard-coded becomes an argument: the `jed` directory name in `paths` (`state_dir`, `config_dir`, socket dir), the window title and user variable names in `terminal`, the cursor shape (a `jtui` type; `CursorShape::of(&edit::Settings)` stays in jed)
  - Generic where jed's `Command` was named: `ContextMenu<C>` / `MenuItem<C>`; `TextField::run(FieldCommand)` stays as is and `FieldCommand::from_command(Command)` stays in jed; the grapheme and word steps `field` takes from `buffer::pos` on a `&str` move with it (jed's `buffer::pos` calls the shared ones, so there is one implementation)
  - `Theme`: the key → `Style` map with `get` / `find` / `dim` / `ansi` / `terminal_cell`, the 256-colour fallback and `terminal_has_truecolor` move; a `Theme` is built from a list of entries, and the keys the shared widgets read (scroll bar, borders, tab bar, text field, menu, terminal) are listed in one `jtui` function both apps call. `dark_plus`'s editor and syntax entries, `token_styles`, `markdown_style` and `status_bar_auto` stay in jed (a wrapper or extension over the shared type)
  - `Keymap`: `KeyCombo` (`from_event`, `parse`, `spec`, `label`), `Platform` and a `Keymap<Ctx, Cmd>` (lookup with a context's parent fallback, user bindings over defaults, `Source`, the `keybindings.toml` reader) move; jed's `Context`, `defaults`, command names and the `terminal_docs_list_every_cmd_binding` test stay in jed
  - Deferred: `logging.rs` (tied to jed's tool logs), `palette.rs`, `group.rs` and `ui/tabs.rs` (closed over jed's tab kinds), `HitMap` and the hit structs, `session.rs`
  - Done when: `crates/jtui` builds and tests alone (`cargo test -p jtui`) with no reference to jed; `grep -rn '"jed"\|JED_' crates/jtui/src` finds nothing outside tests; jed's `scripts/check` passes with no test changed except moved unit tests; the release `milestone` and `search` budget runs are within noise of before
  - Subtasks (one agent and one commit each):
    - ✅ jtui-a: leaves — `paths`, `url`, the socket dir helpers from `ipc.rs`, `clipboard` and `terminal` (they use each other: OSC 52 passthrough, base64), with the application name, window title and cursor shape passed in
      - Done: `jtui::{paths, url, socket, clipboard, terminal}`. `pub const APP: &str = "jed"` in `main.rs` is the name jed passes in; jed's `paths::{state_dir, data_dir, config_dir}` and `ipc::default_socket_dir` are argument-free wrappers over jtui's `(app)` versions. `recent::display` is `jtui::paths::display`
      - `terminal`: jtui has its own `CursorStyle`; jed's `terminal::cursor_shape(&edit::Settings)` replaces `CursorShape::of`. `Identity::new(app, root, single, home)`; `enter` remembers the app in a `Mutex` (poison-tolerant, for the panic hook) so `restore()` stays argument-free
      - `cfg(test)` doesn't cross crates: `Clipboard::memory()` is always compiled; jed's `url::open` wraps jtui's and keeps the `OPENED` test hook. Later moves: keep test hooks jed-side or always compiled
      - `arboard`, `libc`, `tempfile` are in `[workspace.dependencies]`
    - jtui-b: `scrollbar` (model) and `ui/scrollbar.rs`'s drawing that takes no `App` (`render`, `render_row`, `render_marks`, `take_column`, `take_row`; `look(app, …)` stays in jed), `field` with its `&str` grapheme / word steps, `context_menu` generic over the command type
    - jtui-c: the split tree — `SplitTree`, `Node`, `SplitDir`, `DropSide`, `Divider`, `GroupId` and their tests out of `layout.rs` into `jtui::split`, and `ui/borders.rs` (junction glyphs) into `jtui::borders`; `layout.rs` keeps `Regions`, `compute`, `HitMap` and the hit structs
    - jtui-d: `Theme` machinery and the shared widgets' key list; jed's `Theme::dark_plus` builds on it
    - jtui-e: `KeyCombo`, `Platform` and the generic `Keymap<Ctx, Cmd>`; if making `Keymap` generic needs changes outside `config/keymap.rs`, `keys.rs` and their direct users, move only `KeyCombo` / `Platform` and the file reader and record the rest as deferred. Also the done-when checks, `docs/dev/modules.md` / `crates.md` entries for everything moved, and marks the task ✅
