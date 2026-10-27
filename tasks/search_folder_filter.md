---
id: search_folder_filter
name: Folder filter in the Search view
summary: VS Code's files-to-include and files-to-exclude fields under the query
status: done
priority: 0
dependencies: []
---

  - `path_filter.rs` (VS Code's globs: comma-split, `{…}` / `\,` aware; no `/` = any depth (`src` = every folder named src, VS Code — a deviation from "relative to the root"), a `/` or `./` = rooted; each pattern takes everything under it; `!` within a field; exclude beats include; an only-`!` include = everything else; `folder_pattern` escapes a folder for Find in Folder), one matcher for the walk (`explorer::walker_with` prunes only excluded folders), files and dirty buffers (also deleted files); `search_view.rs` `include` / `exclude` fields, `details`, `input`, `cycle`; `…` on the query row and `search.toggleDetails` (Cmd+Shift+J, `docs/terminals` updated); Tab / Shift+Tab cycle query → include → exclude → list; the 150 ms debounce for every field; hidden fields still apply (`…` lit); `explorer.findInFolder` (palette; right-click on a folder row — a file row still shows its history) fills include with `./path`, shows details and searches; not in sessions
  - Deferred: nothing of the item
  - Still to check by hand: Cmd+Shift+J reaching jed in each terminal with the updated snippets (iTerm2's default Jump to Mark is on it); the `…` button and the inputs in a narrow side bar; right-clicking a folder
