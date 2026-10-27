---
id: open_file_path
name: Open File by path
summary: "\"File: Open File…\" (`file.open`, unbound; Cmd+O stays the terminal's) opens an `Input` prompt for a path — absolute, `~/…`, or relative to the root, with `:line[:col]` as the command line takes it — Tab completes the path segment (a listing off the UI thread, common prefix, a second Tab lists the choices in the prompt's hint), Enter opens it as quick open does (`open_file`, back / forward records it); a folder → a message with `jed <path>` (jed never switches workspace); a missing file → a message (creating it is the Untitled / Save As milestone's). Files outside the root work as today (no watch, no history)"
status: open
priority: 0
dependencies: []
---

