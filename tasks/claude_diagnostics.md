---
id: claude_diagnostics
name: Fresh diagnostics for Claude
summary: "`get_diagnostics` says whether the servers have caught up with each file, can wait until they have and can cover a file that isn't open; a PostToolUse hook tells Claude the new errors in a file it just edited"
status: open
priority: 0
dependencies: []
---

See [Claude Code integration](claude_ide.md) and [Live diagnostics](live_diagnostics.md). Today `get_diagnostics` returns whatever marks the buffers hold: after Claude writes a file there is no telling whether they are for the new text, a file with no tab is "not open", and Claude only learns of an error it made if it thinks to ask.

  - Needs: `claude_diagnostics` (`app/claude.rs`), `Query::Diagnostics` (`ipc.rs`), answers given later than the request (`claude_review_act`'s `Responder` hand-over; the waiters and `ipc_sweep` of `app/ipc.rs`), the disk checks and reloads of [files changed on disk](../docs/dev/conventions.md#features) (`disk_paths_changed`, `Stamp`), document versions (`lsp/documents.rs`), `Published::version` and `Manager::publish` (`lsp/diagnostics.rs`), pull diagnostics' per-buffer `Doc` (`app/diagnostics_pull.rs`), `lsp::progress`, background file loads (`OpenRequest { background }`), the hook CLI (`claude.rs` `parse_cli`, `hook_text`) and the plugin's `hooks/hooks.json`
  - Why it isn't already right: Claude's edit lands on disk; jed's watcher sees it, checks the buffer's `Stamp` off-thread, reloads a buffer with no edits, sends `didChange`, and the server answers some time later. A read at any point before that returns the old marks with nothing to say so
  - Freshness, per file and server (`app/claude_fresh.rs`, read-only over existing state plus one small record per document):
    - `disk`: `same` / `checking` (an fs event or a disk check for it is pending, or a reload is in flight) / `changed` (the buffer has edits and the file differs: jed kept the user's text) / `deleted`
    - per server with the document open: `checked` = `current` / `stale` / `never`. A pulling server is current when its last report was for the buffer's revision. A publishing server is current when it has published for the document's current version; one that publishes without a version, when it has published since the last `didChange` / `didSave` jed sent it (the record: the document version at the last publish)
    - `busy`: the server's `$/progress` label while work is begun (`cargo check` after a save)
    - a file is `settled` when `disk` is `same`, every server is `current` and not busy, and 300 ms have passed since jed last sent that server a change or save for it (a server shows no progress in the gap between a save and its check starting)
  - Check first, and fix if needed: what a reload from disk sends. rust-analyzer runs `cargo check` on `didSave`, so a clean buffer reloaded after an outside write must send `didSave` after its `didChange` (the buffer's text is the saved text), or the check's marks stay those of the old file
  - `get_diagnostics(path?, wait_ms = 0, open = false)`:
    - the reply gains `files: [{path, open, dirty, disk, servers: [{name, state, checked, busy}], settled}]` for each file asked about (with no `path`: every open file with marks or not settled), and a top-level `settled`
    - `wait_ms` (at most 10000): the answer is held until every file asked about is settled or the time is up (`settled: false`, with the state that kept it waiting). Held answers are swept after each event, as `ipc_sweep` does, with one timer for the earliest deadline; a closed connection drops its waiter. Nothing blocks the UI thread
    - a file with no tab: an error naming `open: true`; with it, the file loads as a background tab through the `opening` map, the servers get it, and the answer waits as above (a load failure is the error). `opened: true` in the reply
    - a dirty buffer is reported as it is (`dirty: true`, `disk: changed` when the file differs): its marks are for the user's unsaved text, not the file Claude reads
    - the items, caps and errors-first order are unchanged
  - The hook, `jed claude hook post-edit` (PostToolUse, matcher `Edit|Write|MultiEdit|NotebookEdit` in `hooks/hooks.json`):
    - reads the hook's JSON on stdin (`tool_input.file_path`, `cwd`); anything it can't read, outside jed, or a file with no tab in jed (it never opens one): prints nothing, exit 0
    - asks `Query::Diagnostics { path, wait_ms: 2000, hook: true }` and prints, as PostToolUse's `hookSpecificOutput.additionalContext` JSON (check the shape against Claude Code's hook reference when building), only errors, only the ones new since the last hook report for that path: at most 10 lines `path:line:col: message [source code]`, 2 KB in all
    - "new": the app keeps, per path, the last hook report as a set of (message, code, text of the line), so an error that only moved isn't repeated and one that was fixed and came back is; cleared when the buffer closes
    - not settled in time: one line, `diagnostics for <path> are still being checked: call get_diagnostics with wait_ms`, only if the file has a server
    - a dirty buffer whose file Claude just changed: one line, `jed has unsaved changes to <path>: the editor does not show your edit`, so Claude tells the user instead of going on
    - the whole hook returns within 2.5 s whatever jed does (the IPC read has a deadline), and is logged as the other hooks are (`hook post-edit → N lines`)
    - a setting `claude_edit_diagnostics` (default `true`, the [settings convention](../docs/dev/conventions.md#settings)): off, the app answers the hook with nothing
  - The session-start hook's line on the tools mentions `wait_ms`
  - Deferred:
    - spelling marks in the tool's answer
    - warnings in the hook; a per-severity setting
    - diagnostics for a file with no tab without opening one; workspace diagnostics (`workspace/diagnostic`)
    - blocking the edit (a PreToolUse hook) when the buffer is dirty in jed
    - reading a dirty buffer's text over MCP
  - Done when:
    - with the fake server and a scripted MCP client: a file is written on disk behind an open clean buffer; `get_diagnostics(path, wait_ms)` sent at once answers only after the reload and the server's publish for the new version, with the new marks and `settled: true`; the same call with `wait_ms = 0` in between says `settled: false` with `disk: checking` or `checked: stale`
    - a server that never publishes: the answer comes at the deadline with `settled: false`; a server showing `$/progress` holds the answer until its `end`; a pulling server settles on its report for the current revision
    - the reload sends `didChange` then `didSave` to the fake server
    - a path with no tab: an error naming `open: true`; with it, a background tab opens and the scripted diagnostics come back
    - a dirty buffer whose file changed on disk reports `dirty: true` and `disk: changed` without waiting
    - `jed claude hook post-edit` with a hook JSON on stdin: prints the new error after an edit that adds one, nothing when run again with no change, the error again after it was fixed and re-made; nothing for a file with no tab, for warnings only, outside jed, and with `claude_edit_diagnostics = false`; the dirty-buffer line when jed has unsaved changes; it returns inside its budget when the server never answers
    - `hooks/hooks.json` passes `claude plugin validate`, and `claude_ide_tests`' check of the plugin's commands against the CLI covers `post-edit`
    - the waiters are gone after their connection closes (`test_ipc_waiters`)
  - Subtasks (one agent and one commit each):
    - claude_diagnostics-a: freshness: the per-document record, `disk` / `checked` / `busy` / `settled` (`app/claude_fresh.rs`), the `files` list in `get_diagnostics`' reply, `didSave` after a reload if the check shows it missing; unit tests with the fake server for a publishing, a versionless and a pulling server
    - claude_diagnostics-b: waiting and opening: `wait_ms` (held answers, the sweep, the deadline timer, closed connections), `open: true` through a background load, the tool's schema and description in `claude.rs`
    - claude_diagnostics-c: the hook: `jed claude hook post-edit` (stdin JSON, the output shape, its own deadline), the per-path last report in the app, the dirty-buffer and still-checking lines, `claude_edit_diagnostics`, `hooks/hooks.json`, the log line
    - claude_diagnostics-d: the done-when tests (`src/claude_diagnostics_tests.rs`), `docs/claude.md` (the hook, the setting, the no-plugin `settings.json` snippet), the module map and conventions lines, gaps and hand checks with real `claude` and rust-analyzer (break a function's call site with an edit: the error arrives with the edit's result; how long the hook takes on a save that starts `cargo check`); marks the task ✅
