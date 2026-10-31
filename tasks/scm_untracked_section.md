---
id: scm_untracked_section
name: Separate untracked files in Source Control
summary: a `[settings] untracked_changes` option splitting the Source Control view's Changes into tracked changes and an Untracked section
status: open
priority: 0
dependencies: []
---

The Source Control view shows modified, deleted and untracked files together in "Changes" (VS Code's `git.untrackedChanges: mixed`). Add a setting to split them: tracked changes stay in "Changes", untracked files go to their own "Untracked Changes" section (VS Code's `separate`).

  - Needs: `scm.rs` (`Section`, `files`, `ScmView::rows`), `app/scm.rs` (`stage_all`, `scm_stage`), `app/discard.rs` (`discard_all`, `scm_discard`), `ui/scm.rs`, `config/settings.rs` + `edit::Settings` ([adding a setting](../docs/dev/conventions.md#settings))
  - Setting: `[settings] untracked_changes = "mixed" | "separate"`, default `mixed` (today's behaviour). Read from `app.settings` (the view isn't per buffer); the view follows a settings reload.
  - Model: a new `Section::Untracked` ("Untracked Changes", `Pair::UNSTAGED`, shown only while it has files, after Changes). `Section::change` / `scm::files` take the mode: `mixed` — Changes has every work-tree change but a conflicted one, Untracked is empty; `separate` — Changes excludes `Change::Untracked`, Untracked holds only those. `ScmView::collapsed` grows to four entries. Switching the mode keeps the selection by `RowKey` (a file moved sections falls back as when it leaves one).
  - Actions: the Untracked header's `+` stages only its files and its discard button trashes only them (confirmation as for Changes); Changes' header in `separate` mode leaves untracked files alone. `stage_all` / `discard_all` (the commands, no row selected) still act on every unstaged file in either mode, as Stage All does in VS Code. `ui/scm.rs` draws the buttons on both headers, and `HitMap::scm` needs nothing new.
  - Done when: with `mixed` the view is unchanged; with `separate` a repo with a modified, a deleted and an untracked file shows Changes with the first two and Untracked Changes with the third, each header with its count, collapsible; staging the Untracked header stages only that file, discarding it trashes only that file; Stage All stages all three; changing the setting in `settings.toml` re-lays out the view live with the selection kept; the setting is in `TEMPLATE`, the settings table and `docs/dev/modules.md`. Tests drive the view headlessly (a temp repo, `press`, `mouse`) and unit-test `rows` for both modes.
  - Deferred: `git.untrackedChanges: hidden`, a per-workspace override beyond the normal `.workspace` layering
