---
id: text_pointer
name: I-beam pointer over text
summary: show the mouse pointer as an I-beam (OSC 22 `text`) over the editor text and the text inputs
status: open
priority: 0
dependencies: []
---

  - Goal: the mouse pointer becomes an I-beam over anything you can type or select text in, as in VS Code, and goes back to the terminal's own over everything else
  - Needs: `terminal::Pointer` gains `Text` (OSC 22 name `text`; `Pointer::name` and `set_pointer`'s `POINTER_SET` bookkeeping already cover any non-default shape, so `restore` resets it); `App::pointer_at` in `app/mouse.rs` returns it, after the resize-border and drag checks (a border or a drag in progress keeps its arrow)
  - Over: an editor group's text area (not its gutter, tab bar, scroll bars or minimap), the find widget's query / replace inputs, the Search view's inputs, the palette's input and the prompt dialog's input (the rects the `HitMap` already holds for clicks), and a diff view's text panes. A terminal tab keeps its own: a program that sets the pointer itself is out of scope
  - Not over: a read-only hex view is not editable; leave it default. No new setting
  - Tests: `todo_tests::text_pointer`: draw, move the mouse (`app.mouse`) over the text area, gutter, a tab, a split divider and each input, and compare `app.pointer`; the bytes `run` sends through `terminal::set_pointer`'s writer variant for `Text`
  - Done when: `Pointer::Text` is chosen from the hit map in the places above, the divider arrows still win over it, and `scripts/check` passes
  - Still to check by hand: the I-beam in kitty / WezTerm / Ghostty (OSC 22 support varies; terminals without it show no change), and that it returns to the arrow when leaving a field for the gutter or a tab
