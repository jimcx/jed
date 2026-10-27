---
id: revert_hunks
name: Revert hunks in the diff view
summary: a Revert button on each hunk of an unstaged diff, restoring the working tree
status: done
priority: 0
dependencies: []
---

  - Only on `Pair::UNSTAGED` diffs: a ` Revert ` button (`diffEditor.revertAction`, white on Dark+'s error red) on each hunk's first row — side by side at the right end of the left (index) column, inline left of Stage, dropped when it doesn't fit (`DiffHit::reverts`); `git.revertHunk` (unbound, palette: the hunk at the diff cursor). Open buffer: `diff_view::revert_edit` (git's line split: CRLF / lone CR / BOM) applied with `apply_changes` (one `EditKind::Other` undo step, left dirty) only while the buffer still holds the shown text; read-only refused. Not open: blocking `git::stage::revert_hunk` (index and disk checked against what was shown, else `Stale`; untracked, conflicted, binary and symlinks refused; bytes as stored, mode kept, a deleted file back with the index's mode; the index untouched), then the file's text from before the revert is recorded in its file history (a failed record only warns) so File History: Restore can undo it; git status and the diff re-read
  - Deferred: revert from the gutter / its peek in a text tab; a confirmation before writing to disk (VS Code doesn't ask); a deleted file's revert records an empty "before" snapshot; a BOM only in the index's first line stays a difference on the buffer path
  - Still to check by hand: Revert in side-by-side and inline layouts; Cmd+Z in the file's tab; revert a file that isn't open, then File History: Restore
