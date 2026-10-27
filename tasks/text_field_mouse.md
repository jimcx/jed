---
id: text_field_mouse
name: Text field mouse
summary: click, drag, Shift+click, double and triple click in the find widget, Search view and palette inputs
status: done
priority: 0
dependencies: []
---

  - Model: `TextField::offset_at_col(col, scroll)` maps a drawn screen column through the field's sideways scroll to a byte offset on a grapheme boundary (a column inside a wide char gives its start, past the text its end; `col` is signed so a drag past the left edge scrolls back, clamped at 0), and `click(at, count, shift)` / `drag_to(at)` run the editor's rules: a click places the cursor, a double click the word (the same classifier), a triple click all the text, Shift extends from the anchor, and a drag extends from the click's anchor (back at it, the selection goes). `pos::word_range_in(text, off)` is the `&str` core extracted from `word_range`, so the editor and the fields share one double-click classifier
  - Drawing: `ui::find::draw_field` now returns `DrawnField { cursor, scroll }` (the cursor's position and the sideways scroll it used to keep the cursor in view); each hit carries its input's rect and scroll — `FindHit` gains the query / replace rects and `query_scroll` / `replace_scroll` it lacked, `SearchHit` gains `input_scrolls` / `replace_scroll`, `PaletteHit` gains `input` / `input_scroll`. The palette's input now goes through `draw_field` too (it scrolls and draws its selection like the others, instead of its old truncating `set_stringn`)
  - Mouse: `Drag::Field(FieldTarget)` in `app/mouse.rs` (`FieldTarget::{Find(find::Field), Search(search_view::Input), Palette}`) with `field_ref` / `field_mut` / `field_geometry` / `field_position`; `find_mouse` and `search_click` place the cursor and arm the drag (Shift / double / triple from the `ClickTracker` count and the event's modifiers), and a `Drag::Field` in `mouse_event` extends the selection. A drag past either end maps past the visible text, so the field scrolls to select off-screen text. `palette_mouse` (which takes every mouse event while open) keeps its own input click / drag / release
  - Also: `find_mouse` and `palette_mouse` now clear a stale drag and count clicks themselves, as `mouse::left_click` does
  - Deferred: the prompt dialog's `Input` field (drawn by `ui/dialog.rs`, not `draw_field`) still has no mouse selection
  - Still to check by hand: double / triple click timing and Shift+click in a real terminal, dragging past either end in the find widget and the Search view's query, and the palette input now scrolling on a long query (a behaviour change from its old truncation)
