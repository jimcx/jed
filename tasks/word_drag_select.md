---
id: word_drag_select
name: Word-wise selection on double-click drag
summary: double-click and drag selects whole words, the end rounded out to the word under the pointer, in editors, shells and task tabs
status: open
priority: 0
dependencies: [term_select_word]
---

Fix up selecting text with the mouse. After a double-click, dragging extends the selection word by word, as in VS Code: the word that was double-clicked stays selected, and the far end follows the pointer rounded out to the end of the word under it (forwards: that word's end; backwards: its start). Today the editor only keeps the anchor word and ends the selection at the raw pointer position, and a shell / task tab's drag from a double-click anchors on the single clicked cell and ends at the raw cell.

  - Needs: `term_select_word` (`select::path_run`, `terminal_select`, `Drag::Terminal`), the editor's `Drag::Text { anchor }` (`app/mouse.rs`, `pos::word_range`)
  - Editor: in the `Drag::Text` arm, when the press was a double-click (anchor is a word range), round the pointer position to the word containing it (`pos::word_range`) — the end when dragging forwards past the anchor, the start when dragging back — so the selection always covers whole words. A triple-click drag keeps selecting whole lines; a single-click drag is unchanged.
  - Shells and task tabs: `Drag::Terminal` carries the seeded run (`path_run`'s range) as its anchor range, not the single clicked cell. While dragging after a double-click, the head is rounded to the run under the pointer (its end forwards, its start backwards; a non-path cell under the pointer extends to that cell), and dragging back past the anchor keeps the whole anchor run selected. A single-click drag is unchanged, as is a program that has asked for the mouse.
  - Done when: in an editor, a double-click on a word and a drag right to the middle of another word selects through that word's end, and a drag left selects from its start, keeping the first word either way; the same in a shell and a task tab with path runs; a single-click drag still selects exactly to the pointer; a triple-click drag still selects whole lines; a mouse-reporting program keeps every event. Tests drive it headlessly (`mouse` presses and drags) for the editor and for a shell tab, and the rounding helper has unit tests.
  - Deferred: Unicode word rules in the terminal (still the path-character run), Alt+drag column selection
