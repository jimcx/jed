# app

`App` (in `mod.rs`) owns all state; submodules are `impl App` blocks split by area, with `pub(super)` methods:

Summary only: [docs/dev/app.md](../../../docs/dev/app.md) has each submodule in full (state, functions, events, rules) and the full form of the notes at the end — read the entries for what you touch, and add detail there, not here (a line or two per change).

- `mod.rs` — the `App` struct, `new` / `start` / `run`, `handle_event`, `execute`, `sync_views`, copy / paste, `handle_app_event`, `render_whitespace()`, and the `#[cfg(test)]` driving helpers (`test_start`, `press`, `type_text`, `mouse`, `settle`, `pump`, `test_execute`, the `test_*_tick`s, …: listed in docs/dev/app.md)
- `config.rs` — `ConfigEvent` (`AppEvent::Config`: the workspace config / settings / keymap re-reads and the `*Ready` files made for editing them) and `handle_config_event`.
- `field.rs` — `field_key`: keys in a text field (find widget, Search view inputs, palette, prompts) — typed, else the editor context's moves / deletes / select all / copy / cut / paste run on the field (`has_field_meaning`)
- `claude.rs` — answers to the Claude Code integration's IPC queries (`claude_answer`: context, editors, diagnostics, terminals, commands, output, screen, environment), capped, from loaded state only
- `files.rs` — opening (off-thread, `opening` map of `OpenRequest`s, `open_file(path, focus, goto)`, which records a successfully opened file in `mru`), saving, closing tabs, `add_buffer`
- `prompt.rs` — `Prompt`, the modal dialogs (save before close / quit / stage, trust, input, confirm delete / replace / stage-a-conflicted-file, restore backups, the shell integration wizard)
- `side.rs` — explorer commands, directory listings, file ops, fs-change batching (`flush_fs_changes`, which also reloads `.workspace`, `settings.toml` and `keybindings.toml`), the side panel
- `navigation.rs` — back / forward (`app.nav`): `nav_begin` / `nav_end`, `nav_jump`, Go Back / Forward
- `reopen.rs` — Reopen Closed Editor: the stack of closed file tabs (`app.closed`), reopening with cursor and scroll
- `palette.rs` — opening, keys and accepting the command palette and its pickers; a command picked there is recorded in `mru` (`App::mru.commands`), and an empty query lists the recent commands and files first (`palette::command_rows_with_recent` / `file_rows_with_recent`)
- `cheatsheet.rs` — the keyboard cheat sheet overlay (`app.cheat_sheet`): toggle, its keys (all swallowed) and mouse
- `keys.rs` — the Keyboard Shortcuts view (`app.keys`, model `crate::keys`): toggle, search, record-keys mode, Enter's `keybindings.toml` stub, keys and mouse
- `explorer_drag.rs` — dragging an Explorer row onto a folder (`Drag::Explorer`): the folder under the pointer shaded (`list.dropBackground`), a release moves it through `FileOp::Rename`, Esc cancels
- `inline_rename.rs` — F2 in the Explorer edits the row's name in place (`app.inline_rename`, a `TextField`): Enter or a click elsewhere commits through `FileOp::Rename`, Esc cancels
- `cursors.rs` — Cmd+D / Cmd+Shift+L / Cmd+Alt+D sessions, Cmd+U (`cursor_undo`), `track_selections`
- `find.rs` — the find / replace widget (`app.find`): open, incremental search, next / previous, replace (all); on a hex tab byte search (`find.hex`, the count and, on a paged doc, next / previous off-thread via `AppEvent::View(ViewEvent::HexCount` / `HexFound)`)
- `workspace_search.rs` — the Search view (`app.search_view`): focus, debounce, starting searches, results, opening matches
- `replace.rs` — the Search view's Replace (`app.replace`): a line / file / all, the confirm, open buffers edited, other files rewritten off-thread with history, the message and the search again
- `syntax.rs` — buffers' language detection and `sync_syntax` (incremental / background parses)
- `diagnostics_pull.rs` — pull diagnostics (`textDocument/diagnostic`, `app.pull`): per-buffer `Doc`s, the 300 ms rest timer, `resultId`s, refresh, related documents; answers go into `Manager::publish`'s store
- `lsp.rs` — `sync_servers`, trust gating, `send_request` / `Pending` / `answer`, diagnostics into buffers, server notices
- `indentation.rs` — the status bar's indentation item, Indent Using Spaces / Tabs, Change Tab Display Size (`Picker::IndentSize`), Detect Indentation from Content
- `problems.rs` — the bottom panel and its Problems list (Cmd+J, F8 / Shift+F8)
- `hover.rs` — hover popup (command and mouse rest): the diagnostics there at once, the server's answer appended, closing
- `tab_tip.rs` — the tab tooltip (500 ms rest on a tab, `Timer::TabTip`) and `path_text`, the one displayed-path helper shared with Copy Relative Path
- `completion.rs` — completion: trigger / quick suggestions, answers, the list's keys, accepting
- `context_menu.rs` — the context menu (`app.context_menu`): `open_context_menu`, `Context::ContextMenu` keys, mouse, dismissal
- `code_action.rs` — code actions: Cmd+., the list, applying (resolve, edits, commands), server `workspace/applyEdit`, the lightbulb, the hover's quick fixes
- `locations.rs` — go to definition and references, the Locations list
- `rename.rs` — rename symbol (F2): prepare, prompt, applying the `WorkspaceEdit` across files (`apply_workspace_edit`, shared with code actions)
- `format.rs` — format document
- `git.rs` — the git status (`app.git`): `refresh_git` off-thread, one at a time, queued and merged; `refresh_git_head` (the cheap branch read at startup), `ensure_status_loaded` (the full walk on demand, or after `STATUS_DEBOUNCE` in the background); what follows a new status
- `git_history.rs` — the Explorer's "Git History" section: following the active file, pages, keys, opening commits
- `disk.rs` — open files changed on disk (`app.disk`): checks, reloads, the banner's actions; hex tabs' re-reads
- `quick_diff.rs` — gutter markers (`sync_quick_diff`), their peek, staging / unstaging from the gutter
- `spell.rs` — the spell checker: dictionaries, background / incremental checks into `buf.spell.marks` (`ServerId::SPELL`), the half-typed rule
- `spell_cmd.rs` — the spell checker's palette commands (Next / Previous, Add to user / workspace dictionary, Ignore, Toggle, Open User Dictionary) and its quick fixes
- `blame.rs` — inline blame (`app.blame`): toggle, `sync_blame`
- `conflict.rs` — merge conflicts: `go_to_conflict(forward)` (Next / Previous Conflict) and `compare_conflict(at)` (Compare Current Conflict)
- `diff_view.rs` — diff tabs (`app.diffs`): the caret's keys, mouse (`Drag::Diff`) and copy / cut, `open_diff(path, Pair)`, `sync_diffs`, `diff_command`, stage / revert hunks
- `diff_edit.rs` — editing in diff tabs: `edit::run` on a scratch `View` at the caret's bytes of the file's buffer; a file not open opens in the background first (`DiffViews::pending`)
- `hex.rs` — hex tabs: binary files' load (`Loaded::Binary`), one tab per path, Reopen as Hex / Text, `hex_command` / `hex_copy`, Go to Offset, `hex_mouse`, paged docs' page reads / copy (`sync_hex_pages`)
- `markdown.rs` — Markdown preview tabs (`Tab::Markdown`): opening to the side, `markdown_command` / `markdown_copy` / `markdown_mouse`, `sync_markdown` (follows the source)
- `commit_tabs.rs` — commit tabs: `sync_commit_tabs`, `fit_commit_tabs`, opening a stat row's diff
- `scm.rs` — the Source Control view (`app.scm`): focus, keys, stage / unstage, opening diffs (a Merge Changes file opens the file)
- `discard.rs` — Discard Changes / Discard All (`Prompt::ConfirmDiscard`): the index's text back for tracked files (an open buffer edited, a closed file rewritten), untracked ones to the Trash
- `history.rs` — file history (`app.file_history`): recording after saves, moving on renames, Show (focuses the Explorer section) / Restore / Compare
- `tasks.rs` — tasks (`app.tasks`): templates, running, task tabs, keys / paste to a task, events, the terminal mouse selection's copy (`terminal_copy`, Cmd+C when one is selected)
- `shell.rs` — shell tabs (`Tab::Shell`) in the terminal area (`app.terminals`, `Focus::Terminal`): New Shell, titles, Install Shell Integration, `Context::Shell`'s scroll commands, keys / pastes and events of its run, `move_terminal`, `start_terminal`, `shutdown_shells`
- `term_search.rs` — the search bar over a shell or task tab (`app.term_search`, `Context::TermSearch`): every edit / step searches the whole scrollback and reveals the match
- `session.rs` — sessions (`app.session`): restore at start, `save_session` on quit
- `backup.rs` — crash backups (`app.backups`): `sync_backups`, `backup_tick`, the offer and restores, `update_crash_snapshot`
- `claude_lsp.rs` — Claude's language server queries (`find_definition` / `find_references` / `get_hover` / `get_document_symbols` / `find_workspace_symbols`): `Query::Lsp` resolved to a place, `Request::Claude`, the waiting responders, timeout and retry
- `claude_log.rs` — the MCP log (`Tool::Claude`): `→` / `←` / lifecycle lines from `jed claude mcp` log requests, the Tools row's sessions (`app.ipc.sessions`)
- `tools.rs` — tool logs and log tabs (`app.logs`, `app.log_tabs`), the Tools view, trace / restart commands
- `recent.rs` — recent workspaces (`app.recent`): record at start, Ctrl+R's picker
- `review.rs` — the Code Review panel (`app.review`): lookups (`review_refresh`, at launch / HEAD moves / shown / every 60 s while shown), `AppEvent::Review`, the file list's keys and mouse, the viewed marks
- `review_comment.rs` — review comments: the comment box under a review diff's selection (`app.composer`, `Context::Composer`), drafts saved in the state dir, the PENDING list's keys, drafts held back when their lines change
- `scrollbar.rs` — scroll bars: `with_scrollable(target, f)`, clicks, drags, hover
- `tab_drag.rs` — dragging a tab (`Drag::Tab`): reordering, moving between groups, edge drops that split, the shaded zone (`tab_drop_shade`), terminal-area groups and docking (`can_dock`)
- `mouse.rs` — clicks, drags and the wheel via the last frame's `HitMap`; a `Moved` event redraws only when its handler sets `needs_redraw`; a terminal program that asked for the mouse gets it (`terminal_mouse`), else a drag selects (`terminal_select`); word-wise drags after a double-click (`round_word_drag`, `round_run_drag`); `ClickTracker` (from `jtui::clicks`), `Drag`
- `mode.rs` — `Mode::{Workspace, SingleFile}` (`app.mode`, `jed -s`): `allows(cmd)` / `allows_view(panel)`, the one availability check (`execute`, palette, activity bar, icon clicks)

