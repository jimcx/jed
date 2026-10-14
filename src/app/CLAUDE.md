# app

`App` (in `mod.rs`) owns all state; submodules are `impl App` blocks split by area, with `pub(super)` methods:

- `mod.rs` — struct, `new`/`start`/`run`, `handle_event`, `execute`, `sync_views`, copy/paste, `handle_app_event`, crash snapshots, and the `#[cfg(test)]` driving helpers (`test_start`, `press`, `type_text`, `mouse`, `handle_test_event`, `settle`, `quitting`, the `last_command` field).
- `files.rs` — open (off-thread `Buffer::open` → `AppEvent::Opened`; `open_file(path, focus, goto)`), `add_buffer` (1-based `goto`), `place_cursor` (1-based line/column, clamped), tab activation/cycling, `focus_group`, `split`, `close_tab`/`discard_tab`, save/`save_done`, quit.
- `prompt.rs` — `Prompt` (modal: `SaveBeforeClose`, `SaveAllBeforeQuit`, `Trust`, `Input`, `ConfirmDelete`), `Input` (a `field::TextField` plus what the name is for), `handle_prompt_key`, trust decisions (the `Trust` dialog also shows the current decision when reopened by "Manage Workspace Trust").
- `side.rs` — explorer commands, directory listings, file ops (`FileOp` → `AppEvent::FileOpDone`), fs-change batching (`flush_fs_changes`), `.workspace` reload (`config_loaded`).
- `palette.rs` — `open_palette(picker, text)` / `close_palette`, `palette_command` (`PaletteCommand`), `palette_key` / `palette_paste` (input → `refresh_palette`), `accept_palette` (run a command, open a file at `file:line:col`, go to line, set a log level), `palette_mouse`; quick open's walk (`start_file_walk`, once per opening, into `FileMatcher`) and `files_matched` / `files_walked`; the palette-only commands: `manage_trust`, `open_log_file` (→ `AppEvent::LogFileFound`), `set_log_filter`.
- `mouse.rs` — clicks via `HitMap` from the last frame, `ClickTracker` (double/triple), `Drag` (side border, split divider, text), wheel scrolls the group under the pointer without focusing it.

Things to know before editing:

- **Background work pattern:** clone `self.tx`, `tokio::task::spawn_blocking`, send an `AppEvent` variant, handle it in `handle_app_event`. In-flight sets (`listing`, `opening`) dedupe requests. Saves report `state` back so `finish_save` marks the right history state.
- `App::new` must start no background work (it once crashed by spawning before the runtime existed); put it in `start()`.
- `Focus` is `Editor` or `SidePanel`; `context()` maps it to the keymap `Context` (`Palette` whenever `app.palette` is open; the palette doesn't change `Focus`). Starting with no file, or closing the last tab while the side panel is visible, focuses the side panel.
- The palette (`app.palette`) is not a `Prompt`: its keys go through the keymap's `Palette` context, unbound keys edit its `TextField`, and while it's open it takes every mouse event (click outside closes it). A prompt opened on top of it gets keys first. `execute` closes it before running any other command.
- Quick open: `FileMatcher`'s notify callback sends `AppEvent::FilesMatched` at most once until handled (`files_pending`); handling it calls `tick()` (never waits) and rebuilds rows. `FilesWalked { generation }` marks the walk done unless a newer walk started.
- `set_keymap(keymap, errors)` (from `main`) replaces the keymap and shows the first load error; `log` (reload handle, `None` if logging is off) and `log_dir` are set by `main` / tests.
- A prompt swallows all keys and mouse events; `close_prompt()` re-opens a pending trust prompt (`trust_pending`). Esc/Enter/letter handling per prompt is in `handle_prompt_key`; unrecognised keys put the prompt back.
- Leaving a tab/group (`activate_tab`, `cycle_tab`, `focus_group`, `split`, editor click) calls `buf.seal_undo()` first.
- Closing: the save prompt appears only when the tab is the last one (in any group) showing a dirty buffer (`groups.tabs_showing`). `discard_tab` drops the buffer when no tab shows it and closes an empty group (never the last). `AfterSave::{Quit, Close}` continues a close/quit once saves finish.
- `sync_views` runs after every input event: the editing tab only re-anchors its viewport; every other view on the buffer runs `View::map_through`.
- `update_crash_snapshot` runs every loop turn so the panic hook can back up dirty buffers.
- Messages: `notify(Level, text)` sets the status-bar message and logs it; any key press clears it.
- File system events are debounced (`FS_DEBOUNCE`, 100 ms) via `fs_deadline`; explorer renames/deletes update open buffers' `path`/`deleted`.
