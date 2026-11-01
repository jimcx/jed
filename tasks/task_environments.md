---
id: task_environments
name: Task environments
summary: named sets of variables (Debug, Release) filled into task commands as {{NAME}}
status: done
priority: 0
dependencies: []
---

`[environments.<name>]` tables of string variables in `.workspace` (trusted only; part of the trust hash) and `tasks.toml` (the workspace wins a name). A task's `command`, `cwd` and `env` values use `{{NAME}}`; shell syntax (`${NAME}`) is untouched. One environment is active at a time: "Tasks: Select Environment" or a click on the status bar's `Env:` item (shown once any is defined) opens the picker, `(none)` first; the pick is kept in the session. A task using a name the active environment lacks is not run and says why.

Deferred: variables exported into the task's process environment; a per-task default environment; `${…}`-style or default values (`{{NAME:-x}}`).
