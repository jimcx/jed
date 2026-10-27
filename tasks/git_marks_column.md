---
id: git_marks_column
name: Git change marks in their own column
summary: changed-line marks in a column of their own beside the scroll bar, clear of the thumb
status: done
priority: 0
dependencies: []
---

  - What it changed (for later work): `ui/scrollbar.rs` — `render` draws only the bar's track and thumb (drops the `marks` parameter); `render_marks(out, track, scroll, base, theme, marks)` is now public and fills a dedicated marks column with `base` then the marks' cells (still projected by `Scroll::cell`, top-independent, so a mark stays put while scrolling). `ui/mod.rs` — after the scroll bar's `take_column`, a text or diff tab takes a second column for the marks (`!app.mode.is_single() && (active_tab() || active_diff())`); the horizontal bar's corner span blanks both columns; `fit_commit_tabs` takes both columns so commit/log tabs reserve the empty column too; every other `scrollbar::render` caller (side / panel / scm / tools / keys / cheat sheet) drops the `marks` argument. `layout.rs` — `GroupHit::marks: Option<Rect>` (the marks track, a diff tab's below its header), `None` for hex / shell / single-file, `#[cfg_attr(not(test), allow(dead_code))]`.
  - Also: a diff tab's marks column's header cell is painted `diffEditor.header` so the header line stays continuous; the column is always reserved, so nothing reflows as marks appear or go; the QuickDiff lifecycle is untouched — marks still follow edits and recompute on staging / index change, only where they are drawn moved; the theme keys (`editorGutter.*` and staged siblings) are unchanged.
  - Deferred: none
  - Still to check by hand: a real terminal — a large thumb beside a column of bright / dark marks, the marks staying at their height while scrolling; a diff tab's hunk rows coloured in the column with the header continuous; a hex / shell tab and `jed -s file` one column wider with no marks column; a commit / log tab's empty reserved column; the horizontal bar's two blank corner cells.
  - Needs: `scrollbar::take_column`, `Scroll::cell`, `quick_diff`'s marks (`lines_of` + `theme_key`), a diff tab's `hunks()` / `aligned().starts` / `Hunk::kind`, `GroupHit`.
  - Done when: in a text tab with staged + unstaged changes, the marks fill their own column beside the scroll bar and stay put as you scroll and under the thumb (the thumb never covers them); a diff tab marks its hunks the same way; a hex / shell tab and `jed -s file` show no marks column; the marks still follow edits and recompute on staging / index change (the existing `QuickDiff` lifecycle, untouched).
