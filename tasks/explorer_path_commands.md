---
id: explorer_path_commands
name: Explorer path commands
summary: copy paths, reveal in Finder, and collapse folders in the explorer
status: open
priority: 0
dependencies: []
---

See [Explorer](../PLAN.md#explorer-file-tree). "File: Copy Path of Active File" / "File: Copy Relative Path of Active File" (VS Code's, unbound; a diff / commit / log tab: its file or nothing), "Explorer: Copy Path" / "Explorer: Copy Relative Path" (the selected row, file or folder; `/`-separated, relative to the root), "File: Reveal in Finder" (Linux: "Open Containing Folder" — `open -R <path>` / `xdg-open <parent>` off the UI thread, failure reported; for the active file, and "Explorer: Reveal in Finder" for the selected row), "Explorer: Reveal Active File" (expand to and select the active file's row, focusing the explorer), "Explorer: Collapse Folders" (every folder closed, the selection on its top-level row); all through `Clipboard::write` (OSC 52 over SSH), each with a short status message
