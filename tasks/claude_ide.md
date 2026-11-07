---
id: claude_ide
name: Claude Code integration
summary: "`claude` in a jed shell sees the active file, recent shell commands and output, and the environment, via a Claude Code plugin (stdio MCP server + hooks) found by `$JED_IPC` with no state files; MCP traffic logged in the Tools view"
status: open
priority: 0
dependencies: []
---

  - Needs: the task socket and its `JED_IPC` injection (`ipc.rs`, `app/ipc.rs`, `Tasks::ipc`), the shell integration snippets and `TitleCallback`'s OSC 133 handling (`shell_integration.rs`, `tasks/run.rs`), the vt100 fork's `Screen::scrolled_lines` and prompt marks, the app's editor state (groups, tabs, `Selections`, `buf.diagnostics`), and the Tools view and its logs (`logs/`, `tools_view.rs`)
  - Uses only Claude Code's own infrastructure: a plugin holding a stdio MCP server (`jed claude mcp`) and two hooks (`jed claude hook session-start`, `jed claude hook prompt`). All three find the running jed through `$JED_IPC`, which every jed shell already inherits. It's process environment: there is no lock file, and nothing is written under `~/.claude` or used for discovery. Outside jed (`JED_IPC` unset, or a dead socket) the hooks print nothing and exit 0, and the tools return an error result ("not inside a jed terminal")
  - Discovery: shells and tasks get `JED_TERMINAL=<RunId>` next to `JED_IPC` (`tasks/mod.rs`), so jed knows which shell `claude` itself runs in. That shell is listed with a `self` flag, and its own output isn't fed back
  - IPC (`ipc.rs`): `Request` gains an optional `query`, a tagged enum: `Context`, `Editors`, `Diagnostics { path? }`, `Terminals`, `Commands { terminal?, n }`, `Output { terminal, command, max_lines }`, `Screen { terminal }`, `Environment`, `Open { path, line?, col? }`. A `log` request carries a log-only line (below). Old `{open, line, col, wait}` requests are unchanged, so `jed file` from a shell keeps working. `Reply` gains `data: Option<serde_json::Value>`. The server caps reply sizes and raises `MAX_LINE` for replies only. A connection may carry many requests: `serve_connection` loops over lines. The app answers on the UI thread (`app/ipc.rs`, builders in `app/claude.rs`). Each answer reads only what it needs: the primary selection's text (capped), at most `max_lines` of output, never a scan of the selection list
  - Shell commands (`shell_integration.rs`, `tasks/run.rs`):
    - The zsh / bash / fish snippets also send the full command line as `OSC 633;E;<escaped cmd>` (VS Code's convention), and the cwd as OSC 7 (`file://host/path`) from precmd
    - `TitleCallback` keeps a ring of each run's last 50 commands: `{ id, command, cwd, started, finished, exit, output rows }`. The rows are absolute (`Screen::scrolled_lines` + the cursor row) at `133;C` / `B` and at `133;D`
    - Output text is read from the scrollback on request. Lines already dropped from the scrollback are reported as truncated
    - An old snippet still works, just without command text (re-running "Install Shell Integration" updates it)
  - CLI modes (`main.rs` beside `Authoring`, before logging and raw mode; blocking std I/O as `ipc::client::open`; the code in `claude.rs`):
    - `jed claude mcp`: a minimal stdio JSON-RPC MCP server (`initialize`, `tools/list`, `tools/call`), hand-rolled on `serde_json` with no new dependency. It keeps one IPC connection for its lifetime, and each `tools/call` is one round trip on it. Tools:
      - `get_active_editor`: path, language, dirty, the 1-based cursor, the selection ranges and their text (capped)
      - `get_open_editors`, `get_diagnostics(path?)`, `get_environment`
      - `list_terminals`: name, cwd, running / idle, the last exit code, the `self` flag
      - `get_recent_commands(terminal?, n)`, `get_command_output(terminal, command_id, max_lines)`
      - `get_terminal_screen(terminal)`: the visible screen, for a TUI or a half-typed prompt
      - `open_file(path, line?, col?)`: opens it in the active group, as quick open does
    - `jed claude hook session-start` prints the environment as SessionStart context: jed's version, the workspace root(s), the git branch and its dirty count, the open files, the running language servers, the task templates and the active task environment, the shells, and one line on what the MCP tools offer
    - `jed claude hook prompt` prints a few lines of UserPromptSubmit context, kept cheap for every prompt:
      - `Active file: src/app/ipc.rs:42:5 (selection 40–48)`
      - the selected text when it's under ~2 KB
      - each other shell's last command line and exit code
      - Shell output is never pushed: it reaches Claude only through a tool call, which Claude Code's permission prompt covers
  - MCP log in the Tools view (the Tools and logs conventions):
    - `logs::Tool::Claude` ("claude (mcp)", file stem `claude-mcp`): the same ring, read-only log tab and rotated `<state dir>/logs/tools/claude-mcp.log` as every tool
    - `tools_view::Row::Claude { sessions, errors }` (+ `RowKey::Claude`), listed after the language servers: the number of connected `jed claude mcp` sessions and an error badge. Enter or a click opens its log
    - `jed claude mcp` forwards every JSON-RPC message it reads and writes as a log-only IPC line (`{"log": {"dir": "in" | "out", "msg": …}}`), so `initialize`, `tools/list`, `tools/call` and errors all show up. The app logs:
      - `→ [mcp <pid>] tools/call get_command_output {"terminal":2,"command":14}`
      - `← [mcp <pid>] 182 lines, 7.9 KB, 2 ms`, then the response JSON (cut at the ring's 64 KB line cap)
      - JSON-RPC errors at Error, which count toward the badge
      - a session connecting (pid, client name and version from `initialize`) and disconnecting
      - each hook call and the text it returned (`hook prompt → 3 lines`)
    - "Tools: Clear Log" / "Tools: Open Log" cover it with no change. It always holds the full traffic, so "Toggle Protocol Trace" doesn't apply
  - Plugin (`claude-plugin/`):
    - `.claude-plugin/plugin.json`
    - `.mcp.json`: `{"jed": {"command": "jed", "args": ["claude", "mcp"]}}`
    - `hooks/hooks.json`: SessionStart → `jed claude hook session-start`, UserPromptSubmit → `jed claude hook prompt`
    - A repo-root `.claude-plugin/marketplace.json`, so installing is `/plugin marketplace add jimcx/jed` (or a local path), then `/plugin install jed`
    - `docs/claude.md` covers installing it, and the no-plugin route: `claude mcp add -s user jed -- jed claude mcp` plus the two hooks in `~/.claude/settings.json`
    - "Claude: New Session" (`claude.newSession`, palette, unbound) opens a shell tab running `claude`
  - Deferred:
    - Claude Code's IDE protocol (a WebSocket MCP server found through `~/.claude/ide/<port>.lock`): `openDiff`, Claude's "In file" indicator, pushed selection notifications. It needs a lock file, which this design rules out
    - accepting or rejecting a proposed diff into the buffer, `executeCode`
    - pushing output tails into every prompt
  - Done when:
    - from a jed shell, a scripted MCP client running `jed claude mcp` reads back the active file and its selection, the open editors, the diagnostics, the terminals, and another shell's last commands with exit codes and output; `open_file` opens a tab
    - the Tools view lists "claude (mcp)" with its session count, and its log shows each request and response (`initialize` and `tools/list` too), the hook calls, and the session connecting and disconnecting; a JSON-RPC error raises the badge
    - `jed claude hook prompt` prints the active file line
    - outside jed (no `JED_IPC`), the hooks print nothing and the tools report "not inside jed"
    - nothing is written for discovery (only the tool log, as for every tool)
    - hand check with real `claude` after `/plugin install`: "explain this file" uses the active editor, "why did my last build fail" calls `get_command_output` on the other shell, and the session-start context names the workspace and branch. Smoke-test it again when Claude Code's version changes
  - Subtasks (one agent and one commit each):
    - claude_ide-a: command capture: `633;E` and OSC 7 in the three snippets, the per-run command ring with its row anchors and output extraction, `JED_TERMINAL` injection; `run.rs` unit tests feeding OSC sequences and a real `bash --norc --noprofile` PTY test in `shell_tests.rs` (as `shell_bash_snippet_titles_a_real_shell`)
    - claude_ide-b: IPC queries: `query` / `data` / `log` in `ipc.rs`, backward compatible, many requests per connection; the app's answers (`app/ipc.rs`, `app/claude.rs`) with their size caps; tests: a client against a headless app gets the context, editors, diagnostics, terminals and commands, and old `open` requests still work
    - claude_ide-c: the CLI modes: `jed claude mcp` and the two hooks in `claude.rs` (formatting plus the stdio loop, tested with an in-memory reader / writer), the forwarding of log-only lines; tests: the MCP handshake, `tools/list` / `tools/call` against a fake socket, and the hooks' silent no-`JED_IPC` path
    - claude_ide-d: the MCP log: `Tool::Claude`, `Row::Claude` / `RowKey::Claude` and its drawing, and the app's request / response / lifecycle / hook lines; `tools_tests.rs`-style tests of the row, the log's `→` / `←` lines and the error badge
    - claude_ide-e: the plugin and done-when: `claude-plugin/` and the marketplace manifest, `docs/claude.md`, `claude.newSession` (the adding-a-command checklist), the module map and conventions lines, `src/claude_ide_tests.rs` for the done-when, a real-`claude` smoke test (skipped with a note when the CLI is absent), gaps and hand checks; marks the task ✅
