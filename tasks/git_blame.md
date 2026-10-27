---
id: git_blame
name: Inline git blame
summary: the cursor line's blame after its text, VS Code's inline blame
status: done
priority: 0
dependencies: []
---

  - Only the primary cursor's line; never moves text or columns, cut with "…" at the editor's edge, not drawn when the line fills the width; hidden while a selection spans lines; "You, 2 min ago • Uncommitted changes" (or nothing) for a line changed since the last commit, incl. unsaved edits
  - `when` is relative (`history::ago`-style: "3 days ago"); summary = the commit message's first line, cut to fit
  - Blame from `git2` (`blame_file`, then `blame_buffer` with the buffer's text so unsaved edits count) off the UI thread (`spawn_blocking` → an `AppEvent`), cached per buffer by (HEAD, index stamp, revision) — one blame per file, so moving the cursor costs nothing; mapped through edits while a recompute runs like quick diff's marks, recomputed after a short delay; untracked files and files outside a repo: nothing
  - `blame.rs` (model: hunks of line-start bytes + commit or uncommitted, on every `Buffer` as `buf.blame`, mapped in `Buffer::apply`; blocking `compute`: `git2` `blame_file` at HEAD, cached as a `Base` per (HEAD oid, path), then a context-0 diff of the buffer's encoded bytes against the HEAD text marks added / changed lines uncommitted — instead of `blame_buffer`, so an edit doesn't redo the history walk), `app/blame.rs` (quick diff's scheduling: only while on and for shown buffers; at once when never blamed or after each git status read, after edits `DELAY` 300 ms via `Timer::Blame` → `AppEvent::Blame`; off resets); drawn by `ui::editor` (`Overlay::blame`) 3 cells after the primary line's end in `git.blame.editorDecorationForeground`
  - Off by default, session state (VS Code's default); `Author, 3 days ago • summary` (author, as VS Code; "You" when the author's email is `user.email`); uncommitted lines (staged too: compared with HEAD) say "Uncommitted changes" without a name / time; cut with "…" before the scroll bar, nothing with < 6 cells or when the primary spans lines
  - Deferred: a hover / details for the blame; blame in diff tabs; following renames; `autocrlf` work trees marked all uncommitted (as quick diff); a horizontally scrolled view hides it rather than showing part of it
  - Still to check by hand: a big file with long history (blame time; "slow git blame" logged over 500 ms); typing switches to "Uncommitted changes" after ~300 ms; a commit from another terminal updates it
