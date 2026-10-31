---
id: blame_hash
name: Commit hash in inline blame
summary: a short commit hash in the inline blame label that opens the commit view when clicked
status: open
priority: 0
dependencies: [git_blame]
---

The inline blame annotation ("Ann Smith, 3 days ago • Fix crash") gains the commit's short hash, and clicking the hash opens that commit in the same commit tab as clicking a commit in the Explorer's Git History section.

  - Needs: `blame.rs` (`Commit`, `label`, `fit`, `compute`), `ui/editor.rs` (the blame drawing, which records hit rects), `HitMap` (a new `Hit::BlameHash`, like the other render-time rects), `app/mouse.rs`, `app/commit_tabs.rs` (`open_commit(id, None)`, the call the Git History row makes), `theme.rs`
  - Model: `blame::Commit` grows an `id` (the full oid string, from the `git2` hunk's final commit id, kept in the cached `Base`'s per-run commits) so the label can show `id[..7]` and a click can pass the full id on. `label` becomes `7f3a2c1 Ann Smith, 3 days ago • Fix crash` (hash first, in a dimmer `git.blame.hash` theme key, falling back to the label's colour); "Uncommitted changes" has no hash and no hit. `fit` keeps the hash when it cuts the rest to "…", and drops the whole label only below `MIN_WIDTH` as now.
  - Click: `ui::editor` records the hash's cells in the `HitMap` when it draws the label; a left click there (not a drag, no modifier) calls `open_commit(id, None)` — the same view, tab reuse (`find_commit_buffer`) and loading as a Git History row — and doesn't move the cursor or selection. Hovering the hash gives the hand / underline treatment other clickable text uses, if jed has one. Single file mode and a repo-less buffer have no blame, so nothing changes there.
  - Done when: with inline blame on, a committed line's label starts with its 7-character hash; the hash is the same commit as `git blame` shows for that line; an uncommitted line shows no hash; clicking the hash opens the commit tab for that commit (clicking the same hash again, or the same commit in the Git History section, focuses the existing tab rather than opening a second); clicking elsewhere on the label or the line places the cursor as before; a narrow editor keeps the hash while the rest is cut, and shows nothing below `MIN_WIDTH`. Update the `blame.rs` entry in `docs/dev/modules.md` and the mouse notes in `docs/dev/app.md`. Tests: unit-test `label` / `fit` with a hash, and drive the click headlessly in a temp repo (`mouse`, `settle`, `TestBackend` drawing), asserting the commit tab opened.
  - Deferred: a hover card with the full message, author and date, "Copy Commit Hash" in a context menu (see `file_info`), blame in the gutter