Things to know before editing (short form; in full at the end of docs/dev/app.md):

- **Background work:** clone `self.tx`, `tokio::task::spawn_blocking`, send an `AppEvent`, handle it in `handle_app_event`. Results are nested by area (`Files(FileEvent)`, `Config(ConfigEvent)`, `Git(GitEvent)`, …): a new result adds a variant to its area's enum and an arm in its `handle_<area>_event`, not a flat `AppEvent` variant. In-flight sets dedupe requests; restartable work carries a generation number and a cancel flag. Saves report `state` back for `finish_save`.
- `App::new` must start no background work (it can run before the runtime exists); put it in `start()`, which also gives `App::lsp` its event sink.
- `Focus` is `Editor`, `SidePanel`, `Find`, `Panel` or `Terminal`; `context()` maps it (with the active tab's kind, the shown side view and open lists) to the keymap `Context`. The palette doesn't change `Focus`: `Context::Palette` whenever it is open.
- The palette is not a `Prompt`; a prompt opened on top of it gets keys first, and `execute` closes it before any other command. A prompt swallows all keys and mouse events; `close_prompt()` re-opens a pending trust prompt, else a pending backup offer.
- Leaving a tab / group (`activate_tab`, `cycle_tab`, `focus_group`, `split`, editor click) calls `buf.seal_undo()` first.
- Closing: the save prompt appears only for the last tab (in any group) showing a dirty buffer; a diff tab closes without asking. `AfterSave::{Quit, Close}` continues once saves finish.
- `sync_views` runs after every input event (other views `map_through`), then `track_selections` (Cmd+U history; ends the Cmd+D session on a selection change not made by a match command, or when focus leaves the editor).
- **Timers:** `run`'s `select!` and `settle` both use `next_timer()` / `on_timer`, so a new deadline is one `(Option<Instant>, Timer)` line in the array in `next_timer`, not a new `select!` branch.
- Messages: `notify(Level, text)` sets the status-bar message and logs it; any key press clears it.
- `state_dir` is `None` in tests (sessions and crash backups off unless set); `config_dir`, `log`, `log_dir` are set by `main` / tests.
