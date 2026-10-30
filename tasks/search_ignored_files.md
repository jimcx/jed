---
id: search_ignored_files
name: Show ignored files
summary: gitignored files always visible in the explorer (dimmer), and an "Ig" toggle in workspace search to include them
status: done
priority: 0
dependencies: []
---

See [Explorer](../PLAN.md#explorer) and [Workspace search](../PLAN.md#workspace-search).

**Explorer:** gitignored files and folders (`.gitignore`, `.git/info/exclude`, `.ignore`) are always listed, never hidden; no setting. They are drawn in a slightly darker grey than normal entries (new theme key, e.g. `explorer.ignored`, added to `Theme::dark_plus`), and a folder's whole subtree counts as ignored. Git status colouring wins where it applies. `.git/` and `.jed/` stay hidden and `.workspace` excludes still apply. Ignored state is computed off the UI thread with the directory listing (`ignore` crate's matcher, no `git2` on the UI thread) and refreshes with the live `notify` refresh. Large ignored trees (`target/`, `node_modules/`) are only listed when expanded, as any folder, so the initial listing stays cheap. Quick open keeps skipping ignored files.

**Workspace search:** an "Include ignored files" checkbox in the Search view, unchecked by default; checked, the search also walks files and directories `.gitignore` (and `.git/info/exclude`, `.ignore`) leaves out. `.git/`, `.jed/` and binary files are always skipped, `.workspace` excludes still apply, dirty buffers filtered the same way. Search view state like the other toggles (not in sessions); toggling re-searches; a keyboard/mouse-reachable checkbox that stays visible (or lit) while the details are hidden, as `…` is.

Tests: an ignored file is listed in the explorer with the ignored theme style while a normal one is not; a file under a gitignored `target/` is found by search only with the checkbox checked.

✅ Built.
- Explorer: `Entry::ignored`; `explorer::list_dir_in(dir, excludes, parent_ignored)` (two depth-1 walks, all vs non-ignored; a subtree of an ignored folder is ignored) in `request_listing`'s `spawn_blocking`; `AppEvent::Listed` carries the `ignored` flag it was asked with. Theme key `explorer.ignored`; git decoration wins. A changed `.gitignore` / `.ignore` / `.git/info/exclude` relists the loaded folders below it; `set_children` returns loaded folders whose flag changed.
- Search: `SearchView::include_ignored` (not in sessions), `Job::include_ignored` → `explorer::without_ignore_files`; fourth toggle `Ig` on the options row (always visible), `SearchCommand::ToggleIgnored` ("Search: Toggle Include Ignored Files", unbound).
- Deferred: none.
- Still to check by hand: the `explorer.ignored` grey against a real terminal theme; the `Ig` toggle fitting a narrow side panel.
