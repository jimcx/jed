# tasks/

One file per task and per milestone, each with a machine-readable YAML
front-matter header followed by the task's body (verbatim from `PLAN.md`).
`scripts/tasks` is the interface: it lists, shows, finds, checks and indexes
these files, and nothing else should parse the headers by hand.

## The header

Every file opens with a `---` block, then the body. The fields, in order:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string (snake_case) | yes | The task's id; must equal the filename stem. |
| `name` | string | yes | The display name (PLAN.md's bold heading). |
| `summary` | string | yes | The one-line description. |
| `status` | string | yes | `open` · `in-progress` · `done`. |
| `priority` | integer | no | Higher = earlier in the index; unset = `0`. |
| `dependencies` | list of ids | no | Ids this task needs; `[]` when none. |

A task with subtasks:

```
---
id: soft_wrap
name: Soft wrap
summary: wrap long lines at the editor's width, with movement, scrolling and the mouse on screen rows
status: open
priority: 0
dependencies: []
---

<body verbatim>
```

A single task (no `Subtasks` list):

```
---
id: compare_files
name: Compare any two files
summary: a diff tab of one file against another
status: open
priority: 0
dependencies: []
---

<body verbatim>
```

A milestone:

```
---
id: milestone_9
name: LSP
summary: clangd and ty
status: done
priority: 0
dependencies: []
---

<body verbatim>
```

Values are plain YAML scalars or an inline list `[a, b]` for `dependencies`.
Quote a `name` or `summary` that contains `:` or `#` (e.g.
`name: "VHDL: tree-sitter and VHDL-LS"`). **No `#` comments and no nested
YAML** inside the block — the enum values and the conventions below live here,
not in the files.

## Status and the ✅ marks

`status` is set by the `/task` orchestrator (its step 9): `open` at creation,
`in-progress` after the first subtask lands, `done` after the last. It must
agree with the subtask `✅` marks in the body: a task with a
`Subtasks (one agent and one commit each):` list is `open` with none of them
marked, `in-progress` with some, `done` with all.

`scripts/tasks check` enforces the header shape (id, required fields, unique
ids, a valid status, an int priority, resolvable dependencies) as **errors**,
and reports a status↔`✅` disagreement as a **warning** (exit 0). The
orchestrator sets `status` in the same commit as the ✅ marks, so the
disagreement is a warning, not a failure.

## Milestones

`tasks/milestone_<N>.md` holds each finished milestone, same schema, with
`status: done` and `id: milestone_<N>` — referenceable as `milestone_<N>` in a
`dependencies` list. The ordinal lives in the id, not the name. Prose `M9` /
`M8` in a body is shorthand for `milestone_9` / `milestone_8`; there are no
`dependencies` on historical work.

## The script

`scripts/tasks` (Python 3, stdlib only). Exit 0 ok, 1 error, 2 usage.

```
scripts/tasks list [--status open|in-progress|done]... [--sort priority|name|id] [--reverse] [--json]
scripts/tasks show <id> [--header | --body]
scripts/tasks find <id>
scripts/tasks check [--warn-only]
scripts/tasks index [--write] [--milestones | --tasks]
scripts/tasks done-since <since> [<until>]
```

Examples:

```
scripts/tasks list --status open --sort priority   # one line per open task
scripts/tasks show soft_wrap                        # the whole file
scripts/tasks show soft_wrap --body                 # just the body
scripts/tasks find git_hist                          # exact or unique prefix
scripts/tasks check                                  # the migration's test
scripts/tasks index --write                          # regenerate PLAN.md's index
scripts/tasks done-since v12 v13                     # for scripts/changelog
```

Mutations are not here: creating a file, splitting a task and setting `status`
are edits the `/task` orchestrator makes directly (its steps 5 and 9), followed
by `scripts/tasks index --write`.

## How `/task` uses these

1. **Step 1** — `scripts/tasks list --status open` (and `--status done` when
   needed) replaces the `awk` over PLAN.md's Tasks section.
2. **Step 2** — `scripts/tasks show <id>` (plus reading `tasks/<id>.md`) replaces
   "read only the chosen task's entry".
3. **Step 5** — splitting a too-big task edits `tasks/<id>.md` (add "Needs",
   "Done when", the `Subtasks` list, the id), then `scripts/tasks index --write`.
4. **Step 9** — marking done sets `status` (`in-progress` after the first
   subtask, `done` after the last), updates `summary`, then
   `scripts/tasks index --write`. No move to a separate archive.
5. **Before committing** — `scripts/tasks check` must be clean.

## Conventions

- `id` = snake_case filename; renaming a file means renaming its `id` and every
  `dependencies` reference to it.
- `dependencies` cites ids only; the prose context (a `Needs:` line naming
  modules or milestones) stays in the body.
- Keep the body's `Needs:` / `Done when:` / `Subtasks (one agent and one commit
  each):` structure — the script reads the subtask list from it.
- A finished task keeps its file with `status: done`; its full entry (what was
  built, "Deferred", hand checks) stays in the body. Nothing is archived
  elsewhere.
