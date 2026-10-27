---
id: editor_scrollbars
name: Scroll bars in the editors
summary: a vertical scroll bar per editor group, click and drag to scroll
status: done
priority: 0
dependencies: []
---

  - Text and diff tabs always reserve the editor's last column (text never reflows; the find widget, popups, `view.width` and `GroupHit::editor` use the narrower area; the splash keeps the full width); a thumb only when the text doesn't fit or is scrolled (VS Code's `auto`). `scrollbar.rs` (model: `Scroll::text` — `max_top = lines - 1`, as jed scrolls the last line to the top — / `Scroll::rows` for diff tabs, `thumb`, `top_at`), `ui/scrollbar.rs` (drawing, `scrollbarSlider.{background,hoverBackground,activeBackground}`), `app/scrollbar.rs` (`Hit::ScrollBar(group)`, `Drag::ScrollBar { group, grab }`): a track click centres the thumb there and grabs it, a drag keeps the grab and continues off the bar until release; never focuses the group or moves cursors; the wheel over it scrolls the editor
  - Deferred: the overview-ruler marks; a setting to hide it; paging on track clicks; sub-cell thumb precision; a horizontal bar; a tab switch mid-drag keeps scrolling the new tab; the hover colour can stay after a wheel scroll until the pointer moves
  - Still to check by hand: the slider greys' visibility (also the 256-colour fallback); hover (needs any-motion mouse reporting); smooth drags on a huge file; a drag continuing outside the window; the diff tab's header cell above the bar; the find widget and popups clear of the bar
