---
id: change_eol
name: Change End of Line Sequence
summary: "\"Change End of Line Sequence\" (palette, unbound; a click on the status bar's `LF` / `CRLF`) picks LF or CRLF for the buffer: every line break rewritten as one `EditKind::Other` step through `Buffer::edit` (undo brings the old ones back), new lines then use it, saved as is; a `files_eol` setting (`auto` (detect, default) / `lf` / `crlf`) for new files and files without a line break"
status: open
priority: 0
dependencies: []
---

