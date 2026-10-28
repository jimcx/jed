---
id: discard_changes
name: Discard changes in Source Control
summary: discard a file's or all changes in Source Control
status: done
priority: 0
dependencies: []
---

See [Git](../PLAN.md#git-view-and-stage-only). A `↶` button on each Changes row (beside `+`) and "Git: Discard Changes" (palette, `git.clean` — the active file or the selected row) / "Git: Discard All Changes" (the Changes section's header button): after a confirm prompt naming the file(s) ("Discard changes in a.rs?"; for a closed file it can't be undone, though file history keeps its last save from jed), the work tree file gets the index's text back (= `git checkout -- path`; an open buffer through `Buffer::edit`, one undo step, left dirty, as Revert hunk (so the confirm says the buffer needs saving); a closed file written off-thread, keeping permissions); an untracked file is moved to the Trash (VS Code deletes it), its tab marked deleted; a staged-only change is left alone (the Staged section isn't touched); conflicted files refused (merge conflicts milestone). Refused while the file has a save in flight, like staging
- ✅ Built (`src/app/discard.rs`, `stage::discard_file`, `Prompt::ConfirmDiscard`, `ScmHit::discards`)
  - `git.clean` / `git.cleanAll` (unbound), `↶` on Changes rows and the Changes header; conflicted, staged-only and saving files refused (rechecked on confirm).
  - `explorer::ops::delete` removes the entry under `cfg(test)` instead of using the real Trash.
  - Deferred: an untracked entry that is a whole folder (nested repository) goes to the Trash, untested.
  - Still to check by hand: the `↶` glyph in a real terminal; discard an open dirty buffer, Cmd+Z once, save; discard an untracked file and see it in the system Trash with its tab marked deleted.
