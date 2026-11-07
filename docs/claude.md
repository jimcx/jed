# Claude Code in jed

> **Status: planned, not built yet.** This page documents the design in
> [tasks/claude_ide.md](../tasks/claude_ide.md) (the `jed claude mcp` server, the hooks and the
> plugin). Until that task lands, the commands below don't exist. Update this note when it does.

Run `claude` in a jed shell tab and it can see what you are looking at: the active file and
selection, the open editors, diagnostics, other shells' recent commands and output, and the open
merge request's review threads. jed's side is a small stdio MCP server (`jed claude mcp`) plus two
hooks. Both find the running jed through the `$JED_IPC` environment variable that every jed shell
already has, so there is no lock file, no port and nothing written under `~/.claude`.

Outside a jed shell (`JED_IPC` unset or its socket gone) the hooks print nothing and the tools
return "not inside a jed terminal".

## Install: the plugin (recommended)

In Claude Code:

    /plugin marketplace add jimcx/jed        # or a path to a jed checkout
    /plugin install jed

The plugin (`claude-plugin/`) registers the MCP server and both hooks. `jed` must be on your
`PATH`.

## Install: by hand

Register the MCP server for your user:

    claude mcp add -s user jed -- jed claude mcp

Then add the hooks to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "jed claude hook session-start" }] }
    ],
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "command": "jed claude hook prompt" }] }
    ]
  }
}
```

The server alone works; the hooks only add context up front (versions, workspace, branch, the
active file line on each prompt).

## Shell integration

Command text and output reach Claude only if the shell has jed's integration snippet. Run
"Install Shell Integration" from the palette (re-run it after upgrading jed). Without it the tools
still work, minus command lines.

## What Claude can use

| Tool | Gives |
|---|---|
| `get_active_editor` | path, language, dirty flag, cursor, selections and their text (capped) |
| `get_open_editors`, `get_diagnostics`, `get_environment` | open tabs, language-server diagnostics, workspace / branch / servers |
| `list_terminals`, `get_recent_commands`, `get_command_output`, `get_terminal_screen` | other shells: commands with exit codes, output, the visible screen |
| `open_file(path, line?, col?)` | opens a tab in jed |
| `get_review`, `get_review_threads`, `review_refresh` | the open merge request and its threads (from jed's loaded state; no forge calls, no token) |
| `review_apply_suggestion` | applies a suggestion and saves the file; refused if the buffer has unsaved changes or the suggestion is out of date |
| `review_draft_reply` | queues a reply (optionally resolving) as a draft tagged `claude` in PENDING |

Shell output is never pushed into prompts: it reaches Claude only through a tool call, which
Claude Code's permission prompt covers. Nothing reaches the forge until you run **Submit Review**
in jed; read the `claude` drafts in PENDING first.

## Seeing what Claude does

The Tools view lists **claude (mcp)** with its session count and an error badge. Enter opens its
log: every JSON-RPC request and response, hook calls and sessions connecting. "Tools: Clear Log"
and "Tools: Open Log" work as for the other tools.

## Troubleshooting

- *"not inside a jed terminal"*: `claude` was started outside a jed shell, or from a shell opened
  before jed restarted. Open a new jed shell tab.
- *Tools missing*: run `/mcp` in Claude Code; `jed` must be on `PATH` for the process Claude Code
  spawns.
- *No command text*: install or refresh the shell integration (above).
- *Review tools say there is no merge request*: the message names the cause (no repository,
  unknown host, no token). Forge hosts and tokens come only from `~/.config/jed/forges.toml`.
