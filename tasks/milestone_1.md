---
id: milestone_1
name: Skeleton
summary: open a file, edit, save, scroll, status bar
status: done
priority: 0
dependencies: []
---

   - Selection list from the start; undo transactions and grouping
   - Text position types (`buffer::pos`); grapheme-aware movement, display width
   - Files: UTF-8 check (keep BOM), refuse binary, detect and keep LF/CRLF, keep final newline
   - Unsaved-changes prompt on quit
   - Kitty keyboard protocol on; terminal check at startup
   - Panic hook; logging; atomic save
   - Done when: a 1 MB CRLF file can be opened, edited, undone/redone and saved back byte-identical, with no lag; a panic leaves the terminal usable
