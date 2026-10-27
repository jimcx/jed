---
id: live_reload_settings
name: Live reload of settings
summary: `settings.toml` and `keybindings.toml` reloaded live when they change (`.workspace` already was)
status: done
priority: 0
dependencies: []
---

  - `Watcher::watch_dir` (`src/watcher.rs`) is a non-recursive watch of the config dir (only `settings.toml` / `keybindings.toml`, not `languages/`); `App::watch_config_dir` adds it in `start` and `start_single`, skipping a missing dir and retrying once "Open User Settings" or the Keyboard Shortcuts stub creates a file; `start_file` forwards the config dir's children too, so single-file mode reloads as well
  - `flush_fs_changes` compares each flushed path with `config_dir/settings.toml` and `config_dir/keybindings.toml`, reloads off-thread and sends `AppEvent::SettingsLoaded` / `KeymapLoaded`
  - `settings::reload` / `Keymap::reload` return `Err` when the file can't be read or parsed at all, so the last good settings / keymap stay and the error is reported (per-key / per-binding problems still apply the rest); `parse` / `load` split to share `table` / `from_text`
  - `App::settings_loaded` replaces `workspace.user_settings`, re-layers `settings = workspace.settings()` (`.workspace` still wins per key), swaps `lsp.builtins.user` and sets `servers_dirty`; `App::keymap_loaded` goes through `set_keymap`; both say "Reloaded …" only on a clean load
  - `TEMPLATE` and `KEYBINDINGS_TEMPLATE` no longer claim "next time jed starts"; the Keyboard Shortcuts stub notice is "Keybinding written — reloaded live"
  - Tests: `todo_tests::live_reload` — settings apply live and layer under `.workspace`, a rebind, a bad file keeping the last good, `[lsp.*]` servers updating, and single-file mode
  - Also: `servers_dirty` is set so a `[lsp.*]` change restarts servers; cursor shape, render-whitespace, rulers and scrollback read `self.settings` at use time and self-correct
  - Deferred: two writes >100 ms apart can apply out of order (no generation guard, same parity as `.workspace`)
  - Still to check by hand: an editor that saves a config file by renaming over it (an atomic save), in a real terminal
