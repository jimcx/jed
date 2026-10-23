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

## Aborting

Git aborts itself when the message is empty (only comments). Delete the text, save, close; or close
without saving a message git left empty.
