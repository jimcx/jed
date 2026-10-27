---
id: discard_changes
name: Discard changes in Source Control
summary: discard a file's or all changes in Source Control
status: open
priority: 0
dependencies: []
---

See [Git](../PLAN.md#git-view-and-stage-only). A `↶` button on each Changes row (beside `+`) and "Git: Discard Changes" (palette, `git.clean` — the active file or the selected row) / "Git: Discard All Changes" (the Changes section's header button): after a confirm prompt naming the file(s) ("Discard changes in a.rs?"; for a closed file it can't be undone, though file history keeps its last save from jed), the work tree file gets the index's text back (= `git checkout -- path`; an open buffer through `Buffer::edit`, one undo step, left dirty, as Revert hunk (so the confirm says the buffer needs saving); a closed file written off-thread, keeping permissions); an untracked file is moved to the Trash (VS Code deletes it), its tab marked deleted; a staged-only change is left alone (the Staged section isn't touched); conflicted files refused (merge conflicts milestone). Refused while the file has a save in flight, like staging
