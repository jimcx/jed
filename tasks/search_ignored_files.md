---
id: search_ignored_files
name: Search ignored files toggle
summary: "a toggle button beside the \"files to exclude\" input (VS Code's \"Use Exclude Settings and Ignore Files\"), on by default; off, the search also walks files `.gitignore` (and `.git/info/exclude`, `.ignore`) leaves out. `.git/`, `.jed/` and binary files are always skipped, `.workspace` excludes still apply, dirty buffers filtered the same way. Search view state like the other toggles (not in sessions); toggling re-searches; lit while the details are hidden, as `…` is. Tests: a file under a gitignored `target/` found only with the toggle off"
status: open
priority: 0
dependencies: []
---

