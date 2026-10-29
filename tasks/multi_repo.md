---
id: multi_repo
name: Multiple git repos in one workspace
summary: source control, quick diff marks, blame, history and the git tools work with several repositories under one workspace folder, as VS Code does
status: open
priority: -10
dependencies: []
---

  - Today the workspace has one repository (`git::` discovers it from the root); a workspace folder that holds several repos side by side (or nested ones, or a non-repo parent) shows nothing or only one
  - Discover every repository under the workspace root (a bounded, off-thread scan honouring ignores; `.git` dirs and files, so worktrees and submodules count), plus the repo a file belongs to via the nearest enclosing one; rescan when a `.git` appears or goes
  - Source control view: one section per repository (name, branch, change count), each collapsible, with its own commit box, stage / unstage / discard, sync and branch actions; the status bar branch shows the active file's repo
  - Per-file features (quick diff, blame, file history, diff tabs, conflicts) resolve the repo from the file's path, not the workspace root; the explorer's git decorations use the owning repo
  - Setting `git.autoRepositoryDetection` (`true` / `false` / `subFolders` / `openEditors`) and `git.repositoryScanMaxDepth`, as VS Code; repos can be closed / hidden from the view
  - Git operations stay off the UI thread (`spawn_blocking` → `AppEvent`), one status read per repo; many repos must not make a refresh slower than the slowest one
  - Needs: `git/` (repo discovery), `scm.rs`, `app/blame.rs`, the quick diff and explorer decorations code; split into subtasks (`multi_repo-a` discovery and per-file resolution, `-b` the SCM view, …) when built
  - Done when: a workspace with two sibling repos shows both in the source control view, each file's marks / blame / history come from its own repo, and committing in one leaves the other untouched
