# app

`App` (in `mod.rs`) owns all state; submodules are `impl App` blocks split by area, with `pub(super)` methods:

- `mod.rs` — struct, `new`/`start`/`run`, `handle_event`, `execute`, `sync_views`, copy/paste, `handle_app_event`, crash snapshots, and the `#[cfg(test)]` driving helpers (`test_start`, `press`, `type_text`, `mouse`, `handle_test_event`, `settle`, `quitting`).
- `files.rs` — open (off-thread `Buffer::open` → `AppEvent::Opened`), `add_buffer` (1-based `goto`), tab activation/cycling, `focus_group`, `split`, `close_tab`/`discard_tab`, save/`save_done`, quit.
- `prompt.rs` — `Prompt` (modal: `SaveBeforeClose`, `SaveAllBeforeQuit`, `Trust`, `Input`, `ConfirmDelete`), one-line `Input` field, `handle_prompt_key`, trust decisions.
- `side.rs` — explorer commands, directory listings, file ops (`FileOp` → `AppEvent::FileOpDone`), fs-change batching (`flush_fs_changes`), `.workspace` reload (`config_loaded`).
- `mouse.rs` — clicks via `HitMap` from the last frame, `ClickTracker` (double/triple), `Drag` (side border, split divider, text), wheel scrolls the group under the pointer without focusing it.

Things to know before editing:

- **Background work pattern:** clone `self.tx`, `tokio::task::spawn_blocking`, send an `AppEvent` variant, handle it in `handle_app_event`. In-flight sets (`listing`, `opening`) dedupe requests. Saves report `state` back so `finish_save` marks the right history state.
- `App::new` must start no background work (it once crashed by spawning before the runtime existed); put it in `start()`.
- `Focus` is `Editor` or `SidePanel`; `context()` maps it to the keymap `Context`. Starting with no file, or closing the last tab while the side panel is visible, focuses the side panel.
- A prompt swallows all keys and mouse events; `close_prompt()` re-opens a pending trust prompt (`trust_pending`). Esc/Enter/letter handling per prompt is in `handle_prompt_key`; unrecognised keys put the prompt back.
- Leaving a tab/group (`activate_tab`, `cycle_tab`, `focus_group`, `split`, editor click) calls `buf.seal_undo()` first.
- Closing: the save prompt appears only when the tab is the last one (in any group) showing a dirty buffer (`groups.tabs_showing`). `discard_tab` drops the buffer when no tab shows it and closes an empty group (never the last). `AfterSave::{Quit, Close}` continues a close/quit once saves finish.
- `sync_views` runs after every input event: the editing tab only re-anchors its viewport; every other view on the buffer runs `View::map_through`.
- `update_crash_snapshot` runs every loop turn so the panic hook can back up dirty buffers.
- Messages: `notify(Level, text)` sets the status-bar message and logs it; any key press clears it.
- File system events are debounced (`FS_DEBOUNCE`, 100 ms) via `fs_deadline`; explorer renames/deletes update open buffers' `path`/`deleted`.
