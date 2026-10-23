# jed as git's editor

jed can be the editor git opens for commit messages and `git rebase -i` todo lists.

## Set-up

```sh
git config --global core.editor "jed --wait"    # commit, merge, tag and notes messages (and rebase lists)
git config --global sequence.editor "jed --wait" # only the rebase todo list (optional)
export GIT_EDITOR="jed --wait"                  # per shell, wins over core.editor
```

`--wait` makes any file open in editor mode; git's own message files (`COMMIT_EDITMSG`, `MERGE_MSG`,
`SQUASH_MSG`, `TAG_EDITMSG`, `EDIT_DESCRIPTION`, `NOTES_EDITMSG`, `git-rebase-todo`) inside a git
directory (the folder holds `HEAD`) get it even without the flag. Git waits for jed to exit.

## What editor mode does

- It is single file mode (`jed -s`) with the repository's work tree as the root (a bare repository: its
  git directory), so the repository's `.workspace` settings and file associations apply.
- Nothing per-workspace happens: no session is read or written, no recent-workspace entry, no trust
  dialog, no language servers or tasks. The side and bottom panels stay closed.
- The cursor starts at line 1, column 1. The status bar says "Save and close to commit · an empty message
  aborts" (for a rebase list: "Save and close to rebase · an empty list aborts").
- Commit messages are coloured and ruled at column 72; Cmd+/ comments lines with `#`.
- Cmd+W on the file's tab (or Cmd+Q) quits jed. Unsaved changes ask first. The exit status is 0.
- Crash backups still run.

## From a task terminal

`git commit` run in one of jed's tasks opens its message as a tab in that jed rather than starting a
second one in the task's terminal. jed listens on a Unix socket (`$XDG_RUNTIME_DIR/jed/<pid>.sock`, else
`$TMPDIR/jed-<uid>/<pid>.sock`, removed on quit) and gives only task processes `JED_IPC=<socket>`; `jed` in
an ordinary shell never sees it. With `JED_IPC` set, `jed [--wait] <file>[:line[:col]]` sends the request
to the running jed and exits: the file opens in the active group (a missing one as a new file), and with
`--wait` the command finishes once the file's last tab (any group) is closed, so the task waits for you
to save and close the message. Quitting jed while it waits makes the command exit 1 (git aborts). A dead
or refused socket is logged and jed starts normally. Workspace mode only (editor mode has no tasks).

## Aborting

Git aborts itself when the message is empty (only comments). Delete the text, save, close; or close
without saving a message git left empty.
