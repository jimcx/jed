# app

`App` (in `mod.rs`) owns all state; submodules are `impl App` blocks split by area, with `pub(super)` methods:

One line per submodule; [docs/dev/app.md](../../docs/dev/app.md) has each in full (state, functions, events, rules) — read the entries for what you touch, and add detail there, not here (the root CLAUDE.md's budget applies: a line or two per change).

- `mod.rs` — the `App` struct, `new` / `start` / `run`, `handle_event`, `execute`, `sync_views`, copy / paste, `handle_app_event`, `render_whitespace()`, and the `#[cfg(test)]` driving helpers (`test_start`, `press`, `type_text`, `mouse`, `settle`, `pump`, `test_execute`, the `test_*_tick`s, …: listed in docs/dev/app.md)
- `field.rs` — `field_key`: keys in a text field (find widget, Search view inputs, palette, prompts) — typed, else the editor context's moves / deletes / select all / copy / cut / paste run on the field (`has_field_meaning`)
- `files.rs` — opening (off-thread, `opening` map of `OpenRequest`s, `open_file(path, focus, goto)`), saving, closing tabs, `add_buffer`
- `prompt.rs` — `Prompt`, the modal dialogs (save before close / quit / stage, trust, input, confirm delete / replace, restore backups)
- `side.rs` — explorer commands, directory listings, file ops, fs-change batching (`flush_fs_changes`), the side panel
- `navigation.rs` — back / forward (`app.nav`): `nav_begin` / `nav_end`, `nav_jump`, Go Back / Forward
- `palette.rs` — opening, keys and accepting the command palette and its pickers
- `cheatsheet.rs` — the keyboard cheat sheet overlay (`app.cheat_sheet`): toggle, its keys (all swallowed) and mouse
- `cursors.rs` — Cmd+D / Cmd+Shift+L / Cmd+Alt+D sessions, Cmd+U (`cursor_undo`), `track_selections`
- `find.rs` — the find / replace widget (`app.find`): open, incremental search, next / previous, replace (all); on a hex tab byte search (`find.hex`, the count off-thread via `AppEvent::HexCount`)
- `workspace_search.rs` — the Search view (`app.search_view`): focus, debounce, starting searches, results, opening matches
- `replace.rs` — the Search view's Replace (`app.replace`): a line / file / all, the confirm, open buffers edited, other files rewritten off-thread with history, the message and the search again
- `syntax.rs` — buffers' language detection and `sync_syntax` (incremental / background parses)
- `lsp.rs` — `sync_servers`, trust gating, `send_request` / `Pending` / `answer`, diagnostics into buffers, server notices
- `indentation.rs` — the status bar's indentation item, Indent Using Spaces / Tabs, Change Tab Display Size (`Picker::IndentSize`), Detect Indentation from Content
- `problems.rs` — the bottom panel and its Problems list (Cmd+J, F8 / Shift+F8)
- `hover.rs` — hover popup (command and mouse rest): the diagnostics there at once, the server's answer appended, closing
- `completion.rs` — completion: trigger / quick suggestions, answers, the list's keys, accepting
- `code_action.rs` — code actions: Cmd+., the list, applying (resolve, edits, commands), server `workspace/applyEdit`, the lightbulb, the hover's quick fixes
- `locations.rs` — go to definition and references, the Locations list
- `rename.rs` — rename symbol (F2): prepare, prompt, applying the `WorkspaceEdit` across files (`apply_workspace_edit`, shared with code actions)
- `format.rs` — format document
- `git.rs` — the git status (`app.git`): `refresh_git` off-thread, one at a time, queued and merged; what follows a new status
- `git_history.rs` — the Explorer's "Git History" section: following the active file, pages, keys, opening commits
- `disk.rs` — open files changed on disk (`app.disk`): checks, reloads, the banner's actions; hex tabs' re-reads
- `quick_diff.rs` — gutter markers (`sync_quick_diff`), their peek, staging / unstaging from the gutter
- `blame.rs` — inline blame (`app.blame`): toggle, `sync_blame`
- `diff_view.rs` — diff tabs (`app.diffs`): `open_diff(path, Pair)`, `sync_diffs`, `diff_command`, stage / revert hunks
- `hex.rs` — hex tabs: binary files' load (`Loaded::Binary`), one tab per path, Reopen as Hex / Text, `hex_command` / `hex_copy`, Go to Offset, `hex_mouse`
- `commit_tabs.rs` — commit tabs: `sync_commit_tabs`, `fit_commit_tabs`, opening a stat row's diff
- `scm.rs` — the Source Control view (`app.scm`): focus, keys, stage / unstage, opening diffs
- `history.rs` — file history (`app.file_history`): recording after saves, moving on renames, Show / Restore / Compare
- `tasks.rs` — tasks (`app.tasks`): templates, running, task tabs, keys / paste to a task, events
- `shell.rs` — shell tabs (`Tab::Shell`): "Terminal: New Shell" opening `$SHELL` in the active group with `JED_IPC`, its title and its run's events (shell-b draws it)
- `session.rs` — sessions (`app.session`): restore at start, `save_session` on quit
- `backup.rs` — crash backups (`app.backups`): `sync_backups`, `backup_tick`, the offer and restores, `update_crash_snapshot`
- `tools.rs` — tool logs and log tabs (`app.logs`, `app.log_tabs`), the Tools view, trace / restart commands
- `recent.rs` — recent workspaces (`app.recent`): record at start, Ctrl+R's picker
- `scrollbar.rs` — scroll bars: `with_scrollable(target, f)`, clicks, drags, hover
- `mouse.rs` — clicks, drags and the wheel via the last frame's `HitMap`; `ClickTracker`, `Drag`
- `mode.rs` — `Mode::{Workspace, SingleFile}` (`app.mode`, `jed -s`): `allows(cmd)` / `allows_view(panel)`, the one availability check (`execute`, palette, activity bar, icon clicks)

Things to know before editing:

- **Background work pattern:** clone `self.tx`, `tokio::task::spawn_blocking`, send an `AppEvent` variant, handle it in `handle_app_event`. In-flight sets (`listing`, the `opening` map) dedupe requests. Restartable work (quick open's walk, workspace search) carries a generation number so results of a replaced run are dropped, and a cancel flag (`Arc<AtomicBool>`; workspace search's `Handle` sets it on drop). Saves report `state` back so `finish_save` marks the right history state.
- `App::lsp` gets its event sink in `start()`; no server is spawned before that (`add_buffer` only marks `servers_dirty`).
- `App::new` must start no background work (it once crashed by spawning before the runtime existed); put it in `start()`.
- `Focus` is `Editor` (`Context::CodeActions` while the code action list is shown, `Context::Suggest` while the completion list is: its keys first, then the editor's; `Context::Diff` while the active tab is a diff, `Context::Hex` while it's a hex view, `Context::Commit` while it's a commit, likewise), `SidePanel`, `Find` (the find widget's input; `find.field` picks `Context::Find` or `Context::Replace`) or `Panel` (the bottom panel; `ui.panel_tab` picks the context: `Problems`, `Locations` or `Task`; unbound keys and paste do nothing there, except in a task tab: its task gets them); `context()` maps it to the keymap `Context` (`SidePanel` is `Explorer`, or `Search` / `SearchResults` (by `search_view.list_focused`) when the side panel shows the Search view: unbound keys and paste then edit its focused input (query, replace, files to include or exclude), never the buffer; `focus_side` gives that input the keys) (`Palette` whenever `app.palette` is open; the palette doesn't change `Focus`); with the Source Control view shown it is `Scm` (unbound keys and paste do nothing); with the Explorer shown and its Git History section focused (a click or the header) it is `GitHistory` (unbound keys and paste do nothing). The find widget stays shown when focus leaves it (an editor click sets `Focus::Editor`); Esc in the editor then closes it before dropping cursors. Starting with no file, or closing the last tab while the side panel is visible, focuses the side panel.
- The palette (`app.palette`) is not a `Prompt`: its keys go through the keymap's `Palette` context, unbound keys edit its `TextField`, and while it's open it takes every mouse event (click outside closes it). A prompt opened on top of it gets keys first. `execute` closes it before running any other command.
- Quick open: `FileMatcher`'s notify callback sends `AppEvent::FilesMatched` at most once until handled (`files_pending`); handling it calls `tick()` (never waits) and rebuilds rows; while `tick()` says matching still runs, `files_poll` ticks again (nucleo can lose the wake-up for that run). `FilesWalked { generation }` marks the walk done unless a newer walk started.
- `set_keymap(keymap, errors)` (from `main`) replaces the keymap and shows the first load error (`report_errors`: `settings.toml`'s and `keybindings.toml`'s, `(and N more)`); `log` (reload handle, `None` if logging is off) and `log_dir` are set by `main` / tests; `config_dir` (`paths::config_dir()`, where "Open User Settings" looks) and `state_dir` (`Some(paths::state_dir())` outside tests, `None` in them: sessions off unless set) by tests.
- A prompt swallows all keys and mouse events; `close_prompt()` re-opens a pending trust prompt (`trust_pending`), else a pending backup offer (`offer_backups`). Esc/Enter/letter handling per prompt is in `handle_prompt_key`; unrecognised keys put the prompt back.
- Leaving a tab/group (`activate_tab`, `cycle_tab`, `focus_group`, `split`, editor click) calls `buf.seal_undo()` first.
- Closing: the save prompt appears only when the tab is the last one (in any group) showing a dirty buffer (`groups.tabs_showing`); a diff tab closes without asking. `discard_tab` drops the buffer when no tab shows it and closes an empty group (never the last). `AfterSave::{Quit, Close}` continues a close/quit once saves finish.
- `sync_views` runs after every input event: the editing tab only re-anchors its viewport; every other view on the buffer runs `View::map_through`. Then `track_selections`: if the active view's buffer changed its Cmd+U history is reset, else a selection change pushes the previous list; any selection change not made by a match command (`session_changed`), or focus leaving the editor (palette, prompt, side panel, find widget), ends the Cmd+D session (VS Code's rule; word mode resets with it). Only input events are tracked, so selections set by background results are recorded at the next key.
- `find_options` (`search::Options`: case / whole word / regex, and `hex`: the byte search's mode on a hex tab) belongs to the find widget's toggles and is what Cmd+D uses for selected text (case and whole word only); toggling ends a Cmd+D session that uses them (not word mode).
- The bottom panel: `ui.panel_visible`, `ui.panel_height` (`PANEL_HEIGHT` = 10 until dragged; border and tab strip included), `ui.panel_tab` (`PanelTab::Problems` / `Locations` / `Task(id)` — a closed run's tab is never left there: `close_task`; `focus_panel(tab)` selects the tab's first row when nothing is). Cmd+B only moves the focus to the editor when the side panel had it.
- `update_crash_snapshot` (`app/backup.rs`) runs every loop turn so the panic hook can back up dirty buffers (into `state_dir`; nothing without one).
- Messages: `notify(Level, text)` sets the status-bar message and logs it; any key press clears it.
- Timers: `next_timer()` sleeps until the earliest of `fs_deadline`, `search_view.deadline()`, `hover.deadline()` (the mouse's rest, `hover::DELAY` 300 ms), `completion.deadline()` (quick suggestions, `completion::QUICK_DELAY` 10 ms), `code_actions.deadline()` (the lightbulb's rest, `code_action::BULB_DELAY` 250 ms), `quick_diff.deadline()` (edited buffers' gutter markers, `quick_diff::DELAY` 150 ms), `diffs.deadline()` (diff tabs of edited buffers, `diff_view::DELAY`), `blame.deadline()` (edited buffers' inline blame, `blame::DELAY` 300 ms), `backups.deadline()` (crash backups, `backup::INTERVAL` 30 s), `log_tabs.deadline()` (log tabs' wake throttle, `tools::LOG_FRAME` 20 ms) and `files_poll` (quick open's matcher still running at its last tick, `FILES_POLL` 10 ms) (forever when none is set; the future borrows nothing) and `on_timer` runs it (`flush_fs_changes` / `search_tick` / `hover_tick` / `completion_tick` / `bulb_tick` / `sync_quick_diff` / `sync_diffs` / `sync_blame` / `backup_tick` / `logs_tick` / `files_matched`). `run`'s `select!` and `settle` both use this pair, so a new deadline goes there, not in a new `select!` branch.
- The Search view's query is debounced (`search_view::DEBOUNCE`, 150 ms) via `search_view.deadline()`; starting a search drops the running one's `Handle`.
- File system events are debounced (`FS_DEBOUNCE`, 100 ms) via `fs_deadline` (those under `<root>/.jed/` dropped first, `fs_changed`); explorer renames/deletes update open buffers' `path`/`deleted`.
