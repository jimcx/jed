---
id: selection_highlight
name: Selection highlight
summary: every other occurrence of the selected text is highlighted in the editor, and the status bar counts them (unless the find widget is open)
status: done
priority: 0
dependencies: []
---

  - VS Code's `editor.selectionHighlight`: the primary selection's text, when it is non-empty, on one line, at most `occurrences::MAX_LEN` (200) bytes and not only whitespace, is found literally and case-sensitively (`search::Matcher`, as find) across the buffer; each occurrence on screen gets `editor.selectionHighlight` (under find matches and the selection)
  - `occurrences::Occurrences` in each `View`, refreshed by `ui::editor` every frame but searched again only when the revision or the text changes (as the find widget's cache); drawn from a binary search of the visible range, walked with one index
  - The status bar shows `N occurrences` (the selection included) before the position while it highlights and the find widget is closed (the widget shows its own count)
  - Setting `selection_highlight` (bool, default true) in `settings.toml` / `.workspace`
  - Deferred: VS Code's word highlight for an empty cursor (the language server's `documentHighlight`); the occurrences in the scroll bar's marks column; with several selections VS Code highlights only when they all hold the same text (here the primary's text is used); a whole-buffer search on the UI thread per selection change (as find's), so a 10 MB file may cost a slow frame
  - Still to check by hand: the colour against the current-line, find-match and selection backgrounds in a real terminal
