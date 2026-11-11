---
id: claude_lsp
name: Language server queries for Claude
summary: "MCP tools that ask jed's running language servers on Claude's behalf: definition, references, hover, a file's symbols and workspace symbols, answered without moving the user's cursor or opening a popup"
status: in-progress
priority: 0
dependencies: []
---

See [Claude Code integration](claude_ide.md) and [LSP](../PLAN.md#lsp). jed's servers are already started, indexed and in step with the buffers; today Claude only reads their diagnostics and greps for everything else.

  - Needs: the MCP server and its queries (`claude.rs` `tools()` / `tool_query`, `ipc.rs` `Query`, `app/ipc.rs` `ipc_request`, the builders in `app/claude.rs`), an answer given later than the request (`claude_review_act`'s `Responder` hand-over), `App::send_request` / `Pending` / `check_requests` (`app/lsp.rs`, the [language server requests](../docs/dev/conventions.md#language-server-requests) convention), the location parsing and off-thread line reads of the references list (`app/locations.rs`), `lsp::progress`, background file loads (`OpenRequest { background }`, `open_background`), the fake server (`lsp/fake.rs`)
  - Tools (all read-only; none needs the review's `mcp_read_only`):
    - `find_definition(path, line, symbol?, col?, kind = "definition" | "type" | "implementation" | "declaration")`
    - `find_references(path, line, symbol?, col?, include_declaration = true)`
    - `get_hover(path, line, symbol?, col?)`: the hover's text (markdown as sent, capped at 4 KB) and the range it covers
    - `get_document_symbols(path)`: the file's symbols as a tree (name, kind, detail, 1-based line range, children), capped
    - `find_workspace_symbols(query, max = 50)`: name, kind, container, path and line
  - Naming a place: `line` is 1-based; `symbol` is the name to look up, found as the first whole-word match on that line, so Claude never counts columns (jed's `col` is a display column: tabs widen it). `col` (as `get_active_editor` reports it) picks the match at or after it when the name is there twice; with neither, the line's first non-blank. A `symbol` that isn't on the line is an error quoting the line's text as jed has it, which also shows a buffer that differs from the file on disk. The reply says `dirty: true` when the buffer has unsaved changes (the lines are the buffer's, not the file's)
  - IPC: `Query::Lsp { ask: LspAsk, path, line, symbol, col, .. }`, `Query::Symbols { path }`, `Query::WorkspaceSymbols { query, max }`. `ipc_request` hands the `Responder` to `app/claude_lsp.rs`, as it does for the review actions; `claude_answer` refuses them ("not a read from loaded state")
  - Requests (`app/lsp.rs`): a `Request::Claude { ask }` kind, where `ask` numbers the waiting responder. Unlike the user's kinds:
    - a cursor move, a tab switch or a newer request never cancels it, and several may be in flight (`send_request`'s "newer cancels older of the same kind" is per `ask`)
    - an edit to the buffer cancels it and answers "the file changed while asking: ask again"
    - it times out after 10 s (one timer for the earliest deadline) with `$/cancelRequest`; the connection closing cancels its requests too
    - a routine error (`RpcError::is_routine`: ContentModified while a server loads) is asked again after 500 ms until the deadline
    - nothing the user sees changes: no selection, scroll, popup, Locations list, back / forward entry or `notify`. The traffic is in the "claude (mcp)" log as for every tool call
  - Which server: the first running server of the file that advertises the method, as the user's command picks. Errors name the cause: no server for the language, not started or crashed (with the Tools view's state), the capability missing, timed out. While the server reports work (`$/progress` begun), the answer carries `server_busy: "<label>"`: results may be incomplete while it indexes
  - A file that isn't open: loaded through the `opening` map as a background tab (the server needs the document open; the user sees what Claude asked about), the request sent once the server has it; the reply says `opened: true`. A path jed can't load as text (missing, binary, too big) is an error
  - Answers: a location is `{path, line, col, end_line, end_col, text}`: the path relative to the workspace when inside it, `text` the line clipped to 200 bytes. Lines of files that aren't open are read off the UI thread, as the Locations list reads them (one `spawn_blocking` per answer, the answer given when it reports). At most 200 locations, with `count` and `truncated`; references are sorted by path then line. `LocationLink` and `Location` shapes both read (the code `app/locations.rs` has)
  - Symbols: `documentSymbol` (both shapes: the `DocumentSymbol[]` tree and flat `SymbolInformation[]`, the flat one nested by `containerName`) and `workspace/symbol` are parsed in a model module `symbols.rs` with no app state, which [Language features](lang.md) (lang-a, lang-b) then builds its quick open modes on. If lang-a has landed first, use its model instead of adding one
  - Hook text: the session-start line on what the tools offer mentions them (one line; `claude.rs` `hook_text`)
  - Deferred:
    - `rename_symbol` (a write across files: needs the review tools' kind of consent setting, and saving what it changed)
    - call and type hierarchy, code actions, formatting, completion, signature help
    - asking about a file without a tab (a document open on the server with no buffer)
    - a tree-sitter fallback when the language has no server
    - asking every server of a file and merging
  - Done when:
    - with the fake server and a scripted MCP client (`run_mcp` / `call`): `find_definition` with `line` + `symbol` returns the scripted location with its line text; `find_references` returns locations in two files, one not open, sorted, with `count`; `get_hover` returns the text; none of them moves the selection, changes the active tab or adds a back / forward entry
    - a `symbol` not on the line is an error quoting the line; a dirty buffer answers with `dirty: true` and the buffer's lines
    - moving the cursor and switching tabs while a request waits doesn't cancel it; an edit to the file answers "ask again"; a server that never answers gives a timeout error after the deadline (the test shortens it) and a `$/cancelRequest`; a ContentModified error is asked again and then answered
    - a path with no tab opens a background tab, and the answer says `opened: true`; a language with no server, and a server without the capability, each give their error
    - `get_document_symbols` returns the tree for both server shapes; `find_workspace_symbols` finds a struct in another file
    - with real rust-analyzer on a small cargo project (skipped with a note when absent): definition of a function called from another file, its references, and a workspace symbol
    - the "claude (mcp)" log shows each call and its answer's size
  - Subtasks (one agent and one commit each):
    - ✅ claude_lsp-a: the plumbing and three tools on open files: `Query::Lsp`, `app/claude_lsp.rs` (resolving `line` / `symbol` / `col`, the waiting responders), `Request::Claude` with its own cancel rules, the timeout and the retry, `find_definition` / `find_references` / `get_hover` in `claude.rs`, locations with line text from open buffers; unit tests against the fake server
      - Done: `app/claude_lsp.rs` (`claude_lsp_ask` resolves the place and sends, `claude_lsp_response`, `claude_lsp_check` for edits / stopped servers / closed clients, `claude_lsp_tick` for the timeout and the retry on one `Timer::ClaudeLsp`; `State::timeout` / `retry` shortened in tests); `Request::Claude { ask }` in `app/lsp.rs` (`request_current` always true, staleness handled by `claude_lsp_check`); `ipc::LspAsk`, `Query::Lsp`, test-only `Responder::pair()`; the fake advertises type definition / implementation / declaration and gains `unfail(method)`
      - For b: a file not open is the error "<path> is not open in jed: open it first"; locations in files not open get `text: null`; no server / not running / no capability are one generic error naming the method; every answer carries `at` and `dirty`
    - ✅ claude_lsp-b: files that aren't open: the background tab load before asking, line text for locations in files that aren't open (off-thread), `server_busy`, the no-server / not-running / no-capability errors
      - Done: a file not open loads as a background `OpenRequest` (the query waits in `State::loading`; `claude_lsp_check_loading` from `claude_lsp_check` and the end of `files.rs` `opened`; `claude_lsp_load_result` at its top turns a failed or binary load into the query's error and drops a bare background open of a missing file instead of making a new-file tab); `claude_lsp_why` names the cause, and not-ready causes (server starting, document not open yet) park the query until it runs or the deadline (the timeout names why); locations in files not open are read in one `spawn_blocking` per answer (`IpcEvent::ClaudeLspRead`, `claude_lsp_read`); `server_busy` from the answering server's `progress.latest()`; the fake gains `hold_init`
      - Left: a server failing while a request is in flight answers the generic "stopped before answering"
    - ✅ claude_lsp-c: symbols: `symbols.rs` (both `documentSymbol` shapes, `workspace/symbol`), `Query::Symbols` / `Query::WorkspaceSymbols`, the two tools, the fake server's scripted answers
      - Done: `src/symbols.rs` (`document`: both shapes, flat ones nested under the nearest earlier container of that name whose range holds them; `workspace`: `SymbolInformation[]` / `WorkspaceSymbol[]` → `WorkspaceItem`, `start: None` for a `{uri}`-only location; `kind_name`) for lang-a / lang-b to build on; `Query::Symbols` / `Query::WorkspaceSymbols` (`LspAsk` has internal `DocumentSymbols` / `WorkspaceSymbols` variants, refused as a `Query::Lsp`); document symbols take the a/b path through `send_doc_request` (tree capped at 500, depth-first, `count` the total); workspace symbols (`claude_lsp_workspace`) go to the first running server advertising them, the active file's first, through `send_server_request`, never stale on an edit, answered even when their anchor buffer closed; jed now declares hierarchical `documentSymbol` and `workspace.symbol`; the fake advertises both (`no_symbols` drops them)
      - Workspace symbols' line / col in files not open are the server's line / character + 1 (no disk read)
    - claude_lsp-d: the done-when tests (`src/claude_lsp_tests.rs`, the real rust-analyzer case), `docs/claude.md`, the module map and the conventions line on `Request::Claude`, the hook line, gaps and hand checks with real `claude` ("where is this used?" answered by `find_references`, not grep); marks the task ✅
