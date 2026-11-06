---
id: mr_review
name: Code review
summary: review a Gitea, GitHub or GitLab merge request in jed — changed files, inline diffs with language server navigation, comments, drafts and suggestions
status: in-progress
priority: 0
dependencies: []
---

  - Check out a branch with an open merge request (MR) and review it in jed: its changes since the merge base in inline diffs, F12 and hover on the current code, other people's comments, your own comments as drafts submitted together, and suggestions applied to the buffer
  - Needs: the side panel lists and sections (`side_sections.rs`, `ui/side.rs`, `Scrollable`), diff tabs (`diff_view.rs`, `app/diff_view.rs`, `app/diff_edit.rs`, `ui/diff.rs`), git (`git/show.rs` rename detection, `git::diff`), language servers (`lsp/documents.rs`, `app/locations.rs`, `app/hover.rs`), tool logs (`logs::ToolLog`), `App::state_dir`
  - UI (chosen from mockups):
    - Side panel: the MR's summary, then FILES, then a collapsible CONVERSATIONS section with a draggable divider; a PENDING section while drafts exist
      ```
      CODE REVIEW
       !42 Add review panel
       feature/review → main
       7 files · 2 unresolved · open
      ▾ FILES
       ✓ M app/review.rs    +120 -4
         M ui/review.rs      +88      2●
         A forge/gitea.rs   +210
         D old_panel.rs          -64
      ▾ CONVERSATIONS (2)
       ● ui/review.rs:41  sam
         Can this overflow on a nar…
      ```
    - Comments: inline thread blocks between code lines, collapsible to one row
      ```
       41     - let n = w - 2;
           41 + let n = w - gutter;
       ┌ sam · 2h ago ─────── ▾ Resolve
       │ This can underflow when narrow.
       │ ```suggestion
       │ let n = w.saturating_sub(gutter);
       │ ```
       │ [Apply Suggestion] [Reply]
       └───────────────────────────────
       42  42   draw(n);
      ```
    - Composer: an inline box under the selection
      ```
       41 + let n = w - gutter;
       42 + draw(n);
       ┌ Comment on lines 41–42 ────────
       │ This can underflow.▏
       │
       │ [Insert Suggestion]
       │ [Comment Ctrl+Enter] [Cancel Esc]
       └────────────────────────────────
       43   finish();
      ```
    - Publishing: comments stay drafts until Submit Review (Comment, Approve or Request Changes)
    - A file opens as a new tab in the active group, reused per file like other diff tabs; a split is the existing split command
  - Forge layer (`src/forge/`: no app state, no drawing):
    - A `Forge` trait with `gitea.rs`, `github.rs` and `gitlab.rs`, on shared types: `MergeRequest`, `Thread`, `Comment`, `Anchor { path, side, start_line, end_line, commit }`, `Suggestion`, `DraftReview`
    - HTTP: `ureq` with rustls and the platform certificate verifier (so a private CA works). Blocking calls run in `spawn_blocking` and report through a new `AppEvent::Review`. GitHub GraphQL is a plain JSON POST
    - Config: a user-only `~/.config/jed/forges.toml` with `host`, `kind`, optional `api_url`, optional `token_env`. `github.com` and `gitlab.com` are detected; self-hosted servers need an entry. Tokens come from environment variables, by default `GITEA_TOKEN`, `GITHUB_TOKEN` or `GITLAB_TOKEN`
    - Remote URL mapping: as `scripts/mr` does, the SSH user and port are dropped (`ssh://git@git.example.com:2200/alice/project.git` → `https://git.example.com`)
    - Security: `.workspace` can never set a host, API URL or token variable; HTTPS only, unless an entry explicitly gives an `http://` URL; redirects are not followed with the token attached; tokens are never logged; control characters are stripped from all remote text before drawing
  - Finding the MR and its files (`src/review.rs`, `app/review.rs`, `ui/review.rs`):
    - `SidePanel::Review` between Source Control and Tools, icon `✎`; disabled in single file mode (`Mode::allows_view`)
    - Lookup at start, when HEAD moves and on manual refresh: open MRs matching the current branch and its upstream remote. Several matches: the newest, with a switcher. Empty states: no repository, unknown host, no token, detached HEAD, no MR
    - The merge base is the forge's base commit where available, else `git2::Repository::merge_base`. jed does not fetch (the git non-goal): missing objects show "run `git fetch`"
    - Files: the local diff of merge base to working tree, with the rename detection in `git/show.rs`
    - Viewed `✓` state is local (`App::state_dir`) and clears when the file's content changes
    - Refresh on showing the panel, on HEAD change, after a submit, on demand, and every 60 s only while the view or a review tab is visible. Errors appear in the panel and a tool log; the last result stays on screen
  - Inline diff tab:
    - `DiffTab` with `Text::Commit(base)` on the left and `Text::WorkTree` on the right (a new `Pair::review(base)`), and a per-tab forced inline flag read by `ui::diff::set_layout` (inline is otherwise only a fallback below `SIDE_BY_SIDE_MIN`)
    - `WorkTree` is live and already editable through `app/diff_edit.rs`, so edits and suggestions go through `Buffer::edit`
    - The file's buffer opens behind the tab (`DiffViews::pending`), so document sync (`lsp/documents.rs`) opens it on the server with no protocol changes
    - Deleted, binary and renamed files use the existing diff states; a deleted file has no right side
  - Language server navigation:
    - F12 and hover do nothing on diff tabs today: they go through `active_tab()`, which is `None` there. An `lsp_target()` helper returns `(group, buffer, at)` from a text tab or a review diff's right-side caret, used by `cursor_word` (`app/locations.rs`) and `show_hover` (`app/hover.rs`)
    - Scope: definition, references and hover, by keyboard and mouse. Results open in normal tabs; Back returns to the diff
    - The hover popup's anchor check in `ui/hover.rs` must accept the diff's caret row
    - On a deleted line these commands show "Deleted lines have no language server"
  - Thread blocks (the riskiest part: built alone, before the composer):
    - `diff_view::Row` stays as it is, so other diff tabs are unaffected. A review-only layout of `Block { after_row, height }` maps visual rows to aligned rows
    - `ui/diff.rs`, the caret, mouse hits, `top` and the scroll bar work through that mapping. Heights are recalculated on width or comment changes; drawing stays limited to visible rows
    - The caret steps over blocks; Next / Previous Thread commands and `HitMap` entries reach the buttons
  - Anchors and drift:
    - Each thread: the anchor commit's blob is diffed against the live text with `git::diff` and its lines mapped. A line inside a changed hunk makes the thread outdated: listed under CONVERSATIONS, not placed in the diff. A thread whose commit is missing locally is also listed there, read-only
    - Comments on deleted lines attach to the left-side row
    - Submit requires local HEAD to equal the MR head; otherwise a banner says "local is ahead / behind". A draft on a line changed locally is held back and named, never posted at a guessed position
  - Composer, drafts and submit:
    - The composer is a scratch `Buffer` and `View` driven through `edit::run`, as `diff_edit` does, with its own key context
    - Drafts are saved in `App::state_dir`, keyed by host, repo and MR, written on every change, removed only after the forge confirms them. A failed submit keeps them and reports per-draft errors
    - Reply, resolve / unresolve, edit / delete of your own comments. MR-level comments are read-only in CONVERSATIONS
    - Commands are in the palette and context menu, unbound by default; only the composer's submit and cancel get keys. A Cmd binding means updating `docs/terminals/README.md` and every terminal snippet
  - Suggestions:
    - Parsed into a normalised range: GitHub ` ```suggestion `, GitLab ` ```suggestion:-N+M `
    - Apply maps the range to the live buffer and compares the current text with the commit's text at those lines; if they differ it refuses with "suggestion is out of date" (which also handles overlapping suggestions)
    - A successful apply is one `EditKind::Other` undo step through `Buffer::edit`. It does not commit or resolve the thread (the "no commit" non-goal)
    - Insert Suggestion pre-fills the selected lines
  - Forge differences:
    - Gitea (verified in mr_review-a against a self-hosted server's own `/swagger.v1.json`, plus read-only calls on a repository with no review comments; the review-comment shapes below come from the swagger, not from live data, so check them on a throwaway MR):
      - Single-line only: creating a review comment takes `path`, `body`, `new_position` (line in the new file, else 0) and `old_position` (line in the old file, else 0); there is no start line, end line or side. jed anchors on the last line and adds a hidden range marker, which only means something to jed (the web page shows the comment on one line)
      - Reading a review comment (`GET /pulls/{n}/reviews/{id}/comments`): `id`, `path`, `body`, `position` (current line; 0 once outdated), `original_position`, `commit_id`, `original_commit_id`, `diff_hunk`, `resolver` (a user when resolved), `user`, timestamps, `html_url`. There is no side field (jed reads it from the last line of `diff_hunk`: `-` is the old file) and no reply-to field
      - Threads: not first-class. Comments on the same path, line and side form a conversation; `POST /pulls/{n}/comments/{id}/replies` replies to a comment, `POST /pulls/comments/{id}/resolve` and `/unresolve` take the first comment's id (jed uses it as the thread id and reads resolved from the first comment's `resolver`; unverified whether replies carry it too)
      - Suggestions: none in the API; the fence is treated as text, and jed applies it
      - Merge request: `GET /pulls/{n}` has `merge_base` (the merge base commit), `base` / `head` (`ref`, `sha`, `repo.full_name`), `draft`, `merged`, `state`; `GET /pulls/{n}/files` has `filename`, `previous_filename`, `status` (`added`, `changed`, `deleted`, `renamed`, ...), `additions`, `deletions`. The list endpoint (`GET /pulls?state=open`) cannot filter by head branch, so jed filters `head.ref` itself; the head `ref` of a closed MR whose branch is gone is `refs/pull/N/head`
      - Lists are paged with `page` and `limit` and send `X-Total-Count`; MR-level comments are `GET /issues/{n}/comments`; a review's summary is its `body`; a user's unsent review has state `PENDING`
    - GitHub: native multi-line anchors and suggestions; thread resolved state and resolve actions appear to need GraphQL (mostly community sources: check the schema reference)
    - GitLab: unverified. mr_review-i starts by checking the official API docs (`line_range`, suggestion syntax, discussions)
  - Done when: on a repository with an open MR, the Code Review view lists the MR and its files since the merge base; a file opens in an inline diff tab; F12 and hover work on right-side lines and say why not on deleted ones; other people's threads show inline, or under CONVERSATIONS when outdated; a comment on selected lines is kept as a draft across a restart and a failed submit, then posted by Submit Review; a suggestion applies as one undo step and refuses when out of date; `.workspace` can't set a host or token; and all of it works on Gitea, GitHub and GitLab
  - Hand checks: on a self-hosted Gitea server with `GITEA_TOKEN` and a throwaway MR: open the panel and a file; F12 on an added line, then Back; add a multi-line draft, restart jed, submit; see how it looks on Gitea's web page; apply a suggestion and undo it. The same on GitHub and GitLab test repositories
  - Subtasks (one agent and one commit each):
    - ✅ mr_review-a: forge types, config, URL mapping, HTTP layer, a local fake server, Gitea reads. First verify Gitea's behaviour and record it in this file. Built: `src/forge/` (types, `Forge` trait, `forges.toml`, `parse_remote`, ureq `Client`, `FakeServer`, `gitea.rs` reads; the `mod forge` has `#[allow(dead_code)]` until b). Gitea thread side/outdated/resolver semantics still need a hand check on a throwaway MR
    - ✅ mr_review-b: activity icon, panel, MR lookup, merge base, FILES, viewed state, refresh and empty states. Built: `SidePanel::Review`, `review.rs` / `app/review.rs` / `ui/review.rs`, `git/range.rs`, viewed marks in `review-viewed.json`, `Tool::Review` log; Enter on a file only selects (c opens the diff). Open: `FileChange.status` is a `char` (an enum would need git/show.rs too); `ReviewHit.rows` clones a path per visible row per frame
    - ✅ mr_review-c: the forced inline review diff tab, opened from the panel. Built: `Pair::review`, `DiffTab::review` (`forced_inline()` from the pair, `left_path` for renames), `App::open_diff_tab`, Enter/click on a FILES row opens it; a background open of a binary file is dropped (no hex tab over the diff). Deferred: session restore keeps the pair but not `left_path`; typing on a deleted file's right side reports an error
    - mr_review-d: definition, references and hover on right-side lines
    - mr_review-e: thread blocks (read-only), anchoring and outdated threads, CONVERSATIONS, next / previous thread
    - mr_review-f: composer, saved drafts, PENDING, submit, reply, resolve, edit, delete, Gitea writes
    - mr_review-g: suggestion parsing, Apply Suggestion and Insert Suggestion
    - mr_review-h: the GitHub adapter (REST and GraphQL)
    - mr_review-i: the GitLab adapter. First check its API against the official docs
  - Tests: `src/mr_review_tests.rs` drives the app headlessly against a local fake forge and `FakeLauncher`, on `git::test_repo` repositories; a release budget test (`milestone` in its name) covers a large diff with many threads
  - Deferred: reviewing without checking out the branch; creating or merging MRs; committing a suggestion; Apply All Suggestions; CI status; fork MRs not on the upstream remote; reactions and images in comments; full Markdown rendering in threads; rename and code actions from the diff; a side-by-side review layout
