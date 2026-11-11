# Claude Code in jed

Run `claude` in a jed shell tab and it can see what you see in the editor: the active file and your
selection, the open files, the diagnostics, what the other shells ran and printed, and the open
merge request's review threads. It is a Claude Code plugin (a stdio MCP server plus two hooks) that
finds the running jed through `$JED_IPC`, which every jed shell already has. Nothing is written
under `~/.claude` or anywhere else for discovery, and outside jed the plugin does nothing.

"Claude: New Session" in the command palette opens a new shell tab with `claude` already started.

## Install

With the plugin (needs `jed` on `PATH`):

```
/plugin marketplace add jimcx/jed
/plugin install jed
```

A local checkout works too: `/plugin marketplace add /path/to/jed`.

Without the plugin, add the MCP server and the two hooks yourself:

```sh
claude mcp add -s user jed -- jed claude mcp
```

and in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "jed claude hook session-start" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "jed claude hook prompt" }] }]
  }
}
```

Shell commands are captured by jed's shell integration ("Terminal: Install Shell Integration"; run it
again after updating jed so the snippet sends the command text).

## What Claude gets

The hooks add a little context on their own:

- at session start: jed's version, the workspace, the git branch and how many files are changed, the open
  files, the running language servers, the task templates, the shells, and a line on the tools below
- with each prompt: the active file and line (`Active file: src/app/ipc.rs:42:5 (selection 40-48)`), a small
  selection's text, each other shell's last command and exit code, and, with a merge request open, a line of counts
  (`Review: !42 "Add review panel", 3 unresolved threads, 1 pending draft`)

The MCP tools (all read the editor's current state; `claude` runs in one of the shells, which is marked `self`
and not fed back to it):

| Tool | What it does |
|---|---|
| `get_active_editor` | path, language, unsaved state, cursor, selections and their text (capped) |
| `get_open_editors` | every open file |
| `get_diagnostics` | errors and warnings, for one file or all |
| `get_environment` | workspace, branch, servers, tasks, shells |
| `list_terminals` | the shells: name, directory, running or idle, last exit code |
| `get_recent_commands` | a shell's last commands with exit codes |
| `get_command_output` | the output of one command |
| `get_terminal_screen` | what a shell shows now (a TUI, a half-typed prompt) |
| `find_definition` | where a symbol is defined (`kind`: definition, type, implementation or declaration), from the language server |
| `find_references` | the references to a symbol (up to 200, sorted by path then line) |
| `get_hover` | the server's hover text (markdown, capped at 4 KB) and its range |
| `get_document_symbols` | a file's symbols as a tree: name, kind, detail, line range, children |
| `find_workspace_symbols` | symbols by name across the workspace (`max`, default 50, up to 200) |
| `open_file` | opens a file at a line in jed |
| `get_review` | the merge request, its files and your pending drafts |
| `get_review_threads` | the threads, their live lines, comments and suggestions |
| `review_refresh` | reloads the merge request |
| `review_apply_suggestion` | applies a suggestion and saves the file |
| `review_draft_reply` | adds a reply as a draft, optionally marking the thread to resolve |

The language server tools ask the servers jed already runs, so they know what the editor knows (and need the
server for that language to be running). Name a place with `path`, a 1-based `line` and the `symbol` on it
(the first whole-word match; `col` picks a later one; with neither, the line's first non-blank): Claude never
counts columns, and a `symbol` that isn't on the line is an error quoting the line as jed has it. Answers carry
locations as `{path, line, col, end_line, end_col, text}` (path relative to the workspace, at most 200, with
`count` and `truncated`). `dirty: true` says the file has unsaved changes and the server answered for them. A
file that isn't open is loaded as a background tab (`opened: true`). `server_busy` names the work a server is
still reporting (indexing: the answer may be incomplete). A request waits up to 10 s (then `$/cancelRequest` and a
timeout error), is asked again after a routine `ContentModified`, and answers "the file changed while asking:
ask again" if the file is edited meanwhile. Nothing the user sees changes: no cursor move, scroll, popup,
Locations list, back / forward entry or notice. Each call and the size of its answer are in the "claude (mcp)"
log.

Outside jed (`JED_IPC` unset or the socket gone) the hooks print nothing and the tools answer "not inside a
jed terminal".

## Privacy

- Shell output reaches Claude only through `get_command_output` and `get_terminal_screen`, which Claude Code
  asks you to allow. The hooks never include it.
- Comment text reaches Claude only as a tool result, never through the hooks (which show counts).
- Every request and response is in the Tools view under "claude (mcp)" (open it with Enter), so you can read
  what was sent.

## Code review

With a merge request open in jed, ask Claude to "deal with the review feedback". It reads the threads
(`get_review_threads`), fixes the code with its own edits, applies suggestions, and drafts replies. It can
never post: the drafts wait in PENDING, tagged `claude`, and you read, edit or delete them and press Submit
Review in jed, which posts the replies and resolves the threads marked "+ resolve". Claude's tools read what jed
already loaded; only `review_refresh` reloads it (read requests to the forge) and none of them writes to the forge.

`review_apply_suggestion` refuses while the file has unsaved changes in jed or when the suggestion is out of
date. To forbid applying and drafting altogether, set `mcp_read_only = true` on the host's entry in
`~/.config/jed/forges.toml`; reading still works. The setting is read only from that file.

## Troubleshooting

- *"not inside a jed terminal"*: `claude` was started outside a jed shell, or from a shell opened before jed
  restarted. Open a new jed shell tab.
- *Tools missing*: run `/mcp` in Claude Code; `jed` must be on `PATH` for the process Claude Code spawns.
- *No command text*: install or refresh the shell integration (above).
- *Review tools say there is no merge request*: the message names the cause (no repository, unknown host, no
  token). Forge hosts and tokens come only from `~/.config/jed/forges.toml`.
