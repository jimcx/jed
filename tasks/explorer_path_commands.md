---
id: explorer_path_commands
name: Explorer path commands
summary: copy paths, reveal in Finder, and collapse folders in the explorer
status: done
priority: 0
dependencies: []
---

See [Explorer](../PLAN.md#explorer-file-tree). "File: Copy Path of Active File" / "File: Copy Relative Path of Active File" (VS Code's, unbound; a diff / commit / log tab: its file or nothing), "Explorer: Copy Path" / "Explorer: Copy Relative Path" (the selected row, file or folder; `/`-separated, relative to the root), "File: Reveal in Finder" (Linux: "Open Containing Folder" — `open -R <path>` / `xdg-open <parent>` off the UI thread, failure reported; for the active file, and "Explorer: Reveal in Finder" for the selected row), "Explorer: Reveal Active File" (expand to and select the active file's row, focusing the explorer), "Explorer: Collapse Folders" (every folder closed, the selection on its top-level row); all through `Clipboard::write` (OSC 52 over SSH), each with a short status message

✅ Done.
- Built: `ExplorerCommand::{CopyPath, CopyRelativePath, RevealInFinder, RevealActive, CollapseAll}` (`explorer.copyPath`, `.copyRelativePath`, `.revealInFinder`, `.revealActiveFile`, `.collapseFolders`) and `Command::RevealInFinder` (`file.revealInFinder`); all unbound. Handlers in `app/side.rs`, `Explorer::collapse_all`, `AppEvent::RevealFailed`. "File: Copy Path of Active File" already existed (task `copy_paths`).
- Also: reveal runs `open -R <path>` (macOS) / `xdg-open <parent>` in `spawn_blocking`.
- Deferred: a Linux-specific "Open Containing Folder" title (one title "Reveal in Finder"); Reveal Active File isn't gated in single-file mode; File: Reveal in Finder with no active file is silent.
- Still to check by hand: File / Explorer: Reveal in Finder open the file manager on a real desktop; the warning shows without `xdg-open`.
