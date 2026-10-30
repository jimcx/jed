---
id: search_ignored_files
name: Show ignored files
summary: gitignored files always visible in the explorer (dimmer), and an "include ignored" checkbox in workspace search
status: open
priority: 0
dependencies: []
---

See [Explorer](../PLAN.md#explorer) and [Workspace search](../PLAN.md#workspace-search).

**Explorer:** gitignored files and folders (`.gitignore`, `.git/info/exclude`, `.ignore`) are always listed, never hidden; no setting. They are drawn in a slightly darker grey than normal entries (new theme key, e.g. `explorer.ignored`, added to `Theme::dark_plus`), and a folder's whole subtree counts as ignored. Git status colouring wins where it applies. `.git/` and `.jed/` stay hidden and `.workspace` excludes still apply. Ignored state is computed off the UI thread with the directory listing (`ignore` crate's matcher, no `git2` on the UI thread) and refreshes with the live `notify` refresh. Large ignored trees (`target/`, `node_modules/`) are only listed when expanded, as any folder, so the initial listing stays cheap. Quick open keeps skipping ignored files.

**Workspace search:** an "Include ignored files" checkbox in the Search view, unchecked by default; checked, the search also walks files and directories `.gitignore` (and `.git/info/exclude`, `.ignore`) leaves out. `.git/`, `.jed/` and binary files are always skipped, `.workspace` excludes still apply, dirty buffers filtered the same way. Search view state like the other toggles (not in sessions); toggling re-searches; a keyboard/mouse-reachable checkbox that stays visible (or lit) while the details are hidden, as `…` is.

Tests: an ignored file is listed in the explorer with the ignored theme style while a normal one is not; a file under a gitignored `target/` is found by search only with the checkbox checked.
